use super::{buffer::AudioBuffer, error::AudioError, types::AudioFormat};
use std::collections::VecDeque;

/// Maximum positive per-source delay accepted by the portable audio path.
pub const MAX_AUDIO_SYNC_OFFSET_MILLISECONDS: u32 = 5_000;

/// A bounded, sample-quantized delay line for one audio source.
///
/// The line prefixes the configured delay with silence, then emits captured
/// samples in order. Processing takes ownership of the input buffer and reuses
/// its sample allocation for the delayed output, so steady-state delay does
/// not allocate a second payload per audio block. The queue retains at most the
/// configured delay plus one input block.
pub struct AudioDelayLine {
    format: AudioFormat,
    delay_milliseconds: u32,
    delay_frames: usize,
    delay_samples: usize,
    block_frames: usize,
    samples: VecDeque<f32>,
}

impl AudioDelayLine {
    /// Creates a delay line with no block-size hint.
    ///
    /// The first processed block may grow the bounded queue to fit its size;
    /// callers with a fixed block size should prefer
    /// [`Self::with_block_frames`] so that capacity is established during
    /// control-plane setup.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidSyncOffset`] when the requested delay is
    /// outside the portable bound.
    pub fn new(format: AudioFormat, delay_milliseconds: u32) -> Result<Self, AudioError> {
        Self::with_block_frames(format, delay_milliseconds, 0)
    }

    /// Creates a delay line and reserves room for one fixed-size input block.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidSyncOffset`] when the requested delay is
    /// outside the portable bound.
    pub fn with_block_frames(
        format: AudioFormat,
        delay_milliseconds: u32,
        block_frames: usize,
    ) -> Result<Self, AudioError> {
        if delay_milliseconds > MAX_AUDIO_SYNC_OFFSET_MILLISECONDS {
            return Err(AudioError::InvalidSyncOffset {
                milliseconds: delay_milliseconds,
            });
        }
        let delay_frames = usize::try_from(
            u128::from(delay_milliseconds) * u128::from(format.sample_rate()) / 1_000,
        )
        .map_err(|_| AudioError::ScheduleOverflow)?;
        let channels = usize::from(format.channels());
        let delay_samples = delay_frames
            .checked_mul(channels)
            .ok_or(AudioError::ScheduleOverflow)?;
        let block_samples = block_frames
            .checked_mul(channels)
            .ok_or(AudioError::ScheduleOverflow)?;
        let capacity = delay_samples
            .checked_add(block_samples)
            .ok_or(AudioError::ScheduleOverflow)?;
        let mut samples = VecDeque::with_capacity(capacity);
        samples.resize(delay_samples, 0.0);
        Ok(Self {
            format,
            delay_milliseconds,
            delay_frames,
            delay_samples,
            block_frames,
            samples,
        })
    }

    /// Returns the format accepted by this delay line.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the configured user-facing delay.
    #[must_use]
    pub const fn delay_milliseconds(&self) -> u32 {
        self.delay_milliseconds
    }

    /// Returns the exact sample-frame delay after quantization.
    #[must_use]
    pub const fn delay_frames(&self) -> usize {
        self.delay_frames
    }

    /// Returns whether processing can use the input buffer unchanged.
    #[must_use]
    pub const fn is_passthrough(&self) -> bool {
        self.delay_frames == 0
    }

    /// Clears queued audio and restores the configured leading silence.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.samples.resize(self.delay_samples, 0.0);
    }

    /// Replaces the delay and clears already queued audio.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidSyncOffset`] when the new delay is outside
    /// the portable bound.
    pub fn set_delay_milliseconds(&mut self, delay_milliseconds: u32) -> Result<(), AudioError> {
        let replacement =
            Self::with_block_frames(self.format, delay_milliseconds, self.block_frames)?;
        *self = replacement;
        Ok(())
    }

    /// Delays one audio block while preserving its requested output timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FormatMismatch`] when the buffer format differs
    /// from this line's format.
    pub fn process(&mut self, mut input: AudioBuffer) -> Result<AudioBuffer, AudioError> {
        if input.format() != self.format {
            return Err(AudioError::FormatMismatch {
                expected: self.format,
                actual: input.format(),
            });
        }
        if self.is_passthrough() {
            return Ok(input);
        }

        let mut payload = input.take_samples();
        self.samples.reserve(payload.len());
        self.samples.extend(payload.iter().copied());
        for sample in &mut payload {
            *sample = self.samples.pop_front().unwrap_or(0.0);
        }
        input.replace_samples(payload);
        Ok(input)
    }
}
