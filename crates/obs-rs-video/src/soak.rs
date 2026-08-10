use obs_rs_media::{VideoFormat, VideoFrame};
use std::time::Instant;

use super::{
    clock::MonotonicClock,
    error::{RenderError, VideoError, WorkerError},
    types::DropPolicy,
    worker::{CancellationToken, VideoWorker},
};
/// Maximum number of threads created by the bounded multi-worker soak helper.
pub const MAX_SOAK_WORKERS: usize = 64;
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
