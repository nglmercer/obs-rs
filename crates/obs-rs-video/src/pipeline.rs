use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    clock::{DeadlineObservation, FrameDeadline, VideoScheduler},
    error::{RenderError, VideoError},
    queue::FrameQueue,
    types::{DropPolicy, PushOutcome},
};
/// The current reference video transport: a scheduler feeding a bounded queue.
pub struct VideoPipeline {
    pub(crate) scheduler: VideoScheduler,
    pub(crate) queue: FrameQueue,
    pub(crate) metrics: VideoMetrics,
    /// RGBA bytes in one frame of the pipeline format.
    ///
    /// Constant for the pipeline's lifetime, so it is resolved once here rather
    /// than recomputed from the queue format on every rendered frame.
    pub(crate) bytes_per_frame: u64,
}

/// Counters collected by the reference render loop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VideoMetrics {
    pub(crate) render_calls: u64,
    pub(crate) produced_frames: u64,
    pub(crate) produced_bytes: u64,
    pub(crate) peak_queued_bytes: u64,
    pub(crate) empty_frames: u64,
    pub(crate) dropped_oldest: u64,
    pub(crate) dropped_newest: u64,
    pub(crate) missed_deadlines: u64,
    pub(crate) total_lateness_nanos: u64,
}

/// A delta report from a bounded, output-draining sustained render run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SustainedRunReport {
    requested_frames: u64,
    produced_frames: u64,
    empty_frames: u64,
    dropped_oldest: u64,
    dropped_newest: u64,
    remaining_queue: usize,
}

impl SustainedRunReport {
    /// Returns the number of scheduled render callbacks.
    #[must_use]
    pub const fn requested_frames(self) -> u64 {
        self.requested_frames
    }

    /// Returns the number of callbacks that produced a frame.
    #[must_use]
    pub const fn produced_frames(self) -> u64 {
        self.produced_frames
    }

    /// Returns the number of callbacks that produced no frame.
    #[must_use]
    pub const fn empty_frames(self) -> u64 {
        self.empty_frames
    }

    /// Returns the number of frames dropped from the queue's oldest end.
    #[must_use]
    pub const fn dropped_oldest(self) -> u64 {
        self.dropped_oldest
    }

    /// Returns the number of newly produced frames dropped at the newest end.
    #[must_use]
    pub const fn dropped_newest(self) -> u64 {
        self.dropped_newest
    }

    /// Returns the number of frames left for the output consumer.
    #[must_use]
    pub const fn remaining_queue(self) -> usize {
        self.remaining_queue
    }
}

impl VideoMetrics {
    /// Number of render callbacks requested.
    #[must_use]
    pub const fn render_calls(self) -> u64 {
        self.render_calls
    }

    /// Number of callbacks that returned a frame.
    #[must_use]
    pub const fn produced_frames(self) -> u64 {
        self.produced_frames
    }

    /// Number of owned pixel bytes submitted by render callbacks.
    #[must_use]
    pub const fn produced_bytes(self) -> u64 {
        self.produced_bytes
    }

    /// Largest estimated queued pixel footprint observed by this pipeline.
    #[must_use]
    pub const fn peak_queued_bytes(self) -> u64 {
        self.peak_queued_bytes
    }

    /// Number of callbacks that returned no frame.
    #[must_use]
    pub const fn empty_frames(self) -> u64 {
        self.empty_frames
    }

    /// Number of frames discarded from the queue's oldest end.
    #[must_use]
    pub const fn dropped_oldest(self) -> u64 {
        self.dropped_oldest
    }

    /// Number of newly submitted frames discarded at the queue's newest end.
    #[must_use]
    pub const fn dropped_newest(self) -> u64 {
        self.dropped_newest
    }

    /// Number of observed deadlines that were late.
    #[must_use]
    pub const fn missed_deadlines(self) -> u64 {
        self.missed_deadlines
    }

    /// Sum of observed lateness, saturating at `u64::MAX`.
    #[must_use]
    pub const fn total_lateness_nanos(self) -> u64 {
        self.total_lateness_nanos
    }
}

/// Result of one callback-driven render step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderOutcome {
    /// The source produced no frame for this deadline.
    Empty { deadline: FrameDeadline },
    /// The produced frame entered the queue.
    Enqueued { deadline: FrameDeadline },
    /// The oldest queued frame was removed to make room.
    DroppedOldest {
        deadline: FrameDeadline,
        dropped: Timestamp,
    },
    /// The newly produced frame was discarded.
    DroppedNewest {
        deadline: FrameDeadline,
        dropped: Timestamp,
    },
}
impl VideoPipeline {
    /// Creates a pipeline for one video format.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ZeroCapacity`] when the queue capacity is zero.
    pub fn new(
        format: VideoFormat,
        capacity: usize,
        policy: DropPolicy,
    ) -> Result<Self, VideoError> {
        Ok(Self {
            scheduler: VideoScheduler::new(format.frame_rate()),
            queue: FrameQueue::new(format, capacity, policy)?,
            metrics: VideoMetrics::default(),
            bytes_per_frame: u64::try_from(format.rgba_bytes()).unwrap_or(u64::MAX),
        })
    }

    /// Returns and advances the next output deadline.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ScheduleOverflow`] after the integer timeline is
    /// exhausted.
    pub fn next_deadline(&mut self) -> Result<FrameDeadline, VideoError> {
        self.scheduler.next_deadline()
    }

