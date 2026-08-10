//! Deterministic video scheduling and bounded frame transport.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

/// Maximum number of threads created by the bounded multi-worker soak helper.
pub const MAX_SOAK_WORKERS: usize = 64;

/// Policy used when a bounded queue has no free slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropPolicy {
    /// Remove the oldest queued frame and keep the new frame.
    DropOldest,
    /// Keep queued frames and discard the newly submitted frame.
    DropNewest,
}

/// Result of submitting a frame to a bounded queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    /// The frame was stored without dropping another frame.
    Enqueued,
    /// The oldest frame was removed to store the submitted frame.
    DroppedOldest(Timestamp),
    /// The submitted frame was discarded.
    DroppedNewest(Timestamp),
}

/// Errors raised by the video scheduler and queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoError {
    /// A queue must have at least one slot.
    ZeroCapacity,
    /// A frame does not match the queue's configured format.
    FormatMismatch {
        /// Format accepted by the queue.
        expected: VideoFormat,
        /// Format supplied by the caller.
        actual: VideoFormat,
    },
    /// The timestamp calculation or frame index would overflow.
    ScheduleOverflow,
    /// A multi-worker soak was requested without any workers.
    ZeroWorkers,
    /// A multi-worker soak exceeds the bounded worker limit.
    TooManyWorkers { workers: usize },
    /// A multi-worker soak worker terminated unexpectedly.
    WorkerPanic,
}

impl fmt::Display for VideoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("video queue capacity must be non-zero"),
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "video format {actual:?} does not match {expected:?}"
                )
            }
            Self::ScheduleOverflow => formatter.write_str("video schedule timestamp overflowed"),
            Self::ZeroWorkers => formatter.write_str("video worker count must be non-zero"),
            Self::TooManyWorkers { workers } => {
                write!(formatter, "video worker count is too large: {workers}")
            }
            Self::WorkerPanic => formatter.write_str("video soak worker terminated unexpectedly"),
        }
    }
}

impl std::error::Error for VideoError {}

/// A bounded single-consumer frame queue.
pub struct FrameQueue {
    format: VideoFormat,
    capacity: usize,
    policy: DropPolicy,
    frames: VecDeque<VideoFrame>,
}

impl FrameQueue {
    /// Creates a queue with a fixed format, capacity, and drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ZeroCapacity`] when `capacity` is zero.
    pub fn new(
        format: VideoFormat,
        capacity: usize,
        policy: DropPolicy,
    ) -> Result<Self, VideoError> {
        if capacity == 0 {
            return Err(VideoError::ZeroCapacity);
        }

        Ok(Self {
            format,
            capacity,
            policy,
            frames: VecDeque::with_capacity(capacity),
        })
    }

