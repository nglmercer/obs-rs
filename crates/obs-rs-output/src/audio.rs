use obs_rs_audio::{AudioBuffer, AudioFormat, MAX_AUDIO_FRAMES};
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    codec::{pcm16_bits, write_all, write_u16, write_u32},
    error::OutputError,
    types::{EncodedPacket, PacketKind},
    MAX_RECORDING_BYTES, MAX_RECORDING_FRAMES,
};

pub trait AudioEncoder {
    /// Returns the fixed input format accepted by the encoder.
    fn format(&self) -> AudioFormat;

    /// Encodes one audio buffer into one validated packet.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the buffer format or encoded payload is
    /// invalid.
    fn encode(&mut self, buffer: &AudioBuffer) -> Result<EncodedPacket, OutputError>;

    /// Encodes one owned audio buffer. Codecs that can consume the sample
    /// storage may override this to avoid a defensive copy at their boundary.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when encoding fails.
    fn encode_owned(&mut self, buffer: AudioBuffer) -> Result<EncodedPacket, OutputError> {
        self.encode(&buffer)
    }

    /// Flushes delayed packets, if the codec has any.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when flushing the codec fails.
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError>;
}

/// A deterministic little-endian PCM audio encoder.
pub struct RawAudioEncoder {
    format: AudioFormat,
}

impl RawAudioEncoder {
    /// Creates an encoder for one fixed interleaved audio format.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self { format }
    }
}

