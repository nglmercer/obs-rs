use super::{format::VideoFormat, frame::VideoFrame, time::Timestamp};
use std::{collections::VecDeque, fmt};

/// Smallest supported OBS Render Delay value, in milliseconds.
pub const MIN_RENDER_DELAY_MILLISECONDS: u32 = 0;
/// Largest supported OBS Render Delay value, in milliseconds.
pub const MAX_RENDER_DELAY_MILLISECONDS: u32 = 500;

/// Maximum number of frame references retained by the CPU Render Delay oracle.
///
/// The production WGPU filter can retain device textures, but the portable
/// source runtime owns RGBA frames. Keeping both a frame-count and a byte cap
/// makes an accidentally large delay incapable of becoming an unbounded
/// allocation path.
pub const MAX_RENDER_DELAY_HISTORY_FRAMES: usize = 32;
/// Maximum pixel storage reserved by one CPU Render Delay history.
pub const MAX_RENDER_DELAY_HISTORY_BYTES: usize = 256 * 1_024 * 1_024;

const TIMESTAMP_RESET_GAP_NANOS: u64 = 1_000_000_000;

/// A bounded Render Delay history failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderDelayError {
    /// The filter value is outside the OBS-compatible property range.
    DelayOutOfRange { milliseconds: u32, maximum: u32 },
    /// The requested delay would retain more frame slots than the CPU oracle
    /// permits, even though the user-facing setting itself is bounded.
    FrameCapacity { required: usize, maximum: usize },
    /// The requested delay would retain more RGBA bytes than the CPU oracle
    /// permits for this source format.
    MemoryCapacity { required: usize, maximum: usize },
}

impl fmt::Display for RenderDelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DelayOutOfRange {
                milliseconds,
                maximum,
            } => write!(
                formatter,
                "render delay {milliseconds} ms is outside 0..={maximum} ms"
            ),
            Self::FrameCapacity { required, maximum } => write!(
                formatter,
                "render delay needs {required} frame slots; CPU limit is {maximum}"
            ),
            Self::MemoryCapacity { required, maximum } => write!(
                formatter,
                "render delay needs {required} bytes; CPU limit is {maximum}"
            ),
        }
    }
}

/// Rust-owned, timestamped frame history for the portable Render Delay path.
///
/// The queue emits the oldest frame once its timestamp is at least the
/// configured delay behind the newest request. During warm-up it returns
/// `None`, matching OBS's delayed visual path rather than displaying the
/// current frame early. A backwards timestamp or a gap larger than one second
/// resets the history so a restarted capture cannot mix two timelines.
#[derive(Debug)]
pub struct RenderDelayBuffer {
    milliseconds: u32,
    frames: VecDeque<VideoFrame>,
    bytes: usize,
    format: Option<VideoFormat>,
    last_timestamp: Option<Timestamp>,
}

