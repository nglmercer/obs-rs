use std::io::Cursor;
use std::sync::Arc;

use obs_rs_media::{FrameRate, MediaError, Timestamp, VideoFormat, VideoFrame};

use super::{
    codec::{read_exact, read_u32, read_u64, read_vec, write_all, write_u32, write_u64},
    error::OutputError,
    types::OutputState,
    HEADER_BYTES, MAGIC, MAX_RECORDING_BYTES, MAX_RECORDING_FRAMES,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawRecording {
    pub(crate) format: VideoFormat,
    pub(crate) frames: Vec<VideoFrame>,
}

impl RawRecording {
    /// Creates an empty recording for one video format.
    #[must_use]
    pub fn new(format: VideoFormat) -> Self {
        Self {
            format,
            frames: Vec::new(),
        }
    }

    /// Adds a frame to the recording.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::FormatMismatch`] for another format or
    /// [`OutputError::TooManyFrames`] after the safety limit.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        if self.frames.len() >= MAX_RECORDING_FRAMES {
            return Err(OutputError::TooManyFrames {
                frames: self.frames.len() as u64 + 1,
            });
        }
        let frame_bytes = 16_usize.saturating_add(self.format.rgba_bytes());
        let encoded_bytes = HEADER_BYTES.saturating_add(
            self.frames
                .len()
                .saturating_add(1)
                .saturating_mul(frame_bytes),
        );
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        self.frames.push(frame);
        Ok(())
    }

    /// Returns the fixed recording format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the number of recorded frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns whether the recording has no frames.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Returns the recorded frames in timestamp order supplied by the caller.
    #[must_use]
    pub fn frames(&self) -> &[VideoFrame] {
        &self.frames
    }

    /// Encodes the recording into the deterministic raw reference format.
    ///
    /// The header stores the format and frame count. Each frame stores a nanosecond
    /// timestamp followed by its fixed-size RGBA payload.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Write`] only if the in-memory writer fails.
    pub fn encode(&self) -> Result<Vec<u8>, OutputError> {
        let mut bytes = Vec::with_capacity(
            HEADER_BYTES.saturating_add(
                self.frames
                    .len()
                    .saturating_mul(16 + self.format.rgba_bytes()),
            ),
        );
        write_all(&mut bytes, MAGIC)?;
        write_u32(&mut bytes, self.format.width())?;
        write_u32(&mut bytes, self.format.height())?;
        write_u32(&mut bytes, self.format.frame_rate().numerator())?;
        write_u32(&mut bytes, self.format.frame_rate().denominator())?;
        write_u64(&mut bytes, self.frames.len() as u64)?;
        for frame in &self.frames {
            write_u64(&mut bytes, frame.timestamp().as_nanos())?;
            write_u64(&mut bytes, frame.pixels().len() as u64)?;
            write_all(&mut bytes, frame.pixels())?;
        }
        Ok(bytes)
    }

    /// Decodes a complete raw reference recording.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidHeader`], [`OutputError::Truncated`],
    /// [`OutputError::TooManyFrames`], or [`OutputError::Media`] when the stream is
    /// malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self, OutputError> {
        let mut cursor = Cursor::new(bytes);
        let mut magic = [0_u8; 8];
        read_exact(&mut cursor, &mut magic)?;
        if &magic != MAGIC {
            return Err(OutputError::InvalidHeader);
        }

        let width = read_u32(&mut cursor)?;
        let height = read_u32(&mut cursor)?;
        let numerator = read_u32(&mut cursor)?;
        let denominator = read_u32(&mut cursor)?;
        let frame_count = read_u64(&mut cursor)?;
        if frame_count > MAX_RECORDING_FRAMES as u64 {
            return Err(OutputError::TooManyFrames {
                frames: frame_count,
            });
        }
        let rate = FrameRate::new(numerator, denominator).map_err(OutputError::Media)?;
        let format = VideoFormat::new(width, height, rate).map_err(OutputError::Media)?;
        let estimated_bytes = u128::from(frame_count)
            .saturating_mul(u128::from(format.rgba_bytes() as u64 + 16))
            .saturating_add(u128::from(HEADER_BYTES as u64));
        if estimated_bytes > u128::from(MAX_RECORDING_BYTES as u64) {
            return Err(OutputError::TooLarge {
                bytes: u64::try_from(estimated_bytes).unwrap_or(u64::MAX),
            });
        }
        let mut recording = Self::new(format);

        for _ in 0..frame_count {
            let timestamp = Timestamp::from_nanos(read_u64(&mut cursor)?);
            let payload_bytes = read_u64(&mut cursor)?;
            if payload_bytes != format.rgba_bytes() as u64 {
                return Err(OutputError::Media(MediaError::BufferSize {
                    expected: format.rgba_bytes(),
                    actual: usize::try_from(payload_bytes).unwrap_or(usize::MAX),
                }));
            }
            let pixels = read_vec(&mut cursor, format.rgba_bytes())?;
            let frame = VideoFrame::new(format, timestamp, pixels).map_err(OutputError::Media)?;
            recording.push(frame)?;
        }
        Ok(recording)
    }
}

/// A raw recording with explicit open/finalized/aborted lifecycle semantics.
pub struct RawRecordingSession {
    recording: RawRecording,
    state: OutputState,
    /// Committed bytes, shared rather than duplicated.
    ///
    /// `finalize` used to keep one copy and hand the caller another, doubling
    /// peak memory at exactly the moment the recording is largest.
    committed: Option<Arc<Vec<u8>>>,
}

impl RawRecordingSession {
    /// Starts an empty open session for one video format.
    #[must_use]
    pub fn new(format: VideoFormat) -> Self {
        Self {
            recording: RawRecording::new(format),
            state: OutputState::Open,
            committed: None,
        }
    }

    /// Appends one frame while the session is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// recording validation error for the frame itself.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.ensure_open("push a frame")?;
        self.recording.push(frame)
    }

    /// Encodes and atomically commits the complete recording.
    ///
    /// The session changes to [`OutputState::Finalized`] only after encoding
    /// succeeds, so a failed encode cannot expose a partially committed result.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the session is not open.
    pub fn finalize(&mut self) -> Result<Arc<Vec<u8>>, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = self.recording.encode()?;
        let bytes = Arc::new(bytes);
        self.committed = Some(Arc::clone(&bytes));
        self.state = OutputState::Finalized;
        Ok(bytes)
    }

    /// Aborts the session and discards all uncommitted frames.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the session is not open.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        self.recording.frames.clear();
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the in-progress or completed recording view.
    #[must_use]
    pub const fn recording(&self) -> &RawRecording {
        &self.recording
    }

    /// Returns committed bytes after finalization.
    #[must_use]
    pub fn committed_bytes(&self) -> Option<&[u8]> {
        self.committed.as_ref().map(|bytes| bytes.as_slice())
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}
