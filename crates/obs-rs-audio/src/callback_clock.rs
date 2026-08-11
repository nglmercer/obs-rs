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
    /// Running `delivered_frames * 1e9 * (1e6 + correction_ppm)`.
    ///
    /// Each callback adds only its own contribution instead of re-deriving the
    /// product from the running frame total, which keeps the wide multiply out
    /// of the callback path. It is rebuilt from scratch whenever the correction
    /// changes, so the value stays bit-identical to the closed form.
    elapsed_numerator: i128,
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
            elapsed_numerator: 0,
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
        let elapsed = scale_elapsed_nanos(self.elapsed_numerator, self.format.sample_rate())?;
        let expected_timestamp = origin
            .checked_add(elapsed)
            .ok_or(AudioError::ScheduleOverflow)?;
        let drift = i128::from(timestamp.as_nanos())
            .saturating_sub(i128::from(expected_timestamp.as_nanos()));
        let delivered = u64::try_from(frames).map_err(|_| AudioError::ScheduleOverflow)?;
        self.delivered_frames = self
            .delivered_frames
            .checked_add(delivered)
            .ok_or(AudioError::ScheduleOverflow)?;
        self.elapsed_numerator = self
            .elapsed_numerator
            .checked_add(frame_numerator(delivered, self.correction_ppm)?)
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
        // The running numerator is only valid for one correction factor, so it
        // is rebuilt from the frame total whenever the correction changes.
        self.elapsed_numerator = (self.delivered_frames as i128)
            .saturating_mul(1_000_000_000)
            .saturating_mul(scale_for(correction_ppm));
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
/// Returns the numerator contribution of `frames` at one correction setting.
///
/// The value is `frames * 1e9 * (1e6 + correction_ppm)`; keeping the running sum
/// in this un-divided form is what makes the per-callback update a single add.
fn frame_numerator(frames: u64, correction_ppm: i32) -> Result<i128, AudioError> {
    i128::from(frames)
        .checked_mul(1_000_000_000)
        .and_then(|duration| duration.checked_mul(scale_for(correction_ppm)))
        .ok_or(AudioError::ScheduleOverflow)
}

/// Converts an accumulated numerator into corrected elapsed nanoseconds.
fn scale_elapsed_nanos(numerator: i128, sample_rate: u32) -> Result<u64, AudioError> {
    let corrected = numerator / i128::from(1_000_000_i32) / i128::from(sample_rate);
    u64::try_from(corrected).map_err(|_| AudioError::ScheduleOverflow)
}

/// Returns the parts-per-million scale factor for one correction setting.
const fn scale_for(correction_ppm: i32) -> i128 {
    1_000_000_i128 + correction_ppm as i128
}
