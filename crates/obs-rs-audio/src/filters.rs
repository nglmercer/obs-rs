//! Bounded, ordered audio filters for live source processing.

use super::{buffer::AudioBuffer, error::AudioError};

/// Maximum number of ordered filters on one audio source.
pub const MAX_AUDIO_FILTERS: usize = 32;

/// Lower bound of the OBS Gain filter in thousandths of a decibel.
pub const MIN_GAIN_DB_MILLI: i32 = -30_000;
/// Upper bound of the OBS Gain filter in thousandths of a decibel.
pub const MAX_GAIN_DB_MILLI: i32 = 30_000;

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

/// One audio filter operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioFilter {
    /// Multiplies every channel by the same OBS dB gain.
    Gain(AudioGain),
    /// Inverts the polarity of every channel without changing its magnitude.
    InvertPolarity,
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

    /// Applies the filter in place without allocating or changing timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FilterOverflow`] when the finite input could not
    /// remain finite after multiplication. The input is left unchanged in that
    /// case.
    pub fn apply(self, buffer: &mut AudioBuffer) -> Result<(), AudioError> {
        match self {
            Self::Gain(gain) => apply_gain(buffer, gain),
            Self::InvertPolarity => {
                apply_invert_polarity(buffer);
                Ok(())
            }
        }
    }
}

/// A fixed-capacity ordered chain of audio filters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
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
    pub fn apply(&self, buffer: &mut AudioBuffer) -> Result<(), AudioError> {
        for filter in &self.filters {
            (*filter).apply(buffer)?;
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
