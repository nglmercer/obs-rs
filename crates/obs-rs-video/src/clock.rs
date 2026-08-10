use obs_rs_media::{sleep_precise, FrameRate, Timestamp};
use std::time::{Duration, Instant};

use super::error::VideoError;
/// A rational frame deadline generator with no floating-point drift.
///
/// Deadlines advance by exact rational addition rather than being re-derived
/// from the frame index, so producing a deadline is a handful of 64-bit adds
/// instead of a 128-bit multiply and divide per frame. The values it yields are
/// identical to `index * 1_000_000_000 * denominator / numerator`.
pub struct VideoScheduler {
    frame_rate: FrameRate,
    next_index: u64,
    /// Nanosecond timestamp of the next deadline.
    next_nanos: u64,
    /// Sub-nanosecond carry, in units of `frame_rate.numerator()`.
    remainder: u64,
    /// Whole nanoseconds added per frame.
    whole_step: u64,
    /// Carry added per frame, in units of `frame_rate.numerator()`.
    remainder_step: u64,
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
        // Millisecond-granularity parking misses deadlines above 60 fps, so the
        // last stretch before the deadline is spun.
        sleep_precise(Duration::from_nanos(remaining));
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
    pub(crate) index: u64,
    pub(crate) timestamp: Timestamp,
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
        // One frame lasts `1e9 * denominator / numerator` nanoseconds. The whole
        // part and the leftover carry are constant, so both are resolved here.
        let step = 1_000_000_000_u64.saturating_mul(frame_rate.denominator() as u64);
        let numerator = frame_rate.numerator() as u64;
        Self {
            frame_rate,
            next_index: 0,
            next_nanos: 0,
            remainder: 0,
            whole_step: step / numerator,
            remainder_step: step % numerator,
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
        let timestamp = Timestamp::from_nanos(self.next_nanos);

        let mut next_nanos = self
            .next_nanos
            .checked_add(self.whole_step)
            .ok_or(VideoError::ScheduleOverflow)?;
        let mut remainder = self.remainder + self.remainder_step;
        if remainder >= u64::from(self.frame_rate.numerator()) {
            remainder -= u64::from(self.frame_rate.numerator());
            next_nanos = next_nanos
                .checked_add(1)
                .ok_or(VideoError::ScheduleOverflow)?;
        }
        let next_index = self
            .next_index
            .checked_add(1)
            .ok_or(VideoError::ScheduleOverflow)?;

        self.next_nanos = next_nanos;
        self.remainder = remainder;
        self.next_index = next_index;
        Ok(FrameDeadline { index, timestamp })
    }

    /// Resets the scheduler to frame index zero.
    pub fn reset(&mut self) {
        self.next_index = 0;
        self.next_nanos = 0;
        self.remainder = 0;
    }

    /// Returns the configured frame rate.
    #[must_use]
    pub const fn frame_rate(&self) -> FrameRate {
        self.frame_rate
    }
}
