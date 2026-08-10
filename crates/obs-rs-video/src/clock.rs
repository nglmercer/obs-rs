use obs_rs_media::{FrameRate, Timestamp};
use std::time::{Duration, Instant};

use super::error::VideoError;
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
fn timestamp_for(index: u64, frame_rate: FrameRate) -> Result<Timestamp, VideoError> {
    let numerator = u128::from(index)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_mul(u128::from(frame_rate.denominator())))
        .ok_or(VideoError::ScheduleOverflow)?;
    let nanoseconds = numerator / u128::from(frame_rate.numerator());
    let nanoseconds = u64::try_from(nanoseconds).map_err(|_| VideoError::ScheduleOverflow)?;
    Ok(Timestamp::from_nanos(nanoseconds))
}
