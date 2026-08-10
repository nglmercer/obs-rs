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

    /// Encodes one frame into one validated packet.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the frame format or encoded payload is
    /// invalid.
    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError>;

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
        EncodedPacket::new(
            PacketKind::Video,
            frame.timestamp(),
            true,
            frame.pixels().to_vec(),
        )
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

    let mut raw = Vec::with_capacity(uncompressed_bytes);
    for row in frame.pixels().chunks_exact(scanline_bytes) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let zlib = zlib_stored(&raw);
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
    let mut crc_input = Vec::with_capacity(kind.len() + payload.len());
    crc_input.extend_from_slice(&kind);
    crc_input.extend_from_slice(payload);
    push_u32_be(output, crc32(&crc_input));
}

fn push_u32_be(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut sum_a = 1_u32;
    let mut sum_b = 0_u32;
    for byte in bytes {
        sum_a = (sum_a + u32::from(*byte)) % 65_521;
        sum_b = (sum_b + sum_a) % 65_521;
    }
    (sum_b << 16) | sum_a
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

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

        let mut payload = Vec::new();
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
            for _ in 0..run {
                decoded_pixels.extend_from_slice(&pixel);
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