impl AudioEncoder for RawAudioEncoder {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn encode(&mut self, buffer: &AudioBuffer) -> Result<EncodedPacket, OutputError> {
        if buffer.format() != self.format {
            return Err(OutputError::AudioFormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        encode_pcm_payload(buffer.timestamp(), buffer.samples())
    }

    fn encode_owned(&mut self, buffer: AudioBuffer) -> Result<EncodedPacket, OutputError> {
        if buffer.format() != self.format {
            return Err(OutputError::AudioFormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        let timestamp = buffer.timestamp();
        let samples = buffer.into_samples();
        encode_pcm_payload(timestamp, &samples)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

fn encode_pcm_payload(timestamp: Timestamp, samples: &[f32]) -> Result<EncodedPacket, OutputError> {
    let mut payload = Vec::with_capacity(samples.len().saturating_mul(4));
    for sample in samples {
        payload.extend_from_slice(&sample.to_le_bytes());
    }
    EncodedPacket::new(PacketKind::Audio, timestamp, false, payload)
}

/// An offline PCM16 WAV recording assembled from one fixed audio format.
pub struct WavRecording {
    format: AudioFormat,
    /// All appended samples, interleaved and contiguous.
    ///
    /// A `Vec<AudioBuffer>` scattered each block across its own allocation and
    /// cost a cache miss per buffer during encoding; one flat buffer makes the
    /// encode a linear walk.
    samples: Vec<f32>,
    frames: usize,
}

impl WavRecording {
    /// Creates an empty WAV recording.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            samples: Vec::new(),
            frames: 0,
        }
    }

    /// Appends one complete interleaved buffer.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::AudioFormatMismatch`] for a different format or
    /// [`OutputError::TooLarge`] when the resulting WAV exceeds the reference
    /// byte budget.
    pub fn push(&mut self, buffer: AudioBuffer) -> Result<(), OutputError> {
        if buffer.format() != self.format {
            return Err(OutputError::AudioFormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        let frames = self
            .frames
            .checked_add(buffer.frames())
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if frames > MAX_AUDIO_FRAMES {
            return Err(OutputError::TooLarge {
                bytes: u64::try_from(frames).unwrap_or(u64::MAX),
            });
        }
        let data_bytes = frames
            .checked_mul(usize::from(self.format.channels()))
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let encoded_bytes = 44_usize
            .checked_add(data_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        self.frames = frames;
        self.samples.extend(buffer.into_samples());
        Ok(())
    }

    /// Returns the fixed recording format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the number of interleaved audio frames.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// Returns whether no audio frames have been appended.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Encodes the recording as a canonical little-endian PCM16 WAV file.
    ///
    /// Samples are clamped to `[-1.0, 1.0]` and converted deterministically to
    /// signed 16-bit PCM. Buffer timestamps are intentionally not serialized by
    /// WAV; timestamped packet output remains available through the packet API.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::TooLarge`] when the encoded length cannot fit in a
    /// standard WAV 32-bit chunk size.
    pub fn encode(&self) -> Result<Vec<u8>, OutputError> {
        let data_bytes = self
            .frames
            .checked_mul(usize::from(self.format.channels()))
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let riff_size = 36_usize
            .checked_add(data_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let data_bytes_u32 = u32::try_from(data_bytes).map_err(|_| OutputError::TooLarge {
            bytes: data_bytes as u64,
        })?;
        let riff_size_u32 = u32::try_from(riff_size).map_err(|_| OutputError::TooLarge {
            bytes: riff_size as u64,
        })?;
        let byte_rate = self
            .format
            .sample_rate()
            .checked_mul(u32::from(self.format.channels()))
            .and_then(|value| value.checked_mul(2))
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let block_align = self
            .format
            .channels()
            .checked_mul(2)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;

        let mut bytes = Vec::with_capacity(44 + data_bytes);
        write_all(&mut bytes, b"RIFF")?;
        write_u32(&mut bytes, riff_size_u32)?;
        write_all(&mut bytes, b"WAVEfmt ")?;
        write_u32(&mut bytes, 16)?;
        write_u16(&mut bytes, 1)?;
        write_u16(&mut bytes, self.format.channels())?;
        write_u32(&mut bytes, self.format.sample_rate())?;
        write_u32(&mut bytes, byte_rate)?;
        write_u16(&mut bytes, block_align)?;
        write_u16(&mut bytes, 16)?;
        write_all(&mut bytes, b"data")?;
        write_u32(&mut bytes, data_bytes_u32)?;
        for sample in &self.samples {
            write_u16(&mut bytes, pcm16_bits(*sample))?;
        }
        Ok(bytes)
    }
}

/// An interoperable YUV4MPEG2 recording with pure-Rust RGBA-to-4:2:0 conversion.
///
/// Y4M is an uncompressed frame container intended for exchange and reference
/// tooling. It is deliberately not presented as a distribution codec; it gives the
/// output pipeline a standard file artifact without a native encoder dependency.
pub struct Y4mRecording {
    pub(crate) format: VideoFormat,
    pub(crate) frames: Vec<VideoFrame>,
    pub(crate) encoded_bytes: usize,
    pub(crate) last_timestamp: Option<Timestamp>,
    /// The stream header, formatted once because the format never changes.
    pub(crate) header: String,
    /// Reused conversion scratch owned by the recording rather than allocated
    /// once per encoded frame.
    chroma_u: Vec<u8>,
    chroma_v: Vec<u8>,
    luma_top: Vec<u8>,
    luma_bottom: Vec<u8>,
}

impl Y4mRecording {
    /// Creates an empty Y4M recording for one fixed video format.
    #[must_use]
    pub fn new(format: VideoFormat) -> Self {
        Self {
            format,
            frames: Vec::new(),
            encoded_bytes: 0,
            last_timestamp: None,
            header: y4m_header(format),
            chroma_u: Vec::with_capacity(format.pixel_count() / 4),
            chroma_v: Vec::with_capacity(format.pixel_count() / 4),
            luma_top: Vec::with_capacity(usize::try_from(format.width()).unwrap_or(0)),
            luma_bottom: Vec::with_capacity(usize::try_from(format.width()).unwrap_or(0)),
        }
    }

    /// Adds one RGBA frame after validating dimensions, timestamps, and budget.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::UnsupportedFormat`] for odd dimensions,
    /// [`OutputError::FormatMismatch`] for another video format,
    /// [`OutputError::NonMonotonicTimestamp`] for backward timestamps, or
    /// [`OutputError::TooLarge`] when the Y4M recording exceeds the reference
    /// recording budget.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.validate_format()?;
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        if let Some(previous) = self.last_timestamp {
            if frame.timestamp() < previous {
                return Err(OutputError::NonMonotonicTimestamp {
                    previous,
                    actual: frame.timestamp(),
                });
            }
        }
        if self.frames.len() >= MAX_RECORDING_FRAMES {
            return Err(OutputError::TooManyFrames {
                frames: self.frames.len() as u64 + 1,
            });
        }
        let encoded_bytes =
            y4m_encoded_size(self.header.len(), self.format, self.frames.len() + 1)?;
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        // The timestamp is already owned here; no need to re-read the vector's
        // last element after moving the frame in.
        let timestamp = frame.timestamp();
        self.frames.push(frame);
        self.encoded_bytes = encoded_bytes;
        self.last_timestamp = Some(timestamp);
        Ok(())
    }

    /// Returns the fixed recording format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the number of accepted frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns whether no frames have been accepted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Encodes the recording as a YUV4MPEG2 byte stream.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::UnsupportedFormat`] for odd dimensions or a write
    /// error from the in-memory output path.
    pub fn encode(&mut self) -> Result<Vec<u8>, OutputError> {
        self.validate_format()?;
        let mut bytes = Vec::with_capacity(self.encoded_bytes.max(self.header.len()));
        write_all(&mut bytes, self.header.as_bytes())?;
        for frame in &self.frames {
            write_y4m_frame_reusing(
                &mut bytes,
                frame,
                &mut self.chroma_u,
                &mut self.chroma_v,
                &mut self.luma_top,
                &mut self.luma_bottom,
            );
        }
        Ok(bytes)
    }

    fn validate_format(&self) -> Result<(), OutputError> {
        if !self.format.width().is_multiple_of(2) || !self.format.height().is_multiple_of(2) {
            return Err(OutputError::UnsupportedFormat {
                reason: "YUV4MPEG2 4:2:0 requires even width and height".to_owned(),
            });
        }
        Ok(())
    }
}

fn y4m_header(format: VideoFormat) -> String {
    format!(
        "YUV4MPEG2 W{} H{} F{}:{} Ip A0:0 C420jpeg\n",
        format.width(),
        format.height(),
        format.frame_rate().numerator(),
        format.frame_rate().denominator()
    )
}

fn y4m_encoded_size(
    header_bytes: usize,
    format: VideoFormat,
    frames: usize,
) -> Result<usize, OutputError> {
    let frame_bytes = 6_usize
        .checked_add(format.pixel_count())
        .and_then(|bytes| bytes.checked_add(format.pixel_count() / 2))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    header_bytes
        .checked_add(
            frames
                .checked_mul(frame_bytes)
                .ok_or(OutputError::TooLarge { bytes: u64::MAX })?,
        )
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })
}

fn write_y4m_frame_reusing(
    output: &mut Vec<u8>,
    frame: &VideoFrame,
    chroma_u: &mut Vec<u8>,
    chroma_v: &mut Vec<u8>,
    luma_top: &mut Vec<u8>,
    luma_bottom: &mut Vec<u8>,
) {
    output.extend_from_slice(b"FRAME\n");
    let format = frame.format();
    let width = usize::try_from(format.width()).unwrap_or(usize::MAX);
    let pixels = frame.pixels();
    let row_bytes = width.saturating_mul(4);
    if row_bytes == 0 {
        return;
    }

    // Single pass: every pixel goes through `rgb_to_yuv` exactly once and all
    // three components are kept. Rows are processed in vertical pairs so the
    // 2x2 chroma average is available without a second conversion pass, and
    // only two scanlines of luma are buffered at a time.
    chroma_u.clear();
    chroma_v.clear();
    luma_top.clear();
    luma_bottom.clear();

    for rows in pixels.chunks_exact(row_bytes * 2) {
        let (top, bottom) = rows.split_at(row_bytes);
        luma_top.clear();
        luma_bottom.clear();

        for (top_pair, bottom_pair) in top.chunks_exact(8).zip(bottom.chunks_exact(8)) {
            let mut u_sum = 0_u32;
            let mut v_sum = 0_u32;
            for pixel in top_pair.chunks_exact(4) {
                let (y, u, v) = rgb_to_yuv(pixel[0], pixel[1], pixel[2]);
                luma_top.push(y);
                u_sum += u32::from(u);
                v_sum += u32::from(v);
            }
            for pixel in bottom_pair.chunks_exact(4) {
                let (y, u, v) = rgb_to_yuv(pixel[0], pixel[1], pixel[2]);
                luma_bottom.push(y);
                u_sum += u32::from(u);
                v_sum += u32::from(v);
            }
            chroma_u.push(u8::try_from((u_sum + 2) / 4).unwrap_or(u8::MAX));
            chroma_v.push(u8::try_from((v_sum + 2) / 4).unwrap_or(u8::MAX));
        }

        output.extend_from_slice(luma_top);
        output.extend_from_slice(luma_bottom);
    }

    output.extend_from_slice(chroma_u);
    output.extend_from_slice(chroma_v);
}

#[cfg(test)]
fn write_y4m_frame(output: &mut Vec<u8>, frame: &VideoFrame) {
    let format = frame.format();
    let width = usize::try_from(format.width()).unwrap_or(0);
    let mut chroma_u = Vec::with_capacity(format.pixel_count() / 4);
    let mut chroma_v = Vec::with_capacity(format.pixel_count() / 4);
    let mut luma_top = Vec::with_capacity(width);
    let mut luma_bottom = Vec::with_capacity(width);
    write_y4m_frame_reusing(
        output,
        frame,
        &mut chroma_u,
        &mut chroma_v,
        &mut luma_top,
        &mut luma_bottom,
    );
}

fn rgb_to_yuv(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    let red = i32::from(red);
    let green = i32::from(green);
    let blue = i32::from(blue);
    let y = (77 * red + 150 * green + 29 * blue + 128) >> 8;
    let u = (-43 * red - 85 * green + 128 * blue + 32_768) >> 8;
    let v = (128 * red - 107 * green - 21 * blue + 32_768) >> 8;
    (clamp_byte(y), clamp_byte(u), clamp_byte(v))
}

fn clamp_byte(value: i32) -> u8 {
    u8::try_from(value.clamp(0, i32::from(u8::MAX))).unwrap_or_default()
}

#[cfg(test)]
mod y4m_equivalence_tests {
    use super::{rgb_to_yuv, write_y4m_frame};
    use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

    /// The original two-pass encoder, kept as the correctness oracle.
    fn write_y4m_frame_two_pass(output: &mut Vec<u8>, frame: &VideoFrame) {
        output.extend_from_slice(b"FRAME\n");
        let format = frame.format();
        let width = usize::try_from(format.width()).unwrap_or(usize::MAX);
        let height = usize::try_from(format.height()).unwrap_or(usize::MAX);
        let mut luma = Vec::with_capacity(format.pixel_count());
        for pixel in frame.pixels().chunks_exact(4) {
            let (y, _, _) = rgb_to_yuv(pixel[0], pixel[1], pixel[2]);
            luma.push(y);
        }
        output.extend_from_slice(&luma);

        let mut chroma_u = Vec::with_capacity(format.pixel_count() / 4);
        let mut chroma_v = Vec::with_capacity(format.pixel_count() / 4);
        for block_y in (0..height).step_by(2) {
            for block_x in (0..width).step_by(2) {
                let mut u_sum = 0_u32;
                let mut v_sum = 0_u32;
                for y in block_y..block_y + 2 {
                    for x in block_x..block_x + 2 {
                        let offset = (y * width + x) * 4;
                        let (_, u, v) = rgb_to_yuv(
                            frame.pixels()[offset],
                            frame.pixels()[offset + 1],
                            frame.pixels()[offset + 2],
                        );
                        u_sum += u32::from(u);
                        v_sum += u32::from(v);
                    }
                }
                chroma_u.push(u8::try_from((u_sum + 2) / 4).unwrap_or(u8::MAX));
                chroma_v.push(u8::try_from((v_sum + 2) / 4).unwrap_or(u8::MAX));
            }
        }
        output.extend_from_slice(&chroma_u);
        output.extend_from_slice(&chroma_v);
    }

    #[test]
    fn single_pass_yuv_matches_the_two_pass_encoder() {
        for (width, height) in [(2_u32, 2_u32), (4, 2), (2, 4), (16, 10), (64, 36)] {
            let rate = FrameRate::new(30, 1).expect("valid rate");
            let format = VideoFormat::new(width, height, rate).expect("valid format");
            let pixels: Vec<u8> = (0..format.rgba_bytes())
                .map(|index| u8::try_from((index * 37) % 256).unwrap_or(0))
                .collect();
            let frame = VideoFrame::new(format, Timestamp::ZERO, pixels).expect("valid frame");

            let mut single = Vec::new();
            write_y4m_frame(&mut single, &frame);
            let mut two_pass = Vec::new();
            write_y4m_frame_two_pass(&mut two_pass, &frame);

            assert_eq!(single, two_pass, "{width}x{height}");
        }
    }
}
