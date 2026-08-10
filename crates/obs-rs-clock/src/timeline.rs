use obs_rs_audio::{
    AudioDeadline, AudioFormat, AudioScheduler, AvSyncMetrics, AvSyncMonitor, AvSyncObservation,
};
use obs_rs_media::{FrameRate, Timestamp};
use obs_rs_video::{FrameDeadline, VideoScheduler};

use super::error::TimelineError;
/// A deterministic pair of rational media timelines with shared drift telemetry.
pub struct MediaTimeline {
    video: VideoScheduler,
    audio: AudioScheduler,
    sync: AvSyncMonitor,
}

impl MediaTimeline {
    /// Creates a timeline pair with an A/V classification tolerance in nanoseconds.
    #[must_use]
    pub fn new(video_rate: FrameRate, audio_format: AudioFormat, tolerance_nanos: u64) -> Self {
        Self {
            video: VideoScheduler::new(video_rate),
            audio: AudioScheduler::new(audio_format),
            sync: AvSyncMonitor::new(tolerance_nanos),
        }
    }

    /// Returns and advances the next video frame deadline.
    ///
    /// # Errors
    ///
    /// Returns [`TimelineError::Video`] if the integer video timeline overflows.
    pub fn next_video_frame(&mut self) -> Result<FrameDeadline, TimelineError> {
        self.video.next_deadline().map_err(TimelineError::Video)
    }

    /// Returns and advances the first deadline of the next audio block.
    ///
    /// # Errors
    ///
    /// Returns [`TimelineError::Audio`] for an empty block or timeline overflow.
    pub fn next_audio_block(&mut self, frames: usize) -> Result<AudioDeadline, TimelineError> {
        self.audio
            .next_block_deadline(frames)
            .map_err(TimelineError::Audio)
    }

    /// Records one cross-domain timestamp comparison.
    #[must_use]
    pub fn observe(
        &mut self,
        video_timestamp: Timestamp,
        audio_timestamp: Timestamp,
    ) -> AvSyncObservation {
        self.sync.observe(video_timestamp, audio_timestamp)
    }

    /// Returns accumulated synchronization diagnostics.
    #[must_use]
    pub const fn metrics(&self) -> AvSyncMetrics {
        self.sync.metrics()
    }

    /// Returns the configured video frame rate.
    #[must_use]
    pub const fn video_rate(&self) -> FrameRate {
        self.video.frame_rate()
    }

    /// Returns the configured audio format.
    #[must_use]
    pub const fn audio_format(&self) -> AudioFormat {
        self.audio.format()
    }

    /// Resets both schedules and accumulated drift diagnostics.
    pub fn reset(&mut self) {
        self.video.reset();
        self.audio.reset();
        self.sync.reset();
    }
}
