use super::{
    buffer::AudioBuffer,
    error::AudioError,
    types::{AudioFormat, MAX_AUDIO_FRAMES},
};
/// A deterministic linear resampler for interleaved buffers with equal channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioResampler {
    input: AudioFormat,
    output: AudioFormat,
}
impl AudioResampler {
    /// Creates a resampler between two sample rates.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ChannelMismatch`] when the formats do not have the
    /// same channel layout.
    pub const fn new(input: AudioFormat, output: AudioFormat) -> Result<Self, AudioError> {
        if input.channels != output.channels {
            return Err(AudioError::ChannelMismatch);
        }
        Ok(Self { input, output })
    }

    /// Converts one owned buffer with linear interpolation.
    ///
    /// The output frame count is rounded to the nearest integer. Empty input
    /// remains empty and timestamps are preserved at the beginning of the buffer.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FormatMismatch`] for a buffer with the wrong source
    /// format or [`AudioError::BufferTooLarge`] when the output exceeds the limit.
    #[allow(clippy::cast_precision_loss)]
    pub fn process(&self, input: &AudioBuffer) -> Result<AudioBuffer, AudioError> {
        if input.format() != self.input {
            return Err(AudioError::FormatMismatch {
                expected: self.input,
                actual: input.format(),
            });
        }
        if input.frames() == 0 {
            return AudioBuffer::silence(self.output, input.timestamp(), 0);
        }

        let output_frames = (input.frames() as u128 * u128::from(self.output.sample_rate)
            + u128::from(self.input.sample_rate) / 2)
            / u128::from(self.input.sample_rate);
        let output_frames = usize::try_from(output_frames)
            .ok()
            .filter(|frames| *frames <= MAX_AUDIO_FRAMES)
            .ok_or(AudioError::BufferTooLarge { frames: usize::MAX })?;
        let channels = usize::from(self.input.channels);
        let mut samples = vec![0.0; output_frames * channels];

        for output_frame in 0..output_frames {
            let position = output_frame as u128 * u128::from(self.input.sample_rate) * 1_000_000
                / u128::from(self.output.sample_rate);
            let base = usize::try_from(position / 1_000_000).unwrap_or(usize::MAX);
            let fraction = (position % 1_000_000) as f32 / 1_000_000.0;
            let first = base.min(input.frames() - 1);
            let second = (first + 1).min(input.frames() - 1);
            for channel in 0..channels {
                let first_sample = input.sample(first, channel).unwrap_or(0.0);
                let second_sample = input.sample(second, channel).unwrap_or(first_sample);
                samples[output_frame * channels + channel] =
                    first_sample + (second_sample - first_sample) * fraction;
            }
        }

        AudioBuffer::new(self.output, input.timestamp(), samples)
    }

    /// Returns the input format.
    #[must_use]
    pub const fn input_format(&self) -> AudioFormat {
        self.input
    }

    /// Returns the output format.
    #[must_use]
    pub const fn output_format(&self) -> AudioFormat {
        self.output
    }
}