impl RenderDelayBuffer {
    /// Creates an empty history with no delay.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            milliseconds: MIN_RENDER_DELAY_MILLISECONDS,
            frames: VecDeque::new(),
            bytes: 0,
            format: None,
            last_timestamp: None,
        }
    }

    /// Returns the configured delay in milliseconds.
    #[must_use]
    pub const fn milliseconds(&self) -> u32 {
        self.milliseconds
    }

    /// Returns the number of retained frame references.
    #[must_use]
    pub fn buffered_frames(&self) -> usize {
        self.frames.len()
    }

    /// Clears the timeline while retaining the configured delay value.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
        self.format = None;
        self.last_timestamp = None;
    }

    /// Changes the delay and resets old frames so a setting edit cannot leak
    /// stale pixels into the new timeline.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDelayError::DelayOutOfRange`] when the value exceeds
    /// the portable OBS Render Delay property range.
    pub fn set_milliseconds(&mut self, milliseconds: u32) -> Result<(), RenderDelayError> {
        if !(MIN_RENDER_DELAY_MILLISECONDS..=MAX_RENDER_DELAY_MILLISECONDS).contains(&milliseconds)
        {
            return Err(RenderDelayError::DelayOutOfRange {
                milliseconds,
                maximum: MAX_RENDER_DELAY_MILLISECONDS,
            });
        }
        if self.milliseconds != milliseconds {
            self.milliseconds = milliseconds;
            self.clear();
        }
        Ok(())
    }

    /// Pushes one source frame and returns the frame that is ready to present.
    ///
    /// The returned frame carries the newest request timestamp while sharing
    /// the delayed pixel storage. `None` is the bounded warm-up state.
    ///
    /// # Errors
    ///
    /// Returns a capacity error when this source format cannot retain the
    /// requested history inside the frame-count and byte budgets.
    pub fn push(&mut self, frame: VideoFrame) -> Result<Option<VideoFrame>, RenderDelayError> {
        if self.milliseconds == MIN_RENDER_DELAY_MILLISECONDS {
            self.clear();
            return Ok(Some(frame));
        }

        let format = frame.format();
        if self.format != Some(format) {
            self.clear();
            self.format = Some(format);
        }

        let required_frames = self.required_frames(format);
        if required_frames > MAX_RENDER_DELAY_HISTORY_FRAMES {
            return Err(RenderDelayError::FrameCapacity {
                required: required_frames,
                maximum: MAX_RENDER_DELAY_HISTORY_FRAMES,
            });
        }
        let required_bytes = format.rgba_bytes().saturating_mul(required_frames);
        if required_bytes > MAX_RENDER_DELAY_HISTORY_BYTES {
            return Err(RenderDelayError::MemoryCapacity {
                required: required_bytes,
                maximum: MAX_RENDER_DELAY_HISTORY_BYTES,
            });
        }
        let frame_bytes = format.rgba_bytes();
        if self.frames.len() >= MAX_RENDER_DELAY_HISTORY_FRAMES {
            return Err(RenderDelayError::FrameCapacity {
                required: self.frames.len().saturating_add(1),
                maximum: MAX_RENDER_DELAY_HISTORY_FRAMES,
            });
        }
        if self.bytes > MAX_RENDER_DELAY_HISTORY_BYTES.saturating_sub(frame_bytes) {
            return Err(RenderDelayError::MemoryCapacity {
                required: self.bytes.saturating_add(frame_bytes),
                maximum: MAX_RENDER_DELAY_HISTORY_BYTES,
            });
        }

        let timestamp = frame.timestamp();
        if self.last_timestamp.is_some_and(|previous| {
            timestamp < previous
                || timestamp.as_nanos().saturating_sub(previous.as_nanos())
                    > TIMESTAMP_RESET_GAP_NANOS
        }) {
            self.frames.clear();
            self.bytes = 0;
        }
        self.last_timestamp = Some(timestamp);

        self.bytes = self.bytes.saturating_add(frame_bytes);
        self.frames.push_back(frame);
        let delay_nanos = u64::from(self.milliseconds).saturating_mul(1_000_000);
        let ready = self.frames.front().is_some_and(|candidate| {
            timestamp
                .as_nanos()
                .saturating_sub(candidate.timestamp().as_nanos())
                >= delay_nanos
        });
        if !ready {
            return Ok(None);
        }

        let Some(delayed) = self.frames.pop_front() else {
            return Ok(None);
        };
        self.bytes = self.bytes.saturating_sub(frame_bytes);
        Ok(Some(delayed.at_timestamp(timestamp)))
    }

    fn required_frames(&self, format: VideoFormat) -> usize {
        let delay_nanos = u64::from(self.milliseconds).saturating_mul(1_000_000);
        let period = format.frame_rate().period_nanos().unwrap_or(1).max(1);
        let delayed_frames = delay_nanos.saturating_add(period.saturating_sub(1)) / period;
        usize::try_from(delayed_frames.saturating_add(1)).unwrap_or(usize::MAX)
    }
}

impl Default for RenderDelayBuffer {
    fn default() -> Self {
        Self::new()
    }
}
