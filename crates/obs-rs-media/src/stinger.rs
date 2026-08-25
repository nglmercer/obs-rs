//! Bounded, preloaded Stinger transition clips.

use std::sync::Arc;

use super::{error::MediaError, format::VideoFormat, frame::VideoFrame, time::Timestamp};

/// Maximum number of decoded frames retained by one Stinger clip.
pub const MAX_STINGER_FRAMES: usize = 256;
/// Maximum decoded RGBA storage retained by one Stinger clip.
pub const MAX_STINGER_MEMORY_BYTES: usize = 256 * 1_024 * 1_024;
/// Smallest accepted decoded Stinger frame duration.
pub const MIN_STINGER_FRAME_DURATION_NANOS: u64 = 1_000_000;
/// Largest accepted duration for one decoded Stinger frame.
pub const MAX_STINGER_FRAME_DURATION_NANOS: u64 = 60_000_000_000;
/// Maximum total playback duration of one Stinger clip.
pub const MAX_STINGER_DURATION_NANOS: u64 = 120_000_000_000;
/// Smallest safe interior transition point accepted by the portable model.
pub const MIN_STINGER_TRANSITION_POINT_MILLI: u16 = 1;
/// Largest safe interior transition point accepted by the portable model.
pub const MAX_STINGER_TRANSITION_POINT_MILLI: u16 = 999;

/// One preloaded Stinger frame and its playback duration.
#[derive(Clone, Debug, Eq, PartialEq)]
struct StingerFrame {
    frame: VideoFrame,
    duration_nanos: u64,
}

/// A validated, preloaded Stinger clip.
///
/// Resource decoding deliberately happens outside this type. The clip only
/// accepts already-decoded RGBA frames, so rendering never performs file or
/// decoder I/O and the retained memory has an explicit bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StingerClip {
    format: VideoFormat,
    frames: Arc<Vec<StingerFrame>>,
    duration_nanos: u64,
    transition_point_milli: u16,
}

impl StingerClip {
    /// Builds a bounded clip from decoded frames and per-frame durations.
    ///
    /// # Errors
    ///
    /// Returns a typed media error when the frame count, dimensions, duration,
    /// transition point, or resident memory exceeds the portable limits.
    pub fn new(
        frames: Vec<VideoFrame>,
        frame_durations_nanos: Vec<u64>,
        transition_point_milli: u16,
    ) -> Result<Self, MediaError> {
        let frame_count = frames.len();
        if !(1..=MAX_STINGER_FRAMES).contains(&frame_count) {
            return Err(MediaError::InvalidStingerFrameCount { count: frame_count });
        }
        if frame_durations_nanos.len() != frame_count {
            return Err(MediaError::InvalidStingerFrameDurations {
                expected: frame_count,
                actual: frame_durations_nanos.len(),
            });
        }
        if !(MIN_STINGER_TRANSITION_POINT_MILLI..=MAX_STINGER_TRANSITION_POINT_MILLI)
            .contains(&transition_point_milli)
        {
            return Err(MediaError::InvalidStingerTransitionPoint {
                transition_point_milli,
            });
        }

        let format = frames[0].format();
        let mut resident_bytes = 0_usize;
        let mut duration_nanos = 0_u64;
        let mut retained = Vec::with_capacity(frame_count);
        for (frame, frame_duration_nanos) in frames.into_iter().zip(frame_durations_nanos) {
            if frame.format() != format {
                return Err(MediaError::FormatMismatch {
                    expected: format,
                    actual: frame.format(),
                });
            }
            if !(MIN_STINGER_FRAME_DURATION_NANOS..=MAX_STINGER_FRAME_DURATION_NANOS)
                .contains(&frame_duration_nanos)
            {
                return Err(MediaError::InvalidStingerFrameDuration {
                    duration_nanos: frame_duration_nanos,
                });
            }
            resident_bytes = resident_bytes.saturating_add(frame.pixels().len());
            if resident_bytes > MAX_STINGER_MEMORY_BYTES {
                return Err(MediaError::StingerTooLarge {
                    bytes: resident_bytes,
                });
            }
            duration_nanos = duration_nanos.saturating_add(frame_duration_nanos);
            if duration_nanos > MAX_STINGER_DURATION_NANOS {
                return Err(MediaError::StingerDurationTooLong { duration_nanos });
            }
            retained.push(StingerFrame {
                frame,
                duration_nanos: frame_duration_nanos,
            });
        }

        Ok(Self {
            format,
            frames: Arc::new(retained),
            duration_nanos,
            transition_point_milli,
        })
    }

