use std::io::Cursor;

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    codec::{read_exact, read_u32, write_all, write_u32},
    error::OutputError,
    types::{EncodedPacket, PacketKind},
    MAX_PACKET_BYTES, PNG_SIGNATURE, RLE_MAGIC,
};

pub trait VideoEncoder {
    /// Returns the fixed input format accepted by the encoder.
    fn format(&self) -> VideoFormat;

    /// Encodes one borrowed frame into one validated packet.
    ///
    /// Because the frame is borrowed, an encoder whose payload is the pixel
    /// buffer itself has no choice but to copy it — a full frame per call, which
    /// at 4K60 is roughly 500 MB/s of pure memcpy. **If the caller is finished
    /// with the frame, use [`VideoEncoder::encode_owned`] instead**, which lets
    /// those encoders move the buffer into the packet for free. Reach for this
    /// method only when the frame is genuinely still needed afterwards.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the frame format or encoded payload is
    /// invalid.
    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError>;

    /// Encodes one owned frame into one validated packet.
    ///
    /// Callers that no longer need the frame should prefer this: an encoder
    /// whose payload is the pixel buffer itself can move it into the packet
    /// rather than copying it. The default implementation simply borrows, so
    /// codecs that must transform the pixels need not override it.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the frame format or encoded payload is
    /// invalid.
    fn encode_owned(&mut self, frame: VideoFrame) -> Result<EncodedPacket, OutputError> {
        self.encode(&frame)
    }

    /// Flushes delayed packets, if the codec has any.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when flushing the codec fails.
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError>;
}

/// A deterministic one-packet-per-frame reference encoder.
pub struct RawVideoEncoder {
    format: VideoFormat,
}

impl RawVideoEncoder {
    /// Creates an encoder for one fixed RGBA video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self { format }
    }
}

impl VideoEncoder for RawVideoEncoder {
    fn format(&self) -> VideoFormat {
        self.format
    }

    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        // This encoder's payload *is* the pixel buffer, so a borrowed frame
        // forces a full copy here. `encode_owned` avoids it entirely; see the
        // trait docs.
        EncodedPacket::new(
            PacketKind::Video,
            frame.timestamp(),
            true,
            frame.pixels().to_vec(),
        )
    }

    fn encode_owned(&mut self, frame: VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        // The raw payload *is* the pixel buffer, so ownership transfers with no
        // copy at all.
        let timestamp = frame.timestamp();
        EncodedPacket::new(PacketKind::Video, timestamp, true, frame.into_pixels())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// A standards-based PNG screenshot encoder implemented without native codecs.
///
/// The PNG uses RGBA8 pixels and zlib stored blocks. Stored blocks are intentionally
/// simple and deterministic; they provide interoperability and a safe reference
/// path, while a later production encoder may replace the compression strategy
/// behind the same [`VideoEncoder`] contract.
pub struct PngVideoEncoder {
    format: VideoFormat,
}

impl PngVideoEncoder {
    /// Creates an encoder for one fixed RGBA video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self { format }
    }
}