    /// Runs one callback at a caller-provided deadline and submits its frame.
    ///
    /// This is used by [`crate::VideoWorker`] after wall-clock pacing. Unlike
    /// [`Self::render_next`], it does not advance the internal scheduler.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Source`] when the callback fails or
    /// [`RenderError::Submit`] when its frame has the wrong format.
    pub fn render_at<E, F>(
        &mut self,
        deadline: FrameDeadline,
        render: F,
    ) -> Result<RenderOutcome, RenderError<E>>
    where
        F: FnOnce(FrameDeadline, VideoFormat) -> Result<Option<VideoFrame>, E>,
    {
        self.metrics.render_calls = self.metrics.render_calls.saturating_add(1);
        // Hoisted: the queue format is fixed for the pipeline, so it is read
        // once instead of three times per rendered frame.
        let format = self.queue.format();
        let frame = render(deadline, format).map_err(RenderError::Source)?;
        let Some(frame) = frame else {
            self.metrics.empty_frames = self.metrics.empty_frames.saturating_add(1);
            return Ok(RenderOutcome::Empty { deadline });
        };

        self.metrics.produced_frames = self.metrics.produced_frames.saturating_add(1);
        // Every queued frame carries exactly one frame's worth of RGBA bytes,
        // which the pipeline already knows; no length or format lookup needed.
        self.metrics.produced_bytes = self
            .metrics
            .produced_bytes
            .saturating_add(self.bytes_per_frame);
        let outcome = self.queue.push(frame).map_err(RenderError::Submit)?;
        let queued_bytes = u64::try_from(self.queue.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(self.bytes_per_frame);
        self.metrics.peak_queued_bytes = self.metrics.peak_queued_bytes.max(queued_bytes);
        match outcome {
            PushOutcome::Enqueued => Ok(RenderOutcome::Enqueued { deadline }),
            PushOutcome::DroppedOldest(dropped) => {
                self.metrics.dropped_oldest = self.metrics.dropped_oldest.saturating_add(1);
                Ok(RenderOutcome::DroppedOldest { deadline, dropped })
            }
            PushOutcome::DroppedNewest(dropped) => {
                self.metrics.dropped_newest = self.metrics.dropped_newest.saturating_add(1);
                Ok(RenderOutcome::DroppedNewest { deadline, dropped })
            }
        }
    }

    /// Runs one source callback at the next rational deadline and submits its frame.
    ///
    /// The callback receives the deadline and the pipeline's configured format. A
    /// missing frame is counted but does not change queue state.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Schedule`] when the timeline overflows,
    /// [`RenderError::Source`] when the callback fails, or [`RenderError::Submit`]
    /// when its frame has the wrong format.
    pub fn render_next<E, F>(&mut self, render: F) -> Result<RenderOutcome, RenderError<E>>
    where
        F: FnOnce(FrameDeadline, VideoFormat) -> Result<Option<VideoFrame>, E>,
    {
        let deadline = self
            .scheduler
            .next_deadline()
            .map_err(RenderError::Schedule)?;
        self.render_at(deadline, render)
    }

    /// Runs a bounded sustained render fixture while consuming one output frame
    /// after each callback.
    ///
    /// This models the normal producer/consumer relationship and keeps the
    /// fixture deterministic: the caller controls the frame count and callback,
    /// while the returned report contains only counter deltas from this run.
    ///
    /// # Errors
    ///
    /// Returns the first scheduling, source, or submission error.
    pub fn run_sustained<E, F>(
        &mut self,
        frame_count: u64,
        mut render: F,
    ) -> Result<SustainedRunReport, RenderError<E>>
    where
        F: FnMut(FrameDeadline, VideoFormat) -> Result<Option<VideoFrame>, E>,
    {
        let before = self.metrics;
        for _ in 0..frame_count {
            self.render_next(&mut render)?;
            let _ = self.take_next();
        }
        let after = self.metrics;
        Ok(SustainedRunReport {
            requested_frames: frame_count,
            produced_frames: after.produced_frames.saturating_sub(before.produced_frames),
            empty_frames: after.empty_frames.saturating_sub(before.empty_frames),
            dropped_oldest: after.dropped_oldest.saturating_sub(before.dropped_oldest),
            dropped_newest: after.dropped_newest.saturating_sub(before.dropped_newest),
            remaining_queue: self.queued(),
        })
    }

    /// Submits a rendered frame to the bounded transport.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::FormatMismatch`] for a frame in the wrong format.
    pub fn submit(&mut self, frame: VideoFrame) -> Result<PushOutcome, VideoError> {
        self.queue.push(frame)
    }

    /// Takes the oldest frame ready for output.
    pub fn take_next(&mut self) -> Option<VideoFrame> {
        self.queue.pop()
    }

    /// Returns the number of frames waiting for output.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Records an observed output time against a scheduled deadline.
    #[must_use]
    pub fn observe_deadline(
        &mut self,
        deadline: FrameDeadline,
        observed: Timestamp,
    ) -> DeadlineObservation {
        let observation = DeadlineObservation::new(deadline, observed);
        if observation.missed() {
            self.metrics.missed_deadlines = self.metrics.missed_deadlines.saturating_add(1);
            self.metrics.total_lateness_nanos = self
                .metrics
                .total_lateness_nanos
                .saturating_add(observation.lateness_nanos());
        }
        observation
    }

    /// Returns the pipeline's configured output format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.queue.format()
    }

    /// Returns a snapshot of render/drop counters.
    #[must_use]
    pub const fn metrics(&self) -> VideoMetrics {
        self.metrics
    }

    /// Clears render/drop counters without changing queued frames or the clock.
    pub fn reset_metrics(&mut self) {
        self.metrics = VideoMetrics::default();
    }
}
