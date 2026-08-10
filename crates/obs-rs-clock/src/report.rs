/// Aggregate diagnostics from a coordinated audio/video session run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaSessionReport {
    pub(crate) requested_ticks: u64,
    pub(crate) completed_ticks: u64,
    pub(crate) cancelled: bool,
    pub(crate) audio_blocks: u64,
    pub(crate) video_frames: u64,
    pub(crate) audio_underflow_blocks: u64,
    pub(crate) video_empty_frames: u64,
    pub(crate) audio_dropped_oldest_frames: u64,
    pub(crate) audio_dropped_newest_frames: u64,
    pub(crate) video_dropped_oldest: u64,
    pub(crate) video_dropped_newest: u64,
    pub(crate) audio_missed_deadlines: u64,
    pub(crate) video_missed_deadlines: u64,
    pub(crate) audio_lateness_nanos: u64,
    pub(crate) video_lateness_nanos: u64,
    pub(crate) video_wait_nanos: u64,
    pub(crate) video_render_nanos: u64,
    pub(crate) video_max_lateness_nanos: u64,
}

impl MediaSessionReport {
    /// Returns the number of coordinated ticks requested.
    #[must_use]
    pub const fn requested_ticks(self) -> u64 {
        self.requested_ticks
    }

    /// Returns the number of ticks that completed both audio and video work.
    #[must_use]
    pub const fn completed_ticks(self) -> u64 {
        self.completed_ticks
    }

    /// Returns whether cancellation stopped the run early.
    #[must_use]
    pub const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// Returns completed audio blocks.
    #[must_use]
    pub const fn audio_blocks(self) -> u64 {
        self.audio_blocks
    }

    /// Returns completed video frames.
    #[must_use]
    pub const fn video_frames(self) -> u64 {
        self.video_frames
    }

    /// Returns audio producer underflow blocks.
    #[must_use]
    pub const fn audio_underflow_blocks(self) -> u64 {
        self.audio_underflow_blocks
    }

    /// Returns video callbacks that produced no frame.
    #[must_use]
    pub const fn video_empty_frames(self) -> u64 {
        self.video_empty_frames
    }

    /// Returns audio frames removed under drop-oldest pressure.
    #[must_use]
    pub const fn audio_dropped_oldest_frames(self) -> u64 {
        self.audio_dropped_oldest_frames
    }

    /// Returns audio frames discarded under drop-newest pressure.
    #[must_use]
    pub const fn audio_dropped_newest_frames(self) -> u64 {
        self.audio_dropped_newest_frames
    }

    /// Returns video frames removed under drop-oldest pressure.
    #[must_use]
    pub const fn video_dropped_oldest(self) -> u64 {
        self.video_dropped_oldest
    }

    /// Returns video frames discarded under drop-newest pressure.
    #[must_use]
    pub const fn video_dropped_newest(self) -> u64 {
        self.video_dropped_newest
    }

    /// Returns late audio block observations.
    #[must_use]
    pub const fn audio_missed_deadlines(self) -> u64 {
        self.audio_missed_deadlines
    }

    /// Returns late video frame observations.
    #[must_use]
    pub const fn video_missed_deadlines(self) -> u64 {
        self.video_missed_deadlines
    }

    /// Returns total audio post-callback lateness.
    #[must_use]
    pub const fn audio_lateness_nanos(self) -> u64 {
        self.audio_lateness_nanos
    }

    /// Returns total video post-render lateness.
    #[must_use]
    pub const fn video_lateness_nanos(self) -> u64 {
        self.video_lateness_nanos
    }

    /// Returns total video time spent waiting for deadlines.
    #[must_use]
    pub const fn video_wait_nanos(self) -> u64 {
        self.video_wait_nanos
    }

    /// Returns total video time spent inside render callbacks.
    #[must_use]
    pub const fn video_render_nanos(self) -> u64 {
        self.video_render_nanos
    }

    /// Returns the greatest video lateness observed during the run.
    #[must_use]
    pub const fn video_max_lateness_nanos(self) -> u64 {
        self.video_max_lateness_nanos
    }
}