impl VideoEncoder for PngVideoEncoder {
    fn format(&self) -> VideoFormat {
        self.format
    }

    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        EncodedPacket::new(
            PacketKind::Video,
            frame.timestamp(),
            true,
            encode_png(frame)?,
        )
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// Encodes one owned RGBA frame as an interoperable PNG image.
///
/// # Errors
///
/// Returns [`OutputError::TooLarge`] when the deterministic stored-block PNG would
/// exceed the packet safety limit.
pub fn encode_png(frame: &VideoFrame) -> Result<Vec<u8>, OutputError> {
    let format = frame.format();
    let width =
        usize::try_from(format.width()).map_err(|_| OutputError::TooLarge { bytes: u64::MAX })?;
    let height =
        usize::try_from(format.height()).map_err(|_| OutputError::TooLarge { bytes: u64::MAX })?;
    let scanline_bytes = width
        .checked_mul(4)
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    let uncompressed_bytes = scanline_bytes
        .checked_add(1)
        .and_then(|bytes| bytes.checked_mul(height))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    let blocks = uncompressed_bytes.saturating_add(65_534) / 65_535;
    let zlib_bytes = 2_usize
        .checked_add(uncompressed_bytes)
        .and_then(|bytes| bytes.checked_add(blocks.saturating_mul(5)))
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    let encoded_bytes = PNG_SIGNATURE
        .len()
        .checked_add(12 + 13)
        .and_then(|bytes| bytes.checked_add(12 + zlib_bytes))
        .and_then(|bytes| bytes.checked_add(12))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    if encoded_bytes > MAX_PACKET_BYTES {
        return Err(OutputError::TooLarge {
            bytes: u64::try_from(encoded_bytes).unwrap_or(u64::MAX),
        });
    }

    // The filtered scanlines are produced straight into the zlib stream, so a
    // full-frame intermediate buffer is never materialized.
    let zlib = zlib_stored_scanlines(frame.pixels(), scanline_bytes, uncompressed_bytes);
    let mut png = Vec::with_capacity(encoded_bytes);
    png.extend_from_slice(PNG_SIGNATURE);
    let mut header = Vec::with_capacity(13);
    push_u32_be(&mut header, format.width());
    push_u32_be(&mut header, format.height());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    append_png_chunk(&mut png, *b"IHDR", &header);
    append_png_chunk(&mut png, *b"IDAT", &zlib);
    append_png_chunk(&mut png, *b"IEND", &[]);
    Ok(png)
}

/// Builds a stored-mode zlib stream over PNG scanlines without materializing
/// the filtered image first.
///
/// Each scanline contributes a leading zero filter byte followed by its pixels;
/// both are written directly into the output blocks and folded into the Adler
/// checksum as they go.
fn zlib_stored_scanlines(
    pixels: &[u8],
    scanline_bytes: usize,
    uncompressed_bytes: usize,
) -> Vec<u8> {
    let blocks = uncompressed_bytes.saturating_add(65_534) / 65_535;
    let mut output = Vec::with_capacity(2 + uncompressed_bytes + blocks.saturating_mul(5) + 4);
    output.extend_from_slice(&[0x78, 0x01]);

    if uncompressed_bytes == 0 {
        output.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
        push_u32_be(&mut output, ADLER32_INIT_VALUE);
        return output;
    }

    let mut adler = Adler32::new();
    let mut written = 0_usize;
    let mut block_remaining = 0_usize;
    let mut rows = pixels.chunks_exact(scanline_bytes);
    let mut pending: &[u8] = &[];
    let mut needs_filter_byte = true;

    while written < uncompressed_bytes {
        if block_remaining == 0 {
            let block_len = (uncompressed_bytes - written).min(65_535);
            let final_block = written + block_len == uncompressed_bytes;
            output.push(u8::from(final_block));
            let length = u16::try_from(block_len).unwrap_or(u16::MAX);
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&(!length).to_le_bytes());
            block_remaining = block_len;
        }

        if needs_filter_byte {
            output.push(0);
            adler.update(&[0]);
            written += 1;
            block_remaining -= 1;
            needs_filter_byte = false;
            pending = rows.next().unwrap_or(&[]);
            continue;
        }

        let take = pending.len().min(block_remaining);
        let (head, tail) = pending.split_at(take);
        output.extend_from_slice(head);
        adler.update(head);
        written += take;
        block_remaining -= take;
        pending = tail;
        if pending.is_empty() {
            needs_filter_byte = true;
        }
    }

    push_u32_be(&mut output, adler.finish());
    output
}

/// Adler-32 checksum value for an empty input.
const ADLER32_INIT_VALUE: u32 = 1;

/// A streaming Adler-32 accumulator.
struct Adler32 {
    sum_a: u32,
    sum_b: u32,
}

impl Adler32 {
    const fn new() -> Self {
        Self { sum_a: 1, sum_b: 0 }
    }

