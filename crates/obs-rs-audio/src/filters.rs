//! Bounded, ordered audio filters for live source processing.

use super::{buffer::AudioBuffer, error::AudioError};

/// Maximum number of ordered filters on one audio source.
pub const MAX_AUDIO_FILTERS: usize = 32;

/// Lower bound of the OBS Gain filter in thousandths of a decibel.
pub const MIN_GAIN_DB_MILLI: i32 = -30_000;
/// Upper bound of the OBS Gain filter in thousandths of a decibel.
pub const MAX_GAIN_DB_MILLI: i32 = 30_000;
/// Lower bound of the OBS Limiter threshold in thousandths of a decibel.
pub const MIN_LIMITER_THRESHOLD_DB_MILLI: i32 = -60_000;
/// Upper bound of the OBS Limiter threshold in thousandths of a decibel.
pub const MAX_LIMITER_THRESHOLD_DB_MILLI: i32 = 0;
/// Lower bound of the OBS Limiter release time in milliseconds.
pub const MIN_LIMITER_RELEASE_MS: u16 = 1;
/// Upper bound of the OBS Limiter release time in milliseconds.
pub const MAX_LIMITER_RELEASE_MS: u16 = 1_000;

const LIMITER_ATTACK_TIME_SECONDS: f32 = 0.001;
const LIMITER_SILENCE_DB: f32 = -120.0;

/// A validated OBS-compatible audio gain value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioGain {
    milli_db: i32,
}

impl AudioGain {
    /// Creates a gain in thousandths of a decibel.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidFilterGain`] outside the bounded range
    /// exposed by OBS's Gain filter.
    pub const fn new(milli_db: i32) -> Result<Self, AudioError> {
        if milli_db < MIN_GAIN_DB_MILLI || milli_db > MAX_GAIN_DB_MILLI {
            return Err(AudioError::InvalidFilterGain { milli_db });
        }
        Ok(Self { milli_db })
    }

    /// Returns the gain in thousandths of a decibel.
    #[must_use]
    pub const fn milli_db(self) -> i32 {
        self.milli_db
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "the fixed-point UI value is intentionally converted once per block"
    )]
    fn multiplier(self) -> f32 {
        10.0_f32.powf(self.milli_db as f32 / 20_000.0)
    }
}

/// A validated, stateful OBS-compatible limiter configuration.
///
/// The envelope is deliberately kept with the filter instance so adjacent
/// audio blocks have continuous attack/release behavior without a side
/// allocation or a second runtime state store.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioLimiter {
    threshold_db_milli: i32,
    release_ms: u16,
    envelope: f32,
    sample_rate: u32,
    attack_coefficient: f32,
    release_coefficient: f32,
}

impl AudioLimiter {
    /// Creates a limiter with OBS's bounded threshold and release controls.
    ///
    /// The attack is the fixed 1 ms behavior used by OBS's native limiter;
    /// only threshold and release are user-facing settings here.
    ///
    /// # Errors
    ///
    /// Returns a validation error when either control is outside the bounded
    /// OBS-compatible range.
    pub const fn new(threshold_db_milli: i32, release_ms: u16) -> Result<Self, AudioError> {
        if threshold_db_milli < MIN_LIMITER_THRESHOLD_DB_MILLI
            || threshold_db_milli > MAX_LIMITER_THRESHOLD_DB_MILLI
        {
            return Err(AudioError::InvalidLimiterThreshold {
                milli_db: threshold_db_milli,
            });
        }
        if release_ms < MIN_LIMITER_RELEASE_MS || release_ms > MAX_LIMITER_RELEASE_MS {
            return Err(AudioError::InvalidLimiterRelease {
                milliseconds: release_ms,
            });
        }
        Ok(Self {
            threshold_db_milli,
            release_ms,
            envelope: 0.0,
            sample_rate: 0,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
        })
    }

    /// Returns the threshold in thousandths of a decibel.
    #[must_use]
    pub const fn threshold_db_milli(&self) -> i32 {
        self.threshold_db_milli
    }

    /// Returns the release time in milliseconds.
    #[must_use]
    pub const fn release_ms(&self) -> u16 {
        self.release_ms
    }

    /// Applies the limiter in place without allocating or changing timestamps.
    ///
    /// The envelope follows OBS's per-frame peak detector: rising levels use a
    /// fixed 1 ms attack and falling levels use the configured exponential
    /// release. A single gain is then applied to every channel in that frame,
    /// preserving inter-channel balance.
    pub fn apply(&mut self, buffer: &mut AudioBuffer) {
        self.configure_for_sample_rate(buffer.format().sample_rate());
        let channels = usize::from(buffer.format().channels());
        let threshold_db = self.threshold_db();
        for frame in buffer.samples_mut().chunks_exact_mut(channels) {
            let input_peak = frame
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            let previous_envelope = self.envelope;
            self.envelope = if input_peak > previous_envelope {
                input_peak + self.attack_coefficient * (previous_envelope - input_peak)
            } else {
                input_peak + self.release_coefficient * (previous_envelope - input_peak)
            };
            let gain = limiter_gain(self.envelope, threshold_db);
            for sample in frame {
                *sample *= gain;
            }
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "sample-rate conversion happens once when the live format changes"
    )]
    fn configure_for_sample_rate(&mut self, sample_rate: u32) {
        if self.sample_rate == sample_rate {
            return;
        }
        let sample_rate_f32 = sample_rate as f32;
        self.attack_coefficient = (-1.0 / (sample_rate_f32 * LIMITER_ATTACK_TIME_SECONDS)).exp();
        self.release_coefficient =
            (-1.0 / (sample_rate_f32 * f32::from(self.release_ms) / 1_000.0)).exp();
        self.sample_rate = sample_rate;
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "fixed-point settings are converted once per filter application"
    )]
    fn threshold_db(&self) -> f32 {
        self.threshold_db_milli as f32 / 1_000.0
    }
}

