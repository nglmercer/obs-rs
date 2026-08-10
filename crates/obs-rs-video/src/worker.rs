use obs_rs_media::{VideoFormat, VideoFrame};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::{
    clock::{FrameDeadline, VideoClock, VideoPacer},
    error::{VideoError, WorkerError},
    pipeline::{VideoMetrics, VideoPipeline},
    types::DropPolicy,
};
/// A thread-safe cancellation flag checked between video frames.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    /// Creates an uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation before the next frame begins.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Clears a previous cancellation request for reuse.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Delta diagnostics from one paced worker run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VideoWorkerReport {
    requested_frames: u64,
    processed_frames: u64,
    produced_bytes: u64,
    peak_queued_bytes: u64,
    cancelled: bool,
    empty_frames: u64,
    dropped_oldest: u64,
    dropped_newest: u64,
    missed_deadlines: u64,
    total_lateness_nanos: u64,
    total_wait_nanos: u64,
    total_render_nanos: u64,
    max_lateness_nanos: u64,
    remaining_queue: usize,
}

impl VideoWorkerReport {
    /// Returns the number of frames requested from the worker.
    #[must_use]
    pub const fn requested_frames(self) -> u64 {
        self.requested_frames
    }

    /// Returns the number of paced render callbacks completed.
    #[must_use]
    pub const fn processed_frames(self) -> u64 {
        self.processed_frames
    }

    /// Number of owned pixel bytes produced by this run.
    #[must_use]
    pub const fn produced_bytes(self) -> u64 {
        self.produced_bytes
    }

    /// Largest estimated queued pixel footprint observed by this run.
    #[must_use]
    pub const fn peak_queued_bytes(self) -> u64 {
        self.peak_queued_bytes
    }

    /// Returns whether cancellation stopped the run before its requested count.
    #[must_use]
    pub const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// Returns the number of callbacks that produced no frame.
    #[must_use]
    pub const fn empty_frames(self) -> u64 {
        self.empty_frames
    }

    /// Returns the number of frames dropped from the oldest queue end.
    #[must_use]
    pub const fn dropped_oldest(self) -> u64 {
        self.dropped_oldest
    }

    /// Returns the number of newly produced frames dropped at the newest queue end.
    #[must_use]
    pub const fn dropped_newest(self) -> u64 {
        self.dropped_newest
    }

    /// Returns the number of post-render deadlines observed late.
    #[must_use]
    pub const fn missed_deadlines(self) -> u64 {
        self.missed_deadlines
    }

    /// Returns the total post-render lateness in nanoseconds.
    #[must_use]
    pub const fn total_lateness_nanos(self) -> u64 {
        self.total_lateness_nanos
    }

    /// Returns total time spent waiting for frame deadlines.
    #[must_use]
    pub const fn total_wait_nanos(self) -> u64 {
        self.total_wait_nanos
    }

    /// Returns total time spent inside render callbacks.
    #[must_use]
    pub const fn total_render_nanos(self) -> u64 {
        self.total_render_nanos
    }

    /// Returns the largest post-render lateness observed in this run.
    #[must_use]
    pub const fn max_lateness_nanos(self) -> u64 {
        self.max_lateness_nanos
    }

    /// Returns the number of frames left in the output queue.
    #[must_use]
    pub const fn remaining_queue(self) -> usize {
        self.remaining_queue
    }
}

/// A paced, cancellation-aware video producer over a bounded output pipeline.
pub struct VideoWorker {
    pacer: VideoPacer,
    pipeline: VideoPipeline,
}

impl VideoWorker {
    /// Creates a worker with an injected-clock-compatible video pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ZeroCapacity`] when the output queue capacity is zero.
    pub fn new(
        format: VideoFormat,
        capacity: usize,
        policy: DropPolicy,
    ) -> Result<Self, VideoError> {
        Ok(Self {
            pacer: VideoPacer::new(format.frame_rate()),
            pipeline: VideoPipeline::new(format, capacity, policy)?,
        })
    }

    /// Runs up to `frame_count` paced frames and drains one output after each.
    ///
    /// The callback is invoked only after the clock reaches the frame deadline.
    /// Cancellation is checked before each pacing operation; a callback may call
    /// [`CancellationToken::cancel`] to stop before the next frame.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::Pacing`] for timeline/clock failures or
    /// [`WorkerError::Render`] for callback and frame-contract failures.
    pub fn run<C, E, F>(
        &mut self,
        clock: &mut C,
        frame_count: u64,
        cancellation: &CancellationToken,
        mut render: F,
    ) -> Result<VideoWorkerReport, WorkerError<E>>
    where
        C: VideoClock,
        F: FnMut(FrameDeadline, VideoFormat) -> Result<Option<VideoFrame>, E>,
    {
        let before = self.pipeline.metrics;
        let mut processed_frames = 0_u64;
        let mut total_wait_nanos = 0_u64;
        let mut total_render_nanos = 0_u64;
        let mut max_lateness_nanos = 0_u64;
        for _ in 0..frame_count {
            if cancellation.is_cancelled() {
                break;
            }
            let pacing = self.pacer.next(clock).map_err(WorkerError::Pacing)?;
            total_wait_nanos = total_wait_nanos.saturating_add(pacing.waited_nanos());
            let render_started = clock.now();
            self.pipeline
                .render_at(pacing.deadline(), &mut render)
                .map_err(WorkerError::Render)?;
            let observed_at = clock.now();
            total_render_nanos = total_render_nanos.saturating_add(
                observed_at
                    .as_nanos()
                    .saturating_sub(render_started.as_nanos()),
            );
            let observation = self
                .pipeline
                .observe_deadline(pacing.deadline(), observed_at);
            max_lateness_nanos = max_lateness_nanos.max(observation.lateness_nanos());
            let _ = self.pipeline.take_next();
            processed_frames = processed_frames.saturating_add(1);
        }
        let after = self.pipeline.metrics;
        Ok(VideoWorkerReport {
            requested_frames: frame_count,
            processed_frames,
            produced_bytes: after.produced_bytes.saturating_sub(before.produced_bytes),
            peak_queued_bytes: after.peak_queued_bytes,
            cancelled: cancellation.is_cancelled() && processed_frames < frame_count,
            empty_frames: after.empty_frames.saturating_sub(before.empty_frames),
            dropped_oldest: after.dropped_oldest.saturating_sub(before.dropped_oldest),
            dropped_newest: after.dropped_newest.saturating_sub(before.dropped_newest),
            missed_deadlines: after
                .missed_deadlines
                .saturating_sub(before.missed_deadlines),
            total_lateness_nanos: after
                .total_lateness_nanos
                .saturating_sub(before.total_lateness_nanos),
            total_wait_nanos,
            total_render_nanos,
            max_lateness_nanos,
            remaining_queue: self.pipeline.queued(),
        })
    }

    /// Returns the worker's accumulated pipeline metrics.
    #[must_use]
    pub const fn metrics(&self) -> VideoMetrics {
        self.pipeline.metrics()
    }

    /// Resets both the pacer and the pipeline counters/queue for a new run.
    pub fn reset(&mut self) {
        self.pacer.reset();
        self.pipeline.queue.clear();
        self.pipeline.reset_metrics();
    }
}