    fn update(&mut self, bytes: &[u8]) {
        // NMAX is the largest block for which both accumulators stay within
        // u32, so the expensive modulo is paid once per block rather than once
        // per byte.
        for block in bytes.chunks(5_552) {
            for byte in block {
                self.sum_a += u32::from(*byte);
                self.sum_b += self.sum_a;
            }
            self.sum_a %= 65_521;
            self.sum_b %= 65_521;
        }
    }

    const fn finish(&self) -> u32 {
        (self.sum_b << 16) | self.sum_a
    }
}

/// Buffered stored-mode zlib encoder, retained as the oracle that
/// [`zlib_stored_scanlines`] is proven byte-identical against.
#[cfg(test)]
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let blocks = raw.len().saturating_add(65_534) / 65_535;
    let mut output = Vec::with_capacity(2 + raw.len() + blocks.saturating_mul(5) + 4);
    output.extend_from_slice(&[0x78, 0x01]);
    if raw.is_empty() {
        output.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    } else {
        for (index, block) in raw.chunks(65_535).enumerate() {
            let final_block = index + 1 == blocks;
            output.push(u8::from(final_block));
            let length = u16::try_from(block.len()).unwrap_or(u16::MAX);
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&(!length).to_le_bytes());
            output.extend_from_slice(block);
        }
    }
    push_u32_be(&mut output, adler32(raw));
    output
}

fn append_png_chunk(output: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) {
    push_u32_be(output, u32::try_from(payload.len()).unwrap_or(u32::MAX));
    output.extend_from_slice(&kind);
    output.extend_from_slice(payload);
    // CRC is a streaming checksum, so it runs over the tag and the payload in
    // place rather than over a concatenated temporary.
    let crc = crc32_update(crc32_update(CRC32_INIT, &kind), payload);
    push_u32_be(output, crc ^ 0xffff_ffff);
}

fn push_u32_be(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
fn adler32(bytes: &[u8]) -> u32 {
    let mut sum_a = 1_u32;
    let mut sum_b = 0_u32;
    for byte in bytes {
        sum_a = (sum_a + u32::from(*byte)) % 65_521;
        sum_b = (sum_b + sum_a) % 65_521;
    }
    (sum_b << 16) | sum_a
}

/// Initial CRC-32 register value, before any bytes are folded in.
pub(crate) const CRC32_INIT: u32 = u32::MAX;

/// Returns the finished CRC-32 of one contiguous input.
///
/// Production code checksums multi-part chunks with [`crc32_update`]; this
/// one-shot form exists for tests that verify the chunk contract.
#[cfg(test)]
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    crc32_update(CRC32_INIT, bytes) ^ 0xffff_ffff
}

/// Folds `bytes` into a running CRC-32 register.
///
/// Exposed so multi-part inputs can be checksummed without first being
/// concatenated into one buffer.
pub(crate) fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        let index = usize::from(
            u8::try_from((crc ^ u32::from(*byte)) & u32::from(u8::MAX)).unwrap_or_default(),
        );
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    crc
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the table index is bounded by the 256-entry table"
)]
const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0_usize;
    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xedb8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

/// A deterministic lossless RGBA run-length video encoder.
pub struct RleVideoEncoder {
    format: VideoFormat,
}

impl RleVideoEncoder {
    /// Creates an encoder for one fixed RGBA video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self { format }
    }
}

impl VideoEncoder for RleVideoEncoder {
    fn format(&self) -> VideoFormat {
        self.format
    }

    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }

        // Worst case is one 8-byte run record per pixel; half that is a sane
        // starting point for real content and avoids the early doublings.
        let mut payload = Vec::with_capacity(RLE_MAGIC.len() + frame.pixels().len());
        write_all(&mut payload, RLE_MAGIC)?;
        let mut current = None;
        let mut run_length = 0_u32;
        for chunk in frame.pixels().chunks_exact(4) {
            let pixel = [chunk[0], chunk[1], chunk[2], chunk[3]];
            if current == Some(pixel) && run_length < u32::MAX {
                run_length += 1;
                continue;
            }
            if let Some(previous) = current {
                write_u32(&mut payload, run_length)?;
                write_all(&mut payload, &previous)?;
            }
            current = Some(pixel);
            run_length = 1;
        }
        if let Some(last) = current {
            write_u32(&mut payload, run_length)?;
            write_all(&mut payload, &last)?;
        }
        EncodedPacket::new(PacketKind::Video, frame.timestamp(), true, payload)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// Decoder for the deterministic lossless RLE video payload.
