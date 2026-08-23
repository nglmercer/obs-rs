use super::{
    AudioCompressor, AudioExpander, AudioGain, AudioLimiter, AudioNoiseGate, MAX_AUDIO_FILTERS,
};
use crate::{buffer::AudioBuffer, error::AudioError};

/// One audio filter operation.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioFilter {
    /// Multiplies every channel by the same OBS dB gain.
    Gain(AudioGain),
    /// Inverts the polarity of every channel without changing its magnitude.
    InvertPolarity,
    /// Applies a stateful OBS-compatible peak limiter.
    Limiter(AudioLimiter),
    /// Applies a stateful OBS-compatible compressor without a sidechain.
    Compressor(AudioCompressor),
    /// Applies a stateful OBS-compatible peak expander without a sidechain.
    Expander(AudioExpander),
    /// Applies OBS's stateful peak Noise Gate.
    NoiseGate(AudioNoiseGate),
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

    /// Creates a bounded Compressor filter from fixed-point controls.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a control is outside the supported
    /// OBS-compatible range.
    pub fn compressor(
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<Self, AudioError> {
        AudioCompressor::new(
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
        )
        .map(Self::Compressor)
    }

    /// Creates a bounded peak Expander filter from fixed-point controls.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a control is outside the supported
    /// OBS-compatible range.
    pub fn expander(
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<Self, AudioError> {
        AudioExpander::new(
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
        )
        .map(Self::Expander)
    }

    /// Creates OBS's stateful peak Noise Gate from threshold and timing controls.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a control is outside the supported
    /// OBS-compatible range.
    pub fn noise_gate(
        open_threshold_db_milli: i32,
        close_threshold_db_milli: i32,
        attack_ms: u16,
        hold_ms: u16,
        release_ms: u16,
    ) -> Result<Self, AudioError> {
        AudioNoiseGate::new(
            open_threshold_db_milli,
            close_threshold_db_milli,
            attack_ms,
            hold_ms,
            release_ms,
        )
        .map(Self::NoiseGate)
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
            Self::Compressor(compressor) => compressor.apply(buffer),
            Self::Expander(expander) => expander.apply(buffer),
            Self::NoiseGate(gate) => {
                gate.apply(buffer);
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