    /// Submits one frame, applying the configured bounded-drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::FormatMismatch`] when the frame format differs from
    /// the queue format. The queue is unchanged in that case.
    pub fn push(&mut self, frame: VideoFrame) -> Result<PushOutcome, VideoError> {
        if frame.format() != self.format {
            return Err(VideoError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }

        if self.frames.len() < self.capacity {
            self.frames.push_back(frame);
            return Ok(PushOutcome::Enqueued);
        }

        match self.policy {
            DropPolicy::DropOldest => {
                let dropped = self
                    .frames
                    .pop_front()
                    .map_or(Timestamp::ZERO, |frame| frame.timestamp());
                self.frames.push_back(frame);
                Ok(PushOutcome::DroppedOldest(dropped))
            }
            DropPolicy::DropNewest => Ok(PushOutcome::DroppedNewest(frame.timestamp())),
        }
    }

    /// Removes and returns the oldest queued frame.
    pub fn pop(&mut self) -> Option<VideoFrame> {
        self.frames.pop_front()
    }

    /// Returns the oldest queued frame without removing it.
    #[must_use]
    pub fn front(&self) -> Option<&VideoFrame> {
        self.frames.front()
    }

    /// Removes all queued frames.
    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// Returns the queue's configured format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the fixed queue capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the current number of queued frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns whether no frame is currently queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// A rational frame deadline generator with no floating-point drift.
pub struct VideoScheduler {
    frame_rate: FrameRate,
    next_index: u64,
}

/// A monotonic wall-clock origin for render-loop integration.
pub struct MonotonicClock {
    origin: Instant,
}

/// Clock operations needed by the portable video pacer.
pub trait VideoClock {
    /// Returns elapsed monotonic time from the clock's origin.
    fn now(&self) -> Timestamp;

    /// Waits until `deadline`, returning immediately when it has already passed.
    fn sleep_until(&mut self, deadline: Timestamp);
}

impl MonotonicClock {
    /// Starts a clock at the current monotonic instant.
    #[must_use]
    pub fn start() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// Returns elapsed nanoseconds since [`Self::start`].
    #[must_use]
    pub fn now(&self) -> Timestamp {
        Timestamp::from_nanos(u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}

impl VideoClock for MonotonicClock {
    fn now(&self) -> Timestamp {
        Self::now(self)
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        let current = Self::now(self);
        let remaining = deadline.as_nanos().saturating_sub(current.as_nanos());
        if remaining != 0 {
            std::thread::sleep(Duration::from_nanos(remaining));
        }
    }
}

/// Comparison between a scheduled deadline and an observed wall-clock time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineObservation {
    deadline: FrameDeadline,
    observed: Timestamp,
    lateness_nanos: u64,
}

impl DeadlineObservation {
    /// Creates an observation without using a wall clock.
    #[must_use]
    pub fn new(deadline: FrameDeadline, observed: Timestamp) -> Self {
        Self {
            deadline,
            observed,
            lateness_nanos: observed
                .as_nanos()
                .saturating_sub(deadline.timestamp().as_nanos()),
        }
    }

    /// Returns the observed deadline record.
    #[must_use]
    pub const fn deadline(self) -> FrameDeadline {
        self.deadline
    }

    /// Returns the observed time.
    #[must_use]
    pub const fn observed(self) -> Timestamp {
        self.observed
    }

    /// Returns whether the observation arrived after the deadline.
    #[must_use]
    pub const fn missed(self) -> bool {
        self.lateness_nanos != 0
    }

    /// Returns lateness in nanoseconds, or zero when early/on time.
    #[must_use]
    pub const fn lateness_nanos(self) -> u64 {
        self.lateness_nanos
    }
}

/// One scheduled frame index and its target timestamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameDeadline {
    index: u64,
    timestamp: Timestamp,
}

impl FrameDeadline {
    /// Returns the zero-based frame index.
    #[must_use]
    pub const fn index(self) -> u64 {
        self.index
    }

    /// Returns the target timestamp.
    #[must_use]
    pub const fn timestamp(self) -> Timestamp {
        self.timestamp
    }
}

/// The result of waiting for one scheduled video deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacingResult {
    deadline: FrameDeadline,
    requested_at: Timestamp,
    observed_at: Timestamp,
    waited_nanos: u64,
}

impl PacingResult {
    /// Returns the scheduled deadline.
    #[must_use]
    pub const fn deadline(self) -> FrameDeadline {
        self.deadline
    }

    /// Returns the clock reading before waiting.
    #[must_use]
    pub const fn requested_at(self) -> Timestamp {
        self.requested_at
    }

    /// Returns the clock reading after waiting.
    #[must_use]
    pub const fn observed_at(self) -> Timestamp {
        self.observed_at
    }

    /// Returns elapsed wall-clock time spent in the wait operation.
    #[must_use]
    pub const fn waited_nanos(self) -> u64 {
        self.waited_nanos
    }

    /// Returns whether the clock reached the deadline late.
    #[must_use]
    pub const fn missed(self) -> bool {
        self.observed_at.as_nanos() > self.deadline.timestamp().as_nanos()
    }