pub struct RleVideoDecoder;

impl RleVideoDecoder {
    /// Decodes one RLE video packet payload into an owned RGBA frame.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidCodecPayload`] for invalid runs, a wrong
    /// header, or trailing bytes; returns [`OutputError::Truncated`] for a short
    /// run record and [`OutputError::Media`] for a violated frame invariant.
    pub fn decode(
        format: VideoFormat,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> Result<VideoFrame, OutputError> {
        if payload.len() > MAX_PACKET_BYTES {
            return Err(OutputError::PacketTooLarge {
                bytes: payload.len(),
            });
        }
        let mut cursor = Cursor::new(payload);
        let mut magic = [0_u8; 8];
        read_exact(&mut cursor, &mut magic)?;
        if &magic != RLE_MAGIC {
            return Err(OutputError::InvalidCodecPayload(
                "invalid RLE header".to_owned(),
            ));
        }

        let expected_pixels = format.pixel_count();
        let mut decoded_pixels = Vec::with_capacity(format.rgba_bytes());
        let mut decoded_count = 0_usize;
        while decoded_count < expected_pixels {
            let run = usize::try_from(read_u32(&mut cursor)?).unwrap_or(usize::MAX);
            if run == 0 || run > expected_pixels.saturating_sub(decoded_count) {
                return Err(OutputError::InvalidCodecPayload(
                    "RLE run exceeds the expected pixel count".to_owned(),
                ));
            }
            let mut pixel = [0_u8; 4];
            read_exact(&mut cursor, &mut pixel)?;
            // Write the run's first pixel, then double the tail in place so the
            // run is filled by a few block copies instead of one call per pixel.
            let run_start = decoded_pixels.len();
            let run_bytes = run * 4;
            decoded_pixels.extend_from_slice(&pixel);
            while decoded_pixels.len() - run_start < run_bytes {
                let filled = decoded_pixels.len() - run_start;
                let remaining = run_bytes - filled;
                let copy = filled.min(remaining);
                decoded_pixels.extend_from_within(run_start..run_start + copy);
            }
            decoded_count += run;
        }
        if cursor.position() != u64::try_from(payload.len()).unwrap_or(u64::MAX) {
            return Err(OutputError::InvalidCodecPayload(
                "RLE payload has trailing bytes".to_owned(),
            ));
        }
        VideoFrame::new(format, timestamp, decoded_pixels).map_err(OutputError::Media)
    }
}

#[cfg(test)]
mod zlib_equivalence_tests {
    use super::{zlib_stored, zlib_stored_scanlines};

    /// The streaming scanline encoder must be byte-identical to building the
    /// filtered image up front and compressing it in one go.
    #[test]
    fn streaming_scanlines_match_the_buffered_encoder() {
        for scanline_bytes in [4_usize, 12, 4 * 300, 65_535, 65_536] {
            for rows in [0_usize, 1, 2, 5] {
                let pixels: Vec<u8> = (0..scanline_bytes * rows)
                    .map(|index| u8::try_from(index % 251).unwrap_or(0))
                    .collect();
                let uncompressed = rows * (scanline_bytes + 1);

                let mut raw = Vec::with_capacity(uncompressed);
                for row in pixels.chunks_exact(scanline_bytes.max(1)) {
                    raw.push(0);
                    raw.extend_from_slice(row);
                }

                assert_eq!(
                    zlib_stored_scanlines(&pixels, scanline_bytes, uncompressed),
                    zlib_stored(&raw),
                    "scanline_bytes={scanline_bytes} rows={rows}"
                );
            }
        }
    }
}