fn limiter_gain(envelope: f32, threshold_db: f32) -> f32 {
    let envelope_db = if envelope > 0.0 {
        20.0 * envelope.log10()
    } else {
        LIMITER_SILENCE_DB
    };
    let gain_db = (threshold_db - envelope_db).min(0.0);
    10.0_f32.powf(gain_db / 20.0)
}

/// One audio filter operation.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioFilter {
    /// Multiplies every channel by the same OBS dB gain.
    Gain(AudioGain),
    /// Inverts the polarity of every channel without changing its magnitude.
    InvertPolarity,
    /// Applies a stateful OBS-compatible peak limiter.
    Limiter(AudioLimiter),
}

impl AudioFilter {
    /// Creates a bounded Gain filter from thousandths of a decibel.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidFilterGain`] outside the supported range.
    pub const fn gain_db_milli(milli_db: i32) -> Result<Self, AudioError> {
        match AudioGain::new(milli_db) {
            Ok(gain) => Ok(Self::Gain(gain)),
            Err(error) => Err(error),
        }
    }

    /// Creates a bounded Limiter filter from threshold and release controls.
    ///
    /// # Errors
    ///
    /// Returns a validation error when either control is outside the supported
    /// OBS-compatible range.
    pub const fn limiter_db_milli(
        threshold_db_milli: i32,
        release_ms: u16,
    ) -> Result<Self, AudioError> {
        match AudioLimiter::new(threshold_db_milli, release_ms) {
            Ok(limiter) => Ok(Self::Limiter(limiter)),
            Err(error) => Err(error),
        }
    }

    /// Applies the filter in place without allocating or changing timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FilterOverflow`] when the finite input could not
    /// remain finite after multiplication. The input is left unchanged in that
    /// case.
    pub fn apply(&mut self, buffer: &mut AudioBuffer) -> Result<(), AudioError> {
        match self {
            Self::Gain(gain) => apply_gain(buffer, *gain),
            Self::InvertPolarity => {
                apply_invert_polarity(buffer);
                Ok(())
            }
            Self::Limiter(limiter) => {
                limiter.apply(buffer);
                Ok(())
            }
        }
    }
}

/// A fixed-capacity ordered chain of audio filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioFilterChain {
    filters: Vec<AudioFilter>,
}

impl AudioFilterChain {
    /// Creates an empty chain with its fixed capacity reserved up front.
    #[must_use]
    pub fn new() -> Self {
        Self {
            filters: Vec::with_capacity(MAX_AUDIO_FILTERS),
        }
    }

    /// Appends one filter in processing order.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FilterChainFull`] when the fixed chain capacity is
    /// reached.
    pub fn try_push(&mut self, filter: AudioFilter) -> Result<(), AudioError> {
        if self.filters.len() >= MAX_AUDIO_FILTERS {
            return Err(AudioError::FilterChainFull {
                max: MAX_AUDIO_FILTERS,
            });
        }
        self.filters.push(filter);
        Ok(())
    }

    /// Removes every filter while retaining the bounded allocation.
    pub fn clear(&mut self) {
        self.filters.clear();
    }

    /// Returns the current ordered filters.
    #[must_use]
    pub fn filters(&self) -> &[AudioFilter] {
        &self.filters
    }

    /// Returns the number of filters in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    /// Returns whether the chain has no filters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// Applies all filters in order without allocating per audio block.
    ///
    /// A later filter may observe the output of an earlier filter, matching
    /// OBS's ordered source-filter semantics.
    ///
    /// # Errors
    ///
    /// Returns the first [`AudioError`] raised by a filter, leaving the
    /// failing filter's input unchanged.
    pub fn apply(&mut self, buffer: &mut AudioBuffer) -> Result<(), AudioError> {
        for filter in &mut self.filters {
            filter.apply(buffer)?;
        }
        Ok(())
    }
}

fn apply_gain(buffer: &mut AudioBuffer, gain: AudioGain) -> Result<(), AudioError> {
    let multiplier = gain.multiplier();
    let maximum = buffer
        .samples()
        .iter()
        .fold(0.0_f32, |maximum, sample| maximum.max(sample.abs()));
    if !(maximum * multiplier).is_finite() {
        return Err(AudioError::FilterOverflow);
    }
    for sample in buffer.samples_mut() {
        *sample *= multiplier;
    }
    Ok(())
}

fn apply_invert_polarity(buffer: &mut AudioBuffer) {
    for sample in buffer.samples_mut() {
        *sample = -*sample;
    }
}
