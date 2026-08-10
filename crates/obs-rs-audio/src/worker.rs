use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::{
    buffer::AudioBuffer,
    error::{AudioError, AudioWorkerError},
    pacing::{AudioClock, AudioDeadline, AudioPacer},
    queue::{AudioDropPolicy, AudioPushOutcome, AudioQueue},
    types::AudioFormat,
};
/// A thread-safe cancellation flag checked between audio blocks.
#[derive(Clone, Debug)]
pub struct AudioCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl Default for AudioCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCancellationToken {
    /// Creates an uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation before the next block begins.
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

/// Delta diagnostics from one paced audio worker run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudioWorkerReport {
    requested_blocks: u64,
    processed_blocks: u64,
    cancelled: bool,
    underflow_blocks: u64,
    produced_frames: u64,
    dropped_oldest_frames: u64,
    dropped_newest_frames: u64,
    missed_deadlines: u64,
    total_lateness_nanos: u64,
    remaining_queue_frames: usize,
}

impl AudioWorkerReport {
    /// Returns the number of blocks requested from the worker.
    #[must_use]
    pub const fn requested_blocks(self) -> u64 {
        self.requested_blocks
    }

    /// Returns the number of paced blocks completed.
    #[must_use]
    pub const fn processed_blocks(self) -> u64 {
        self.processed_blocks
    }

    /// Returns whether cancellation stopped the run before its requested count.
    #[must_use]
    pub const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// Returns the number of blocks for which the producer returned no audio.
    #[must_use]
    pub const fn underflow_blocks(self) -> u64 {
        self.underflow_blocks
    }

    /// Returns the number of produced sample frames, including dropped output.
    #[must_use]
    pub const fn produced_frames(self) -> u64 {
        self.produced_frames
    }

    /// Returns the number of old sample frames removed under drop-oldest pressure.
    #[must_use]
    pub const fn dropped_oldest_frames(self) -> u64 {
        self.dropped_oldest_frames
    }

    /// Returns the number of submitted sample frames discarded under drop-newest pressure.
    #[must_use]
    pub const fn dropped_newest_frames(self) -> u64 {
        self.dropped_newest_frames
    }

    /// Returns the number of post-callback deadlines observed late.
    #[must_use]
    pub const fn missed_deadlines(self) -> u64 {
        self.missed_deadlines
    }

    /// Returns total post-callback lateness in nanoseconds.
    #[must_use]
    pub const fn total_lateness_nanos(self) -> u64 {
        self.total_lateness_nanos
    }

    /// Returns the number of sample frames remaining in the output queue.
    #[must_use]
    pub const fn remaining_queue_frames(self) -> usize {
        self.remaining_queue_frames
    }
}

/// A paced, cancellation-aware audio producer over a bounded output queue.
pub struct AudioWorker {
    pacer: AudioPacer,
    queue: AudioQueue,
}

impl AudioWorker {
    /// Creates a worker for one audio format and bounded output policy.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroCapacity`] when `capacity_frames` is zero.
    pub fn new(
        format: AudioFormat,
        capacity_frames: usize,
        policy: AudioDropPolicy,
    ) -> Result<Self, AudioError> {
        Ok(Self {
            pacer: AudioPacer::new(format),
            queue: AudioQueue::new(format, capacity_frames, policy)?,
        })
    }

    /// Runs up to `block_count` paced blocks without blocking the output consumer.
    ///
    /// The producer is called after each block reaches its sample-clock deadline.
    /// A produced buffer must match the worker format, block length, and exact
    /// first-sample timestamp. The worker retains complete buffers in its bounded
    /// queue; callers consume them with [`Self::take_next`].
    ///
    /// # Errors
    ///
    /// Returns [`AudioWorkerError::Pacing`] for clock/timeline failures,
    /// [`AudioWorkerError::Source`] for producer failures, or
    /// [`AudioWorkerError::Submit`] for a buffer contract violation.
    pub fn run<C, E, F>(
        &mut self,
        clock: &mut C,
        block_frames: usize,
        block_count: u64,
        cancellation: &AudioCancellationToken,
        mut produce: F,
    ) -> Result<AudioWorkerReport, AudioWorkerError<E>>
    where
        C: AudioClock,
        F: FnMut(AudioDeadline, AudioFormat, usize) -> Result<Option<AudioBuffer>, E>,
    {
        let format = self.pacer.format();
        let mut report = AudioWorkerReport {
            requested_blocks: block_count,
            ..AudioWorkerReport::default()
        };

        for _ in 0..block_count {
            if cancellation.is_cancelled() {
                break;
            }

            let pacing = self
                .pacer
                .next(clock, block_frames)
                .map_err(AudioWorkerError::Pacing)?;
            let deadline = pacing.deadline();
            let produced =
                produce(deadline, format, block_frames).map_err(AudioWorkerError::Source)?;

            match produced {
                None => {
                    report.underflow_blocks = report.underflow_blocks.saturating_add(1);
                }
                Some(buffer) => {
                    if buffer.format() != format {
                        return Err(AudioWorkerError::Submit(AudioError::FormatMismatch {
                            expected: format,
                            actual: buffer.format(),
                        }));
                    }
                    if buffer.frames() != block_frames {
                        return Err(AudioWorkerError::Submit(AudioError::FrameCountMismatch {
                            expected: block_frames,
                            actual: buffer.frames(),
                        }));
                    }
                    if buffer.timestamp() != deadline.timestamp() {
                        return Err(AudioWorkerError::Submit(
                            AudioError::BufferTimestampMismatch {
                                expected: deadline.timestamp(),
                                actual: buffer.timestamp(),
                            },
                        ));
                    }

                    report.produced_frames = report
                        .produced_frames
                        .saturating_add(u64::try_from(buffer.frames()).unwrap_or(u64::MAX));
                    match self.queue.push(buffer).map_err(AudioWorkerError::Submit)? {
                        AudioPushOutcome::Enqueued => {}
                        AudioPushOutcome::DroppedOldest { frames } => {
                            report.dropped_oldest_frames = report
                                .dropped_oldest_frames
                                .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
                        }
                        AudioPushOutcome::DroppedNewest { frames } => {
                            report.dropped_newest_frames = report
                                .dropped_newest_frames
                                .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
                        }
                    }
                }
            }

            let observed_at = clock.now();
            let lateness = observed_at
                .as_nanos()
                .saturating_sub(deadline.timestamp().as_nanos());
            if lateness != 0 {
                report.missed_deadlines = report.missed_deadlines.saturating_add(1);
                report.total_lateness_nanos = report.total_lateness_nanos.saturating_add(lateness);
            }
            report.processed_blocks = report.processed_blocks.saturating_add(1);
        }

        report.cancelled = cancellation.is_cancelled() && report.processed_blocks < block_count;
        report.remaining_queue_frames = self.queue.queued_frames();
        Ok(report)
    }

    /// Removes and returns the oldest produced audio block.
    pub fn take_next(&mut self) -> Option<AudioBuffer> {
        self.queue.pop()
    }

    /// Returns the number of sample frames waiting for the output consumer.
    #[must_use]
    pub const fn queued_frames(&self) -> usize {
        self.queue.queued_frames()
    }

    /// Returns the configured worker format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.pacer.format()
    }

    /// Resets the timeline and discards queued output for a new run.
    pub fn reset(&mut self) {
        self.pacer.reset();
        self.queue.clear();
    }
}