    /// Returns lateness after the scheduled deadline, or zero when on time.
    #[must_use]
    pub const fn lateness_nanos(self) -> u64 {
        self.observed_at
            .as_nanos()
            .saturating_sub(self.deadline.timestamp().as_nanos())
    }
}

/// A wall-clock pacer that turns rational frame deadlines into waits.
pub struct VideoPacer {
    scheduler: VideoScheduler,
}

impl VideoPacer {
    /// Creates a pacer beginning at frame index zero.
    #[must_use]
    pub const fn new(frame_rate: FrameRate) -> Self {
        Self {
            scheduler: VideoScheduler::new(frame_rate),
        }
    }

    /// Waits for and returns the next scheduled frame deadline.
    ///
    /// The clock is injected so production code can use [`MonotonicClock`] while
    /// tests can use a deterministic clock without sleeping.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ScheduleOverflow`] when the timeline is exhausted.
    pub fn next<C: VideoClock>(&mut self, clock: &mut C) -> Result<PacingResult, VideoError> {
        let deadline = self.scheduler.next_deadline()?;
        let requested_at = clock.now();
        clock.sleep_until(deadline.timestamp());
        let observed_at = clock.now();
        Ok(PacingResult {
            deadline,
            requested_at,
            observed_at,
            waited_nanos: observed_at
                .as_nanos()
                .saturating_sub(requested_at.as_nanos()),
        })
    }

    /// Resets the pacer to frame index zero.
    pub fn reset(&mut self) {
        self.scheduler.reset();
    }

    /// Returns the configured frame rate.
    #[must_use]
    pub const fn frame_rate(&self) -> FrameRate {
        self.scheduler.frame_rate()
    }
}

impl VideoScheduler {
    /// Creates a scheduler beginning at frame index zero.
    #[must_use]
    pub const fn new(frame_rate: FrameRate) -> Self {
        Self {
            frame_rate,
            next_index: 0,
        }
    }

    /// Returns and advances the next frame deadline.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ScheduleOverflow`] when the frame index or timestamp
    /// no longer fits in the public integer representation.
    pub fn next_deadline(&mut self) -> Result<FrameDeadline, VideoError> {
        let index = self.next_index;
        let timestamp = timestamp_for(index, self.frame_rate)?;
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or(VideoError::ScheduleOverflow)?;
        Ok(FrameDeadline { index, timestamp })
    }

    /// Resets the scheduler to frame index zero.
    pub fn reset(&mut self) {
        self.next_index = 0;
    }

