use obs_rs_audio::AudioClock;
use obs_rs_media::Timestamp;
use obs_rs_video::VideoClock;

use super::error::ClockRateError;
const CLOCK_PPM_SCALE: u64 = 1_000_000;

/// Maximum supported simulated device-clock error in parts per million.
pub const MAX_CLOCK_DRIFT_PPM: i32 = 500_000;

/// A validated clock-rate offset expressed in parts per million.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockRate {
    drift_ppm: i32,
    scale_ppm: u64,
}

impl ClockRate {
    /// Creates a rate offset bounded to [`MAX_CLOCK_DRIFT_PPM`].
    ///
    /// # Errors
    ///
    /// Returns [`ClockRateError::DriftOutOfRange`] when the requested offset is
    /// outside the safe positive-rate interval.
    #[allow(clippy::cast_lossless, reason = "unsigned_abs is at most i32::MAX")]
    pub const fn new(drift_ppm: i32) -> Result<Self, ClockRateError> {
        if drift_ppm < -MAX_CLOCK_DRIFT_PPM || drift_ppm > MAX_CLOCK_DRIFT_PPM {
            return Err(ClockRateError::DriftOutOfRange { ppm: drift_ppm });
        }
        let magnitude = drift_ppm.unsigned_abs() as u64;
        let scale_ppm = if drift_ppm < 0 {
            CLOCK_PPM_SCALE - magnitude
        } else {
            CLOCK_PPM_SCALE + magnitude
        };
        Ok(Self {
            drift_ppm,
            scale_ppm,
        })
    }

    /// Returns the signed rate offset in parts per million.
    #[must_use]
    pub const fn drift_ppm(self) -> i32 {
        self.drift_ppm
    }

    const fn scale(self) -> u64 {
        self.scale_ppm
    }

    fn observed_at(self, reference: Timestamp) -> Timestamp {
        let nanos = u128::from(reference.as_nanos()) * u128::from(self.scale())
            / u128::from(CLOCK_PPM_SCALE);
        Timestamp::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    fn reference_for(self, deadline: Timestamp) -> Timestamp {
        let numerator = u128::from(deadline.as_nanos()) * u128::from(CLOCK_PPM_SCALE);
        let scale = u128::from(self.scale());
        let reference = numerator.div_ceil(scale);
        Timestamp::from_nanos(u64::try_from(reference).unwrap_or(u64::MAX))
    }
}

/// A deterministic adapter that models independent audio and video device clocks.
///
/// The adapter advances one shared reference timeline whenever either domain waits,
/// then exposes each domain's independently scaled reading. This makes hardware
/// clock drift testable without an operating-system dependency while preserving the
/// [`AudioClock`] and [`VideoClock`] contracts used by production adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndependentMediaClock {
    reference_nanos: u64,
    audio_rate: ClockRate,
    video_rate: ClockRate,
}

impl IndependentMediaClock {
    /// Creates a two-domain clock from signed audio and video drift in ppm.
    ///
    /// # Errors
    ///
    /// Returns [`ClockRateError::DriftOutOfRange`] when either domain is outside
    /// the supported rate interval.
    pub fn new(audio_drift_ppm: i32, video_drift_ppm: i32) -> Result<Self, ClockRateError> {
        Ok(Self::with_rates(
            ClockRate::new(audio_drift_ppm)?,
            ClockRate::new(video_drift_ppm)?,
        ))
    }

    /// Creates a two-domain clock from already validated rates.
    #[must_use]
    pub const fn with_rates(audio_rate: ClockRate, video_rate: ClockRate) -> Self {
        Self {
            reference_nanos: 0,
            audio_rate,
            video_rate,
        }
    }

    /// Returns the configured audio-domain rate.
    #[must_use]
    pub const fn audio_rate(self) -> ClockRate {
        self.audio_rate
    }

    /// Returns the configured video-domain rate.
    #[must_use]
    pub const fn video_rate(self) -> ClockRate {
        self.video_rate
    }

    /// Returns the shared reference time used by the deterministic adapter.
    #[must_use]
    pub const fn reference_now(self) -> Timestamp {
        Timestamp::from_nanos(self.reference_nanos)
    }

    /// Returns the current audio-device clock reading.
    #[must_use]
    pub fn audio_now(self) -> Timestamp {
        self.audio_rate.observed_at(self.reference_now())
    }

    /// Returns the current video-device clock reading.
    #[must_use]
    pub fn video_now(self) -> Timestamp {
        self.video_rate.observed_at(self.reference_now())
    }

    fn wait_until(&mut self, rate: ClockRate, deadline: Timestamp) {
        let required = rate.reference_for(deadline).as_nanos();
        self.reference_nanos = self.reference_nanos.max(required);
    }
}

impl AudioClock for IndependentMediaClock {
    fn now(&self) -> Timestamp {
        self.audio_now()
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.wait_until(self.audio_rate, deadline);
    }
}

impl VideoClock for IndependentMediaClock {
    fn now(&self) -> Timestamp {
        self.video_now()
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.wait_until(self.video_rate, deadline);
    }
}
