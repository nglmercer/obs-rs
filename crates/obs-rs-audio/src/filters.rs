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
/// Lower bound of the OBS Compressor ratio in thousandths of a ratio.
pub const MIN_COMPRESSOR_RATIO_MILLI: u16 = 1_000;
/// Upper bound of the OBS Compressor ratio in thousandths of a ratio.
pub const MAX_COMPRESSOR_RATIO_MILLI: u16 = 32_000;
/// Lower bound of the OBS Compressor threshold in thousandths of a decibel.
pub const MIN_COMPRESSOR_THRESHOLD_DB_MILLI: i32 = -60_000;
/// Upper bound of the OBS Compressor threshold in thousandths of a decibel.
pub const MAX_COMPRESSOR_THRESHOLD_DB_MILLI: i32 = 0;
/// Lower bound of the OBS Compressor attack time in milliseconds.
pub const MIN_COMPRESSOR_ATTACK_MS: u16 = 1;
/// Upper bound of the OBS Compressor attack time in milliseconds.
pub const MAX_COMPRESSOR_ATTACK_MS: u16 = 500;
/// Lower bound of the OBS Compressor release time in milliseconds.
pub const MIN_COMPRESSOR_RELEASE_MS: u16 = 1;
/// Upper bound of the OBS Compressor release time in milliseconds.
pub const MAX_COMPRESSOR_RELEASE_MS: u16 = 1_000;
/// Lower bound of the OBS Compressor output gain in thousandths of a decibel.
pub const MIN_COMPRESSOR_OUTPUT_GAIN_DB_MILLI: i32 = -32_000;
/// Upper bound of the OBS Compressor output gain in thousandths of a decibel.
pub const MAX_COMPRESSOR_OUTPUT_GAIN_DB_MILLI: i32 = 32_000;
/// Lower bound of the OBS Expander ratio in thousandths of a ratio.
pub const MIN_EXPANDER_RATIO_MILLI: u16 = 1_000;
/// Upper bound of the OBS Expander ratio in thousandths of a ratio.
pub const MAX_EXPANDER_RATIO_MILLI: u16 = 20_000;
/// Lower bound of the OBS Expander threshold in thousandths of a decibel.
pub const MIN_EXPANDER_THRESHOLD_DB_MILLI: i32 = -60_000;
/// Upper bound of the OBS Expander threshold in thousandths of a decibel.
pub const MAX_EXPANDER_THRESHOLD_DB_MILLI: i32 = 0;
/// Lower bound of the OBS Expander attack time in milliseconds.
pub const MIN_EXPANDER_ATTACK_MS: u16 = 1;
/// Upper bound of the OBS Expander attack time in milliseconds.
pub const MAX_EXPANDER_ATTACK_MS: u16 = 100;
/// Lower bound of the OBS Expander release time in milliseconds.
pub const MIN_EXPANDER_RELEASE_MS: u16 = 1;
/// Upper bound of the OBS Expander release time in milliseconds.
pub const MAX_EXPANDER_RELEASE_MS: u16 = 1_000;
/// Lower bound of the OBS Expander output gain in thousandths of a decibel.
pub const MIN_EXPANDER_OUTPUT_GAIN_DB_MILLI: i32 = -32_000;
/// Upper bound of the OBS Expander output gain in thousandths of a decibel.
pub const MAX_EXPANDER_OUTPUT_GAIN_DB_MILLI: i32 = 32_000;
/// Lower bound of an OBS Noise Gate threshold in thousandths of a decibel.
pub const MIN_NOISE_GATE_THRESHOLD_DB_MILLI: i32 = -96_000;
/// Upper bound of an OBS Noise Gate threshold in thousandths of a decibel.
pub const MAX_NOISE_GATE_THRESHOLD_DB_MILLI: i32 = 0;
/// Smallest safe attack/release time for the bounded Noise Gate core.
pub const MIN_NOISE_GATE_TIME_MS: u16 = 1;
/// Largest OBS Noise Gate attack/hold/release time in milliseconds.
pub const MAX_NOISE_GATE_TIME_MS: u16 = 10_000;

const LIMITER_ATTACK_TIME_SECONDS: f32 = 0.001;
const LIMITER_SILENCE_DB: f32 = -120.0;
const NOISE_GATE_MIN_DECAY_HZ: f32 = 75.0;

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

