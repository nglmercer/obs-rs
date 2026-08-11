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

        // Bresenham-style rational accumulator. `position` is the same value the
        // closed form `output_frame * input_rate * 1_000_000 / output_rate`
        // produces, but it is reached by addition: `step` is added each frame and
        // the remainder carries, so the per-frame 128-bit multiply, divide, and
        // modulo disappear from the loop.
        let denominator = u64::from(self.output.sample_rate);
        let step = u64::from(self.input.sample_rate) * 1_000_000;
        let whole_step = step / denominator;
        let remainder_step = step % denominator;
        let mut position = 0_u64;
        let mut remainder = 0_u64;

        let last_frame = input.frames() - 1;
        let input_samples = input.samples();

        for output_frame in samples.chunks_exact_mut(channels) {
            let base = usize::try_from(position / 1_000_000).unwrap_or(usize::MAX);
            let fraction = (position % 1_000_000) as f32 / 1_000_000.0;
            let first = base.min(last_frame);
            let second = (first + 1).min(last_frame);
            let first_base = first * channels;
            let second_base = second * channels;
            let first_samples = &input_samples[first_base..first_base + channels];
            let second_samples = &input_samples[second_base..second_base + channels];

            for (channel, output) in output_frame.iter_mut().enumerate() {
                let first_sample = first_samples[channel];
                let second_sample = second_samples[channel];
                *output = first_sample + (second_sample - first_sample) * fraction;
            }

            position += whole_step;
            remainder += remainder_step;
            if remainder >= denominator {
                remainder -= denominator;
                position += 1;
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
