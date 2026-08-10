use super::{
    error::AudioError,
    pacing::AudioClock,
    types::{AudioFormat, MAX_CALLBACK_CORRECTION_PPM},
};
use obs_rs_media::Timestamp;
/// An observed hardware-audio callback interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioCallbackObservation {
    timestamp: Timestamp,
    expected_timestamp: Timestamp,
    frames: usize,
    drift_nanos: i64,
}

impl AudioCallbackObservation {
    /// Returns the device timestamp supplied by the callback.
    #[must_use]
    pub const fn timestamp(self) -> Timestamp {
        self.timestamp
    }

    /// Returns the corrected sample-clock timestamp expected for this callback.
    #[must_use]
    pub const fn expected_timestamp(self) -> Timestamp {
        self.expected_timestamp
    }

    /// Returns the number of sample frames delivered by the callback.
    #[must_use]
    pub const fn frames(self) -> usize {
        self.frames
    }

    /// Returns `device - expected` in nanoseconds, saturated to `i64`.
    #[must_use]
    pub const fn drift_nanos(self) -> i64 {
        self.drift_nanos
    }
}

/// A safe adapter for clocks reported by OS audio callbacks.
///
/// Platform crates can feed callback timestamps and frame counts into this
/// value without exposing callback-thread details to the audio worker. It never
/// sleeps: callback delivery is the clock edge, while [`AudioClock::sleep_until`]
/// is intentionally a no-op for this callback-driven source.
pub struct AudioCallbackClock {
    format: AudioFormat,
    origin: Option<Timestamp>,
    last_timestamp: Timestamp,
    delivered_frames: u64,
    correction_ppm: i32,
}

impl AudioCallbackClock {
    /// Creates an uninitialized callback clock for one device format.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            origin: None,
            last_timestamp: Timestamp::ZERO,
            delivered_frames: 0,
            correction_ppm: 0,
        }
    }

    /// Records one callback edge and returns its measured drift.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroBlock`] for an empty callback or
    /// [`AudioError::CallbackTimestampRegression`] when the device clock moves
    /// backward.
    pub fn observe_callback(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioCallbackObservation, AudioError> {
        if frames == 0 {
            return Err(AudioError::ZeroBlock);
        }
        if timestamp < self.last_timestamp && self.origin.is_some() {
            return Err(AudioError::CallbackTimestampRegression {
                previous: self.last_timestamp,
                actual: timestamp,
            });
        }
        let origin = *self.origin.get_or_insert(timestamp);
        let elapsed = corrected_audio_duration_nanos(
            self.delivered_frames,
            self.format.sample_rate(),
            self.correction_ppm,
        )?;
        let expected_timestamp = origin
            .checked_add(elapsed)
            .ok_or(AudioError::ScheduleOverflow)?;
        let drift = i128::from(timestamp.as_nanos())
            .saturating_sub(i128::from(expected_timestamp.as_nanos()));
        self.delivered_frames = self
            .delivered_frames
            .checked_add(u64::try_from(frames).map_err(|_| AudioError::ScheduleOverflow)?)
            .ok_or(AudioError::ScheduleOverflow)?;
        self.last_timestamp = timestamp;
        Ok(AudioCallbackObservation {
            timestamp,
            expected_timestamp,
            frames,
            drift_nanos: i64::try_from(drift).unwrap_or_else(|_| {
                if drift.is_negative() {
                    i64::MIN
                } else {
                    i64::MAX
                }
            }),
        })
    }

    /// Sets a bounded sample-clock correction in parts per million.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::CallbackCorrectionOutOfRange`] when the requested
    /// adjustment exceeds the safe bound.
    pub const fn set_correction_ppm(&mut self, correction_ppm: i32) -> Result<(), AudioError> {
        if correction_ppm < -MAX_CALLBACK_CORRECTION_PPM
            || correction_ppm > MAX_CALLBACK_CORRECTION_PPM
        {
            return Err(AudioError::CallbackCorrectionOutOfRange {
                ppm: correction_ppm,
            });
        }
        self.correction_ppm = correction_ppm;
        Ok(())
    }

    /// Returns the callback format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the most recent callback timestamp.
    #[must_use]
    pub const fn callback_timestamp(&self) -> Timestamp {
        self.last_timestamp
    }

    /// Returns the total callback sample frames observed.
    #[must_use]
    pub const fn delivered_frames(&self) -> u64 {
        self.delivered_frames
    }

    /// Returns the active rate correction in parts per million.
    #[must_use]
    pub const fn correction_ppm(&self) -> i32 {
        self.correction_ppm
    }
}
impl AudioClock for AudioCallbackClock {
    fn now(&self) -> Timestamp {
        self.callback_timestamp()
    }

    fn sleep_until(&mut self, _deadline: Timestamp) {
        // The next callback is the device-owned clock edge. A callback thread
        // must never be blocked by the portable worker's pacing contract.
    }
}
fn corrected_audio_duration_nanos(
    frames: u64,
    sample_rate: u32,
    correction_ppm: i32,
) -> Result<u64, AudioError> {
    let duration = u128::from(frames)
        .checked_mul(1_000_000_000)
        .ok_or(AudioError::ScheduleOverflow)?;
    let duration = i128::try_from(duration).map_err(|_| AudioError::ScheduleOverflow)?;
    let scale = i128::from(1_000_000_i32) + i128::from(correction_ppm);
    let corrected = duration
        .checked_mul(scale)
        .ok_or(AudioError::ScheduleOverflow)?
        / i128::from(1_000_000_i32)
        / i128::from(sample_rate);
    u64::try_from(corrected).map_err(|_| AudioError::ScheduleOverflow)
}