/// A validated, stateful OBS-compatible compressor without sidechain input.
///
/// The current primitive uses the source's own interleaved signal as its
/// detector. A sidechain-capable graph needs an explicit bounded source
/// identity and synchronized audio queue, so it is intentionally not hidden
/// behind this local filter value.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioCompressor {
    ratio_milli: u16,
    threshold_db_milli: i32,
    attack_ms: u16,
    release_ms: u16,
    output_gain_db_milli: i32,
    slope: f32,
    output_gain: f32,
    envelope: f32,
    sample_rate: u32,
    attack_coefficient: f32,
    release_coefficient: f32,
}

impl AudioCompressor {
    /// Creates a compressor with OBS's bounded controls.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a control is outside the supported OBS
    /// range.
    #[allow(
        clippy::cast_precision_loss,
        reason = "fixed-point controls are converted once during construction"
    )]
    pub fn new(
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<Self, AudioError> {
        if !(MIN_COMPRESSOR_RATIO_MILLI..=MAX_COMPRESSOR_RATIO_MILLI).contains(&ratio_milli) {
            return Err(AudioError::InvalidCompressorRatio {
                milli_ratio: ratio_milli,
            });
        }
        if !(MIN_COMPRESSOR_THRESHOLD_DB_MILLI..=MAX_COMPRESSOR_THRESHOLD_DB_MILLI)
            .contains(&threshold_db_milli)
        {
            return Err(AudioError::InvalidCompressorThreshold {
                milli_db: threshold_db_milli,
            });
        }
        if !(MIN_COMPRESSOR_ATTACK_MS..=MAX_COMPRESSOR_ATTACK_MS).contains(&attack_ms) {
            return Err(AudioError::InvalidCompressorAttack {
                milliseconds: attack_ms,
            });
        }
        if !(MIN_COMPRESSOR_RELEASE_MS..=MAX_COMPRESSOR_RELEASE_MS).contains(&release_ms) {
            return Err(AudioError::InvalidCompressorRelease {
                milliseconds: release_ms,
            });
        }
        if !(MIN_COMPRESSOR_OUTPUT_GAIN_DB_MILLI..=MAX_COMPRESSOR_OUTPUT_GAIN_DB_MILLI)
            .contains(&output_gain_db_milli)
        {
            return Err(AudioError::InvalidCompressorOutputGain {
                milli_db: output_gain_db_milli,
            });
        }
        let ratio = f32::from(ratio_milli) / 1_000.0;
        let slope = 1.0 - (1.0 / ratio);
        let output_gain = 10.0_f32.powf(output_gain_db_milli as f32 / 20_000.0);
        Ok(Self {
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
            slope,
            output_gain,
            envelope: 0.0,
            sample_rate: 0,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
        })
    }

    /// Returns the ratio in thousandths of a ratio.
    #[must_use]
    pub const fn ratio_milli(&self) -> u16 {
        self.ratio_milli
    }

    /// Returns the threshold in thousandths of a decibel.
    #[must_use]
    pub const fn threshold_db_milli(&self) -> i32 {
        self.threshold_db_milli
    }

    /// Returns the attack time in milliseconds.
    #[must_use]
    pub const fn attack_ms(&self) -> u16 {
        self.attack_ms
    }

    /// Returns the release time in milliseconds.
    #[must_use]
    pub const fn release_ms(&self) -> u16 {
        self.release_ms
    }

    /// Returns the output gain in thousandths of a decibel.
    #[must_use]
    pub const fn output_gain_db_milli(&self) -> i32 {
        self.output_gain_db_milli
    }

    /// Applies the compressor in place without allocating or changing
    /// timestamps.
    ///
    /// A read-only preflight computes the complete envelope and checks output
    /// finiteness before the second pass mutates samples. This preserves the
    /// audio buffer contract even when positive output gain would overflow.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FilterOverflow`] and leaves the samples and
    /// envelope unchanged when the configured output gain would be non-finite.
    pub fn apply(&mut self, buffer: &mut AudioBuffer) -> Result<(), AudioError> {
        self.configure_for_sample_rate(buffer.format().sample_rate());
        let channels = usize::from(buffer.format().channels());
        let threshold_db = self.threshold_db();
        let mut envelope = self.envelope;
        for frame in buffer.samples().chunks_exact(channels) {
            envelope = next_envelope(
                frame,
                envelope,
                self.attack_coefficient,
                self.release_coefficient,
            );
            let gain = compressor_gain(envelope, threshold_db, self.slope, self.output_gain);
            if frame.iter().any(|sample| !(*sample * gain).is_finite()) {
                return Err(AudioError::FilterOverflow);
            }
        }

        envelope = self.envelope;
        for frame in buffer.samples_mut().chunks_exact_mut(channels) {
            envelope = next_envelope(
                frame,
                envelope,
                self.attack_coefficient,
                self.release_coefficient,
            );
            let gain = compressor_gain(envelope, threshold_db, self.slope, self.output_gain);
            for sample in frame {
                *sample *= gain;
            }
        }
        self.envelope = envelope;
        Ok(())
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
        self.attack_coefficient =
            (-1.0 / (sample_rate_f32 * f32::from(self.attack_ms) / 1_000.0)).exp();
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

fn next_envelope(
    frame: &[f32],
    previous_envelope: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
) -> f32 {
    let input_peak = frame
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if previous_envelope < input_peak {
        input_peak + attack_coefficient * (previous_envelope - input_peak)
    } else {
        input_peak + release_coefficient * (previous_envelope - input_peak)
    }
}

#[allow(
    clippy::many_single_char_names,
    reason = "the formula mirrors OBS's compact compressor gain equation"
)]
fn compressor_gain(envelope: f32, threshold_db: f32, slope: f32, output_gain: f32) -> f32 {
    let envelope_db = if envelope > 0.0 {
        20.0 * envelope.log10()
    } else {
        LIMITER_SILENCE_DB
    };
    let gain_db = (slope * (threshold_db - envelope_db)).min(0.0);
    10.0_f32.powf(gain_db / 20.0) * output_gain
}