    /// Returns the configured frame rate.
    #[must_use]
    pub const fn frame_rate(&self) -> FrameRate {
        self.frame_rate
    }
}

/// The current reference video transport: a scheduler feeding a bounded queue.
pub struct VideoPipeline {
    scheduler: VideoScheduler,
    queue: FrameQueue,
    metrics: VideoMetrics,
}

/// Counters collected by the reference render loop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VideoMetrics {
    render_calls: u64,
    produced_frames: u64,
    produced_bytes: u64,
    peak_queued_bytes: u64,
    empty_frames: u64,
    dropped_oldest: u64,
    dropped_newest: u64,
    missed_deadlines: u64,
    total_lateness_nanos: u64,
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

/// Errors returned by one callback-driven render step.
#[derive(Debug, Eq, PartialEq)]
pub enum RenderError<E> {
    /// The scheduler could not produce a deadline.
    Schedule(VideoError),
    /// The source callback failed.
    Source(E),
    /// The callback produced a frame that the queue rejected.
    Submit(VideoError),
}

impl<E: fmt::Display> fmt::Display for RenderError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schedule(error) => write!(formatter, "video schedule failed: {error}"),
            Self::Source(error) => write!(formatter, "video source failed: {error}"),
            Self::Submit(error) => write!(formatter, "video submission failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RenderError<E> {}

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

/// Errors from a paced video worker.
#[derive(Debug, Eq, PartialEq)]
pub enum WorkerError<E> {
    /// The wall-clock pacer could not advance its timeline.
    Pacing(VideoError),
    /// The render callback or bounded submission failed.
    Render(RenderError<E>),
}

impl<E: fmt::Display> fmt::Display for WorkerError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pacing(error) => write!(formatter, "video worker pacing failed: {error}"),
            Self::Render(error) => write!(formatter, "video worker render failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for WorkerError<E> {}

/// A paced, cancellation-aware video producer over a bounded output pipeline.
pub struct VideoWorker {
    pacer: VideoPacer,
    pipeline: VideoPipeline,
}

/// Aggregate evidence from a wall-clock multi-worker video soak.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MultiWorkerSoakReport {
    workers: usize,
    requested_frames: u64,
    processed_frames: u64,
    missed_deadlines: u64,
    total_lateness_nanos: u64,
    produced_bytes: u64,
    peak_queued_bytes: u64,
    elapsed_nanos: u64,
}

impl MultiWorkerSoakReport {
    /// Returns the number of worker threads used by the soak.
    #[must_use]
    pub const fn workers(self) -> usize {
        self.workers
    }

    /// Returns total requested frames across all workers.
    #[must_use]
    pub const fn requested_frames(self) -> u64 {
        self.requested_frames
    }

    /// Returns total processed frames across all workers.
    #[must_use]
    pub const fn processed_frames(self) -> u64 {
        self.processed_frames
    }

    /// Returns total missed deadlines across all workers.
    #[must_use]
    pub const fn missed_deadlines(self) -> u64 {
        self.missed_deadlines
    }

    /// Returns total observed post-deadline lateness.
    #[must_use]
    pub const fn total_lateness_nanos(self) -> u64 {
        self.total_lateness_nanos
    }

    /// Returns total owned pixel bytes produced by all workers.
    #[must_use]
    pub const fn produced_bytes(self) -> u64 {
        self.produced_bytes
    }

    /// Returns the largest per-worker queued pixel footprint.
    #[must_use]
    pub const fn peak_queued_bytes(self) -> u64 {
        self.peak_queued_bytes
    }

    /// Returns elapsed wall-clock time for the complete soak.
    #[must_use]
    pub const fn elapsed_nanos(self) -> u64 {
        self.elapsed_nanos
    }
}

/// Runs bounded independent video workers against deterministic solid frames.
///
/// This is a wall-clock stress fixture for scheduler, queue, cancellation, and
/// owned-frame accounting. Each worker owns its scheduler and queue; no runtime
/// state is shared, which makes it safe to replace the solid-frame callback with
/// a thread-safe source adapter in integration tests.
///
/// # Errors
///
/// Returns [`VideoError::ZeroWorkers`] or [`VideoError::TooManyWorkers`] for an
/// invalid worker count, [`VideoError::WorkerPanic`] if a worker thread exits
/// unexpectedly, or a worker scheduling/submission error.
pub fn run_multi_worker_soak(
    format: VideoFormat,
    worker_count: usize,
    frames_per_worker: u64,
    queue_capacity: usize,
    policy: DropPolicy,
) -> Result<MultiWorkerSoakReport, VideoError> {
    if worker_count == 0 {
        return Err(VideoError::ZeroWorkers);
    }
    if worker_count > MAX_SOAK_WORKERS {
        return Err(VideoError::TooManyWorkers {
            workers: worker_count,
        });
    }
    let started = Instant::now();
    let mut handles = Vec::with_capacity(worker_count);
    for worker_index in 0..worker_count {
        handles.push(std::thread::spawn(move || {
            let mut worker = VideoWorker::new(format, queue_capacity, policy)?;
            let cancellation = CancellationToken::new();
            let mut clock = MonotonicClock::start();
            worker
                .run(
                    &mut clock,
                    frames_per_worker,
                    &cancellation,
                    move |deadline, output_format| {
                        let shade = u8::try_from(worker_index % 255).unwrap_or(0);
                        Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                            output_format,
                            deadline.timestamp(),
                            [shade, 64, 128, 255],
                        )))
                    },
                )
                .map_err(|error| match error {
                    WorkerError::Pacing(error)
                    | WorkerError::Render(
                        RenderError::Schedule(error) | RenderError::Submit(error),
                    ) => error,
                    WorkerError::Render(RenderError::Source(error)) => match error {},
                })
        }));
    }

    let mut aggregate = MultiWorkerSoakReport {
        workers: worker_count,
        ..MultiWorkerSoakReport::default()
    };
    for handle in handles {
        let report = handle.join().map_err(|_| VideoError::WorkerPanic)??;
        aggregate.requested_frames = aggregate
            .requested_frames
            .saturating_add(report.requested_frames());
        aggregate.processed_frames = aggregate
            .processed_frames
            .saturating_add(report.processed_frames());
        aggregate.missed_deadlines = aggregate
            .missed_deadlines
            .saturating_add(report.missed_deadlines());
        aggregate.total_lateness_nanos = aggregate
            .total_lateness_nanos
            .saturating_add(report.total_lateness_nanos());
        aggregate.produced_bytes = aggregate
            .produced_bytes
            .saturating_add(report.produced_bytes());
        aggregate.peak_queued_bytes = aggregate.peak_queued_bytes.max(report.peak_queued_bytes());
    }
    aggregate.elapsed_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    Ok(aggregate)
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
    /// This is used by [`VideoWorker`] after wall-clock pacing. Unlike
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
        let frame = render(deadline, self.queue.format()).map_err(RenderError::Source)?;
        let Some(frame) = frame else {
            self.metrics.empty_frames = self.metrics.empty_frames.saturating_add(1);
            return Ok(RenderOutcome::Empty { deadline });
        };

        self.metrics.produced_frames = self.metrics.produced_frames.saturating_add(1);
        self.metrics.produced_bytes = self
            .metrics
            .produced_bytes
            .saturating_add(u64::try_from(frame.pixels().len()).unwrap_or(u64::MAX));
        let outcome = self.queue.push(frame).map_err(RenderError::Submit)?;
        let queued_bytes = u64::try_from(self.queue.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(
                u64::from(self.queue.format().width())
                    .saturating_mul(u64::from(self.queue.format().height()))
                    .saturating_mul(4),
            );
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

fn timestamp_for(index: u64, frame_rate: FrameRate) -> Result<Timestamp, VideoError> {
    let numerator = u128::from(index)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_mul(u128::from(frame_rate.denominator())))
        .ok_or(VideoError::ScheduleOverflow)?;
    let nanoseconds = numerator / u128::from(frame_rate.numerator());
    let nanoseconds = u64::try_from(nanoseconds).map_err(|_| VideoError::ScheduleOverflow)?;
    Ok(Timestamp::from_nanos(nanoseconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock {
        now: Timestamp,
        requested_deadlines: Vec<Timestamp>,
    }

    impl VideoClock for FakeClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep_until(&mut self, deadline: Timestamp) {
            self.requested_deadlines.push(deadline);
            if deadline > self.now {
                self.now = deadline;
            }
        }
    }

    fn format() -> VideoFormat {
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
    }

    fn frame(timestamp: u64, color: [u8; 4]) -> VideoFrame {
        VideoFrame::solid(format(), Timestamp::from_nanos(timestamp), color)
    }

    #[test]
    fn scheduler_has_exact_rational_timestamps() {
        let rate = FrameRate::new(30_000, 1_001).expect("valid rate");
        let mut scheduler = VideoScheduler::new(rate);

        assert_eq!(
            scheduler.next_deadline().expect("first deadline"),
            FrameDeadline {
                index: 0,
                timestamp: Timestamp::ZERO
            }
        );
        assert_eq!(
            scheduler
                .next_deadline()
                .expect("second deadline")
                .timestamp(),
            Timestamp::from_nanos(33_366_666)
        );
        assert_eq!(
            scheduler
                .next_deadline()
                .expect("third deadline")
                .timestamp(),
            Timestamp::from_nanos(66_733_333)
        );
        scheduler.reset();
        assert_eq!(
            scheduler.next_deadline().expect("reset deadline").index(),
            0
        );
    }

    #[test]
    fn queue_drops_oldest_when_configured() {
        let mut queue = FrameQueue::new(format(), 2, DropPolicy::DropOldest).expect("capacity");
        queue.push(frame(1, [1, 0, 0, 255])).expect("first push");
        queue.push(frame(2, [2, 0, 0, 255])).expect("second push");

        assert_eq!(
            queue.push(frame(3, [3, 0, 0, 255])).expect("third push"),
            PushOutcome::DroppedOldest(Timestamp::from_nanos(1))
        );
        assert_eq!(
            queue.pop().expect("first remaining").timestamp(),
            Timestamp::from_nanos(2)
        );
        assert_eq!(
            queue.pop().expect("second remaining").timestamp(),
            Timestamp::from_nanos(3)
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_can_drop_newest_and_reject_wrong_formats() {
        let mut queue = FrameQueue::new(format(), 1, DropPolicy::DropNewest).expect("capacity");
        queue.push(frame(1, [1, 0, 0, 255])).expect("first push");
        assert_eq!(
            queue.push(frame(2, [2, 0, 0, 255])).expect("second push"),
            PushOutcome::DroppedNewest(Timestamp::from_nanos(2))
        );
        let other_format = VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        assert!(matches!(
            queue.push(VideoFrame::solid(
                other_format,
                Timestamp::ZERO,
                [0, 0, 0, 255]
            )),
            Err(VideoError::FormatMismatch { .. })
        ));
    }

    #[test]
    fn pipeline_combines_schedule_and_queue() {
        let mut pipeline =
            VideoPipeline::new(format(), 2, DropPolicy::DropOldest).expect("pipeline");
        assert_eq!(pipeline.next_deadline().expect("deadline").index(), 0);
        pipeline.submit(frame(0, [0, 0, 0, 255])).expect("submit");
        assert_eq!(pipeline.queued(), 1);
        assert_eq!(
            pipeline.take_next().expect("output").timestamp(),
            Timestamp::ZERO
        );
    }

    #[test]
    fn render_loop_tracks_empty_frames_and_queue_drops() {
        let mut pipeline =
            VideoPipeline::new(format(), 1, DropPolicy::DropOldest).expect("pipeline");
        let first = pipeline
            .render_next(|deadline, format| {
                Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                    format,
                    deadline.timestamp(),
                    [1, 0, 0, 255],
                )))
            })
            .expect("first render");
        let second = pipeline
            .render_next(|deadline, format| {
                Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                    format,
                    deadline.timestamp(),
                    [2, 0, 0, 255],
                )))
            })
            .expect("second render");
        let empty = pipeline
            .render_next(|_, _| Ok::<_, std::convert::Infallible>(None))
            .expect("empty render");

        assert!(matches!(first, RenderOutcome::Enqueued { .. }));
        assert!(matches!(
            second,
            RenderOutcome::DroppedOldest {
                dropped: Timestamp::ZERO,
                ..
            }
        ));
        assert!(matches!(empty, RenderOutcome::Empty { .. }));
        assert_eq!(pipeline.metrics().render_calls(), 3);
        assert_eq!(pipeline.metrics().produced_frames(), 2);
        assert_eq!(pipeline.metrics().produced_bytes(), 16);
        assert_eq!(pipeline.metrics().peak_queued_bytes(), 8);
        assert_eq!(pipeline.metrics().empty_frames(), 1);
        assert_eq!(pipeline.metrics().dropped_oldest(), 1);
        let observation = pipeline.observe_deadline(
            FrameDeadline {
                index: 1,
                timestamp: Timestamp::from_nanos(10),
            },
            Timestamp::from_nanos(25),
        );
        assert!(observation.missed());
        assert_eq!(pipeline.metrics().missed_deadlines(), 1);
        assert_eq!(pipeline.metrics().total_lateness_nanos(), 15);
    }

    #[test]
    fn sustained_run_reports_counter_deltas_and_drains_output() {
        let mut pipeline =
            VideoPipeline::new(format(), 2, DropPolicy::DropOldest).expect("pipeline");
        let report = pipeline
            .run_sustained(120, |deadline, format| {
                Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                    format,
                    deadline.timestamp(),
                    [7, 8, 9, 255],
                )))
            })
            .expect("sustained run");

        assert_eq!(report.requested_frames(), 120);
        assert_eq!(report.produced_frames(), 120);
        assert_eq!(report.empty_frames(), 0);
        assert_eq!(report.dropped_oldest(), 0);
        assert_eq!(report.dropped_newest(), 0);
        assert_eq!(report.remaining_queue(), 0);
        assert_eq!(pipeline.metrics().render_calls(), 120);
    }

    #[test]
    fn paced_worker_reports_lateness_and_honors_callback_cancellation() {
        let mut clock = FakeClock {
            now: Timestamp::from_millis(5),
            requested_deadlines: Vec::new(),
        };
        let token = CancellationToken::new();
        let mut worker = VideoWorker::new(format(), 2, DropPolicy::DropOldest).expect("worker");
        let report = worker
            .run(&mut clock, 10, &token, |deadline, output_format| {
                if deadline.index() == 2 {
                    token.cancel();
                }
                Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                    output_format,
                    deadline.timestamp(),
                    [1, 2, 3, 255],
                )))
            })
            .expect("worker run");

        assert_eq!(report.requested_frames(), 10);
        assert_eq!(report.processed_frames(), 3);
        assert!(report.cancelled());
        assert_eq!(report.missed_deadlines(), 1);
        assert_eq!(report.total_lateness_nanos(), 5_000_000);
        assert_eq!(report.total_wait_nanos(), 61_666_666);
        assert_eq!(report.total_render_nanos(), 0);
        assert_eq!(report.max_lateness_nanos(), 5_000_000);
        assert_eq!(report.empty_frames(), 0);
        assert_eq!(report.remaining_queue(), 0);
        assert_eq!(clock.requested_deadlines.len(), 3);
    }

    #[test]
    fn monotonic_clock_is_non_decreasing() {
        let clock = MonotonicClock::start();
        let first = clock.now();
        let second = clock.now();

        assert!(second >= first);
    }

    #[test]
    fn multi_worker_soak_reports_wall_clock_and_owned_frame_footprint() {
        let report = run_multi_worker_soak(format(), 2, 2, 1, DropPolicy::DropOldest)
            .expect("multi-worker soak");
        assert_eq!(report.workers(), 2);
        assert_eq!(report.requested_frames(), 4);
        assert_eq!(report.processed_frames(), 4);
        assert_eq!(report.produced_bytes(), 32);
        assert_eq!(report.peak_queued_bytes(), 8);
        assert!(report.elapsed_nanos() > 0);
        assert_eq!(
            run_multi_worker_soak(format(), 0, 1, 1, DropPolicy::DropOldest),
            Err(VideoError::ZeroWorkers)
        );
    }

    #[test]
    fn pacer_waits_with_an_injected_clock_and_reports_lateness() {
        let mut clock = FakeClock {
            now: Timestamp::from_millis(5),
            requested_deadlines: Vec::new(),
        };
        let mut pacer = VideoPacer::new(FrameRate::new(30, 1).expect("valid rate"));

        let first = pacer.next(&mut clock).expect("first paced deadline");
        assert_eq!(first.deadline().index(), 0);
        assert_eq!(first.requested_at(), Timestamp::from_millis(5));
        assert_eq!(first.observed_at(), Timestamp::from_millis(5));
        assert!(first.missed());
        assert_eq!(first.lateness_nanos(), 5_000_000);
        assert_eq!(first.waited_nanos(), 0);

        let second = pacer.next(&mut clock).expect("second paced deadline");
        assert_eq!(second.deadline().index(), 1);
        assert_eq!(second.observed_at(), Timestamp::from_nanos(33_333_333));
        assert_eq!(second.waited_nanos(), 28_333_333);
        assert!(!second.missed());
        assert_eq!(
            clock.requested_deadlines,
            vec![Timestamp::ZERO, Timestamp::from_nanos(33_333_333)]
        );
    }
}
