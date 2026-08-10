use super::{error::AudioError, types::AudioFormat};
use obs_rs_media::Timestamp;
use std::time::{Duration, Instant};
/// One exact sample-clock deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDeadline {
    pub(crate) index: u64,
    pub(crate) timestamp: Timestamp,
}

impl AudioDeadline {
    /// Returns the zero-based sample-frame index.
    #[must_use]
    pub const fn index(self) -> u64 {
        self.index
    }

    /// Returns the exact integer nanosecond timestamp for the frame.
    #[must_use]
    pub const fn timestamp(self) -> Timestamp {
        self.timestamp
    }
}

/// A rational sample-clock scheduler without floating-point drift.
pub struct AudioScheduler {
    format: AudioFormat,
    next_index: u64,
}

/// Clock operations needed by the portable audio pacer.
pub trait AudioClock {
    /// Returns elapsed monotonic time from the clock's origin.
    fn now(&self) -> Timestamp;

    /// Waits until `deadline`, returning immediately when it has already passed.
    fn sleep_until(&mut self, deadline: Timestamp);
}

/// A monotonic wall-clock origin for audio callback integration.
pub struct MonotonicAudioClock {
    origin: Instant,
}

/// The result of waiting for one audio block deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPacingResult {
    deadline: AudioDeadline,
    frames: usize,
    requested_at: Timestamp,
    observed_at: Timestamp,
    waited_nanos: u64,
}

impl AudioPacingResult {
    /// Returns the first sample-frame deadline in the block.
    #[must_use]
    pub const fn deadline(self) -> AudioDeadline {
        self.deadline
    }

    /// Returns the number of sample frames represented by the block.
    #[must_use]
    pub const fn frames(self) -> usize {
        self.frames
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

    /// Returns elapsed time spent waiting.
    #[must_use]
    pub const fn waited_nanos(self) -> u64 {
        self.waited_nanos
    }

    /// Returns whether the block was observed after its first-sample deadline.
    #[must_use]
    pub const fn missed(self) -> bool {
        self.observed_at.as_nanos() > self.deadline.timestamp().as_nanos()
    }

    /// Returns lateness after the first-sample deadline, or zero when on time.
    #[must_use]
    pub const fn lateness_nanos(self) -> u64 {
        self.observed_at
            .as_nanos()
            .saturating_sub(self.deadline.timestamp().as_nanos())
    }
}

/// A block-based sample-clock pacer with an injected wall clock.
pub struct AudioPacer {
    scheduler: AudioScheduler,
}
impl AudioScheduler {
    /// Creates a scheduler beginning at sample-frame index zero.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            next_index: 0,
        }
    }

    /// Returns and advances the next exact sample-clock deadline.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] when the timeline no longer fits
    /// in the public integer representation.
    pub fn next_deadline(&mut self) -> Result<AudioDeadline, AudioError> {
        let index = self.next_index;
        let timestamp = audio_timestamp_for(index, self.format.sample_rate())?;
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or(AudioError::ScheduleOverflow)?;
        Ok(AudioDeadline { index, timestamp })
    }

    /// Returns and advances the first deadline of a non-empty audio block.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroBlock`] for an empty block or
    /// [`AudioError::ScheduleOverflow`] when advancing the sample index fails.
    pub fn next_block_deadline(&mut self, frames: usize) -> Result<AudioDeadline, AudioError> {
        if frames == 0 {
            return Err(AudioError::ZeroBlock);
        }
        let deadline = AudioDeadline {
            index: self.next_index,
            timestamp: audio_timestamp_for(self.next_index, self.format.sample_rate())?,
        };
        let frames = u64::try_from(frames).map_err(|_| AudioError::ScheduleOverflow)?;
        self.next_index = self
            .next_index
            .checked_add(frames)
            .ok_or(AudioError::ScheduleOverflow)?;
        Ok(deadline)
    }

    /// Resets the scheduler to sample-frame index zero.
    pub fn reset(&mut self) {
        self.next_index = 0;
    }

    /// Returns the scheduled audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }
}

impl MonotonicAudioClock {
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

impl AudioClock for MonotonicAudioClock {
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
impl AudioPacer {
    /// Creates a block pacer beginning at sample-frame index zero.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            scheduler: AudioScheduler::new(format),
        }
    }

    /// Waits for the first sample deadline of the next non-empty block.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroBlock`] for an empty block or
    /// [`AudioError::ScheduleOverflow`] when the sample timeline is exhausted.
    pub fn next<C: AudioClock>(
        &mut self,
        clock: &mut C,
        frames: usize,
    ) -> Result<AudioPacingResult, AudioError> {
        let deadline = self.scheduler.next_block_deadline(frames)?;
        let requested_at = clock.now();
        clock.sleep_until(deadline.timestamp());
        let observed_at = clock.now();
        Ok(AudioPacingResult {
            deadline,
            frames,
            requested_at,
            observed_at,
            waited_nanos: observed_at
                .as_nanos()
                .saturating_sub(requested_at.as_nanos()),
        })
    }

    /// Resets the pacer to sample-frame index zero.
    pub fn reset(&mut self) {
        self.scheduler.reset();
    }

    /// Returns the configured audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.scheduler.format()
    }
}

fn audio_timestamp_for(index: u64, sample_rate: u32) -> Result<Timestamp, AudioError> {
    let nanoseconds = u128::from(index)
        .checked_mul(1_000_000_000)
        .ok_or(AudioError::ScheduleOverflow)?
        / u128::from(sample_rate);
    let nanoseconds = u64::try_from(nanoseconds).map_err(|_| AudioError::ScheduleOverflow)?;
    Ok(Timestamp::from_nanos(nanoseconds))
}