/// A validated, stateful OBS-compatible Noise Gate.
///
/// The detector and envelope follow OBS's native peak gate: the maximum
/// absolute sample across channels opens the gate above the open threshold,
/// the decaying level closes it below the close threshold, and attack/hold /
/// release shape the linear attenuation. The state lives in this filter value,
/// so adjacent audio blocks remain continuous without a second runtime store.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioNoiseGate {
    open_threshold_db_milli: i32,
    close_threshold_db_milli: i32,
    attack_ms: u16,
    hold_ms: u16,
    release_ms: u16,
    open_threshold: f32,
    close_threshold: f32,
    decay_rate: f32,
    attack_rate: f32,
    release_rate: f32,
    hold_time: f32,
    sample_rate: u32,
    sample_rate_i: f32,
    is_open: bool,
    attenuation: f32,
    level: f32,
    held_time: f32,
}

impl AudioNoiseGate {
    /// Creates a Noise Gate with OBS's bounded threshold and timing controls.
    ///
    /// OBS exposes zero as the slider minimum for timing controls, but its
    /// native rate calculation divides by that value. The safe Rust boundary
    /// therefore starts at one millisecond and keeps the real-time path finite.
    ///
    /// # Errors
    ///
    /// Returns a typed validation error when a control is outside the bounded
    /// range or when the close threshold is above the open threshold.
    pub fn new(
        open_threshold_db_milli: i32,
        close_threshold_db_milli: i32,
        attack_ms: u16,
        hold_ms: u16,
        release_ms: u16,
    ) -> Result<Self, AudioError> {
        if !(MIN_NOISE_GATE_THRESHOLD_DB_MILLI..=MAX_NOISE_GATE_THRESHOLD_DB_MILLI)
            .contains(&open_threshold_db_milli)
        {
            return Err(AudioError::InvalidNoiseGateOpenThreshold {
                milli_db: open_threshold_db_milli,
            });
        }
        if !(MIN_NOISE_GATE_THRESHOLD_DB_MILLI..=MAX_NOISE_GATE_THRESHOLD_DB_MILLI)
            .contains(&close_threshold_db_milli)
        {
            return Err(AudioError::InvalidNoiseGateCloseThreshold {
                milli_db: close_threshold_db_milli,
            });
        }
        if close_threshold_db_milli > open_threshold_db_milli {
            return Err(AudioError::InvalidNoiseGateThresholdOrder {
                open_milli_db: open_threshold_db_milli,
                close_milli_db: close_threshold_db_milli,
            });
        }
        if !(MIN_NOISE_GATE_TIME_MS..=MAX_NOISE_GATE_TIME_MS).contains(&attack_ms) {
            return Err(AudioError::InvalidNoiseGateAttack {
                milliseconds: attack_ms,
            });
        }
        if hold_ms > MAX_NOISE_GATE_TIME_MS {
            return Err(AudioError::InvalidNoiseGateHold {
                milliseconds: hold_ms,
            });
        }
        if !(MIN_NOISE_GATE_TIME_MS..=MAX_NOISE_GATE_TIME_MS).contains(&release_ms) {
            return Err(AudioError::InvalidNoiseGateRelease {
                milliseconds: release_ms,
            });
        }

        Ok(Self {
            open_threshold_db_milli,
            close_threshold_db_milli,
            attack_ms,
            hold_ms,
            release_ms,
            open_threshold: db_to_multiplier(open_threshold_db_milli),
            close_threshold: db_to_multiplier(close_threshold_db_milli),
            decay_rate: 0.0,
            attack_rate: 0.0,
            release_rate: 0.0,
            hold_time: f32::from(hold_ms) / 1_000.0,
            sample_rate: 0,
            sample_rate_i: 0.0,
            is_open: false,
            attenuation: 0.0,
            level: 0.0,
            held_time: 0.0,
        })
    }