    /// Returns the clip's frame format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the number of preloaded frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Returns the total bounded playback duration.
    #[must_use]
    pub const fn duration_nanos(&self) -> u64 {
        self.duration_nanos
    }

    /// Returns the normalized point at which the destination scene is shown.
    #[must_use]
    pub const fn transition_point_milli(&self) -> u16 {
        self.transition_point_milli
    }

    /// Returns whether the destination scene should be under the overlay.
    #[must_use]
    pub const fn destination_visible(&self, progress_milli: u16) -> bool {
        progress_milli >= self.transition_point_milli
    }

    /// Selects the decoded overlay frame for a bounded transition progress.
    ///
    /// The returned frame shares the clip's immutable pixels and only changes
    /// its presentation timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] for progress above 1000.
    pub fn frame_at_progress(
        &self,
        progress_milli: u16,
        timestamp: Timestamp,
    ) -> Result<VideoFrame, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        let position = if progress_milli == 1_000 {
            self.duration_nanos.saturating_sub(1)
        } else {
            self.duration_nanos
                .saturating_mul(u64::from(progress_milli))
                / 1_000
        };
        let mut remaining = position;
        let selected = self
            .frames
            .iter()
            .find(|frame| {
                if remaining < frame.duration_nanos {
                    true
                } else {
                    remaining = remaining.saturating_sub(frame.duration_nanos);
                    false
                }
            })
            .or_else(|| self.frames.last())
            .ok_or(MediaError::InvalidStingerFrameCount { count: 0 })?;
        Ok(selected.frame.at_timestamp(timestamp))
    }

    /// Renders the clip over the source/destination scene pair.
    ///
    /// The scene cut occurs at the configured transition point, while the
    /// decoded Stinger frame remains an alpha-composited overlay throughout.
    /// Before the cut, the source storage is shared and copied only when the
    /// overlay actually needs to modify it; after the cut, the destination
    /// buffer remains the output storage.
    ///
    /// # Errors
    ///
    /// Returns an error when the scene formats differ, the overlay progress is
    /// outside the normalized range, or compositing rejects the frame pair.
    pub fn render(
        &self,
        source: &VideoFrame,
        destination: VideoFrame,
        progress_milli: u16,
    ) -> Result<VideoFrame, MediaError> {
        let overlay = self.frame_at_progress(progress_milli, destination.timestamp())?;
        self.render_with_overlay(source, destination, &overlay, progress_milli)
    }

    /// Renders a caller-scaled overlay frame over the scene pair.
    ///
    /// Preview and program consumers may use different resolutions. Their
    /// bounded presentation scalers can prepare the selected clip frame once
    /// for the target before this method performs the alpha blend.
    ///
    /// # Errors
    ///
    /// Returns an error when the scene and overlay formats differ, the overlay
    /// progress is outside the normalized range, or compositing rejects the
    /// frame pair.
    pub fn render_with_overlay(
        &self,
        source: &VideoFrame,
        destination: VideoFrame,
        overlay: &VideoFrame,
        progress_milli: u16,
    ) -> Result<VideoFrame, MediaError> {
        if source.format() != destination.format() {
            return Err(MediaError::FormatMismatch {
                expected: source.format(),
                actual: destination.format(),
            });
        }
        if overlay.format() != destination.format() {
            return Err(MediaError::FormatMismatch {
                expected: destination.format(),
                actual: overlay.format(),
            });
        }
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        let timestamp = destination.timestamp();
        let mut output = if self.destination_visible(progress_milli) {
            destination
        } else {
            source.at_timestamp(timestamp)
        };
        output.blend_over(overlay)?;
        Ok(output)
    }
}