    /// Returns the open threshold in thousandths of a decibel.
    #[must_use]
    pub const fn open_threshold_db_milli(&self) -> i32 {
        self.open_threshold_db_milli
    }

    /// Returns the close threshold in thousandths of a decibel.
    #[must_use]
    pub const fn close_threshold_db_milli(&self) -> i32 {
        self.close_threshold_db_milli
    }

    /// Returns the attack time in milliseconds.
    #[must_use]
    pub const fn attack_ms(&self) -> u16 {
        self.attack_ms
    }

    /// Returns the hold time in milliseconds.
    #[must_use]
    pub const fn hold_ms(&self) -> u16 {
        self.hold_ms
    }

    /// Returns the release time in milliseconds.
    #[must_use]
    pub const fn release_ms(&self) -> u16 {
        self.release_ms
    }

    /// Applies the peak gate in place without allocating or changing timestamps.
    pub fn apply(&mut self, buffer: &mut AudioBuffer) {
        self.configure_for_sample_rate(buffer.format().sample_rate());
        let channels = usize::from(buffer.format().channels());
        for frame in buffer.samples_mut().chunks_exact_mut(channels) {
            let current_level = frame
                .iter()
                .fold(0.0_f32, |level, sample| level.max(sample.abs()));

            if current_level > self.open_threshold && !self.is_open {
                self.is_open = true;
            }
            if self.level < self.close_threshold && self.is_open {
                self.held_time = 0.0;
                self.is_open = false;
            }

            self.level = self.level.max(current_level) - self.decay_rate;
            if self.is_open {
                self.attenuation = (self.attenuation + self.attack_rate).min(1.0);
            } else {
                self.held_time += self.sample_rate_i;
                if self.held_time > self.hold_time {
                    self.attenuation = (self.attenuation - self.release_rate).max(0.0);
                }
            }

            for sample in frame {
                *sample *= self.attenuation;
            }
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn configure_for_sample_rate(&mut self, sample_rate: u32) {
        if self.sample_rate == sample_rate {
            return;
        }
        let sample_rate_f32 = sample_rate as f32;
        self.sample_rate_i = 1.0 / sample_rate_f32;
        self.attack_rate = 1.0 / (f32::from(self.attack_ms) / 1_000.0 * sample_rate_f32);
        self.release_rate = 1.0 / (f32::from(self.release_ms) / 1_000.0 * sample_rate_f32);
        self.decay_rate = (self.open_threshold - self.close_threshold)
            / (sample_rate_f32 / NOISE_GATE_MIN_DECAY_HZ);
        self.sample_rate = sample_rate;
        self.is_open = false;
        self.attenuation = 0.0;
        self.level = 0.0;
        self.held_time = 0.0;
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "the fixed-point threshold is converted once when the filter is created"
)]
fn db_to_multiplier(milli_db: i32) -> f32 {
    10.0_f32.powf(milli_db as f32 / 20_000.0)
}

/// A validated, stateful OBS-compatible peak expander without gate presets,
/// RMS detection, knee shaping, or sidechain input.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioExpander {
    ratio_milli: u16,
    threshold_db_milli: i32,
    attack_ms: u16,
    release_ms: u16,
    output_gain_db_milli: i32,
    slope: f32,
    output_gain: f32,
    gain_db: f32,
    sample_rate: u32,
    attack_coefficient: f32,
    release_coefficient: f32,
}

impl AudioExpander {
    /// Creates a peak-detecting expander with OBS's bounded controls.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a control is outside the supported OBS
    /// range.
    #[allow(
        clippy::cast_precision_loss,
        reason = "fixed-point controls are converted once during construction"
    )]
    pub fn new(
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<Self, AudioError> {
        if !(MIN_EXPANDER_RATIO_MILLI..=MAX_EXPANDER_RATIO_MILLI).contains(&ratio_milli) {
            return Err(AudioError::InvalidExpanderRatio {
                milli_ratio: ratio_milli,
            });
        }
        if !(MIN_EXPANDER_THRESHOLD_DB_MILLI..=MAX_EXPANDER_THRESHOLD_DB_MILLI)
            .contains(&threshold_db_milli)
        {
            return Err(AudioError::InvalidExpanderThreshold {
                milli_db: threshold_db_milli,
            });
        }
        if !(MIN_EXPANDER_ATTACK_MS..=MAX_EXPANDER_ATTACK_MS).contains(&attack_ms) {
            return Err(AudioError::InvalidExpanderAttack {
                milliseconds: attack_ms,
            });
        }
        if !(MIN_EXPANDER_RELEASE_MS..=MAX_EXPANDER_RELEASE_MS).contains(&release_ms) {
            return Err(AudioError::InvalidExpanderRelease {
                milliseconds: release_ms,
            });
        }
        if !(MIN_EXPANDER_OUTPUT_GAIN_DB_MILLI..=MAX_EXPANDER_OUTPUT_GAIN_DB_MILLI)
            .contains(&output_gain_db_milli)
        {
            return Err(AudioError::InvalidExpanderOutputGain {
                milli_db: output_gain_db_milli,
            });
        }
        let ratio = f32::from(ratio_milli) / 1_000.0;
        let slope = 1.0 - ratio;
        let output_gain = 10.0_f32.powf(output_gain_db_milli as f32 / 20_000.0);
        Ok(Self {
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
            slope,
            output_gain,
            gain_db: 0.0,
            sample_rate: 0,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
        })
    }

    /// Returns the ratio in thousandths of a ratio.
    #[must_use]
    pub const fn ratio_milli(&self) -> u16 {
        self.ratio_milli
    }

    /// Returns the threshold in thousandths of a decibel.
    #[must_use]
    pub const fn threshold_db_milli(&self) -> i32 {
        self.threshold_db_milli
    }

    /// Returns the attack time in milliseconds.
    #[must_use]
    pub const fn attack_ms(&self) -> u16 {
        self.attack_ms
    }

    /// Returns the release time in milliseconds.
    #[must_use]
    pub const fn release_ms(&self) -> u16 {
        self.release_ms
    }

    /// Returns the output gain in thousandths of a decibel.
    #[must_use]
    pub const fn output_gain_db_milli(&self) -> i32 {
        self.output_gain_db_milli
    }

    /// Applies the peak expander in place without allocating or changing
    /// timestamps.
    ///
    /// Gain ballistics are tracked in dB, matching OBS's expander path. A
    /// read-only preflight checks output finiteness before the second pass
    /// commits samples and state.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FilterOverflow`] and leaves the samples and gain
    /// state unchanged when output gain would be non-finite.
    pub fn apply(&mut self, buffer: &mut AudioBuffer) -> Result<(), AudioError> {
        self.configure_for_sample_rate(buffer.format().sample_rate());
        let channels = usize::from(buffer.format().channels());
        let threshold_db = self.threshold_db();
        let mut gain_db = self.gain_db;
        for frame in buffer.samples().chunks_exact(channels) {
            gain_db = next_expander_gain_db(
                frame,
                gain_db,
                threshold_db,
                self.slope,
                self.attack_coefficient,
                self.release_coefficient,
            );
            let gain = expander_gain(gain_db, self.output_gain);
            if frame.iter().any(|sample| !(*sample * gain).is_finite()) {
                return Err(AudioError::FilterOverflow);
            }
        }

        gain_db = self.gain_db;
        for frame in buffer.samples_mut().chunks_exact_mut(channels) {
            gain_db = next_expander_gain_db(
                frame,
                gain_db,
                threshold_db,
                self.slope,
                self.attack_coefficient,
                self.release_coefficient,
            );
            let gain = expander_gain(gain_db, self.output_gain);
            for sample in frame {
                *sample *= gain;
            }
        }
        self.gain_db = gain_db;
        Ok(())
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
        self.attack_coefficient =
            (-1.0 / (sample_rate_f32 * f32::from(self.attack_ms) / 1_000.0)).exp();
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

fn next_expander_gain_db(
    frame: &[f32],
    previous_gain_db: f32,
    threshold_db: f32,
    slope: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
) -> f32 {
    let input_peak = frame
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let envelope_db = if input_peak > 0.0 {
        20.0 * input_peak.log10()
    } else {
        LIMITER_SILENCE_DB
    };
    let diff = threshold_db - envelope_db;
    let target_gain_db = if diff > 0.0 {
        (slope * diff).max(-60.0)
    } else {
        0.0
    };
    let coefficient = if target_gain_db > previous_gain_db {
        attack_coefficient
    } else {
        release_coefficient
    };
    coefficient * previous_gain_db + (1.0 - coefficient) * target_gain_db
}

fn expander_gain(gain_db: f32, output_gain: f32) -> f32 {
    10.0_f32.powf(gain_db / 20.0) * output_gain
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
