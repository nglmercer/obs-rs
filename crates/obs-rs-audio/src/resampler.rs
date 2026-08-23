use super::{
    buffer::AudioBuffer,
    error::AudioError,
    types::{AudioChannelLayout, AudioFormat, MAX_AUDIO_FRAMES},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChannelRole {
    Left,
    Right,
    Center,
    Lfe,
    BackLeft,
    BackRight,
    SideLeft,
    SideRight,
}

const MONO_ROLES: [ChannelRole; 1] = [ChannelRole::Center];
const STEREO_ROLES: [ChannelRole; 2] = [ChannelRole::Left, ChannelRole::Right];
const TWO_POINT_ONE_ROLES: [ChannelRole; 3] =
    [ChannelRole::Left, ChannelRole::Right, ChannelRole::Lfe];
const QUAD_ROLES: [ChannelRole; 4] = [
    ChannelRole::Left,
    ChannelRole::Right,
    ChannelRole::BackLeft,
    ChannelRole::BackRight,
];
const FIVE_POINT_ONE_ROLES: [ChannelRole; 6] = [
    ChannelRole::Left,
    ChannelRole::Right,
    ChannelRole::Center,
    ChannelRole::Lfe,
    ChannelRole::SideLeft,
    ChannelRole::SideRight,
];
const SEVEN_POINT_ONE_ROLES: [ChannelRole; 8] = [
    ChannelRole::Left,
    ChannelRole::Right,
    ChannelRole::Center,
    ChannelRole::Lfe,
    ChannelRole::BackLeft,
    ChannelRole::BackRight,
    ChannelRole::SideLeft,
    ChannelRole::SideRight,
];
const LEFT_DOWNMIX_ROLES: [ChannelRole; 4] = [
    ChannelRole::Left,
    ChannelRole::Center,
    ChannelRole::BackLeft,
    ChannelRole::SideLeft,
];
const RIGHT_DOWNMIX_ROLES: [ChannelRole; 4] = [
    ChannelRole::Right,
    ChannelRole::Center,
    ChannelRole::BackRight,
    ChannelRole::SideRight,
];

fn standard_roles(layout: AudioChannelLayout) -> Option<&'static [ChannelRole]> {
    match layout {
        AudioChannelLayout::Mono => Some(&MONO_ROLES),
        AudioChannelLayout::Stereo => Some(&STEREO_ROLES),
        AudioChannelLayout::TwoPointOne => Some(&TWO_POINT_ONE_ROLES),
        AudioChannelLayout::Quad => Some(&QUAD_ROLES),
        AudioChannelLayout::FivePointOne => Some(&FIVE_POINT_ONE_ROLES),
        AudioChannelLayout::SevenPointOne => Some(&SEVEN_POINT_ONE_ROLES),
        AudioChannelLayout::Discrete(_) => None,
    }
}

fn exact_role_sample(samples: &[f32], roles: &[ChannelRole], role: ChannelRole) -> Option<f32> {
    roles
        .iter()
        .position(|candidate| *candidate == role)
        .and_then(|channel| samples.get(channel).copied())
}

fn average_roles(
    samples: &[f32],
    roles: &[ChannelRole],
    candidates: &[ChannelRole],
) -> Option<f32> {
    let mut total = 0.0;
    let mut count = 0_u16;
    for candidate in candidates {
        if let Some(sample) = exact_role_sample(samples, roles, *candidate) {
            total += sample;
            count = count.saturating_add(1);
        }
    }
    (count > 0).then_some(total / f32::from(count))
}

fn first_role_sample(
    samples: &[f32],
    roles: &[ChannelRole],
    candidates: &[ChannelRole],
) -> Option<f32> {
    candidates
        .iter()
        .find_map(|candidate| exact_role_sample(samples, roles, *candidate))
}

fn standard_role_sample(samples: &[f32], roles: &[ChannelRole], role: ChannelRole) -> f32 {
    if let Some(sample) = exact_role_sample(samples, roles, role) {
        return sample;
    }
    let fallback = match role {
        ChannelRole::Left => [
            ChannelRole::Center,
            ChannelRole::BackLeft,
            ChannelRole::SideLeft,
        ],
        ChannelRole::Right => [
            ChannelRole::Center,
            ChannelRole::BackRight,
            ChannelRole::SideRight,
        ],
        ChannelRole::Center => [ChannelRole::Left, ChannelRole::Right, ChannelRole::Center],
        ChannelRole::Lfe => return 0.0,
        ChannelRole::BackLeft => [
            ChannelRole::SideLeft,
            ChannelRole::Left,
            ChannelRole::Center,
        ],
        ChannelRole::BackRight => [
            ChannelRole::SideRight,
            ChannelRole::Right,
            ChannelRole::Center,
        ],
        ChannelRole::SideLeft => [
            ChannelRole::BackLeft,
            ChannelRole::Left,
            ChannelRole::Center,
        ],
        ChannelRole::SideRight => [
            ChannelRole::BackRight,
            ChannelRole::Right,
            ChannelRole::Center,
        ],
    };
    if role == ChannelRole::Center {
        average_roles(samples, roles, &fallback).unwrap_or(0.0)
    } else {
        first_role_sample(samples, roles, &fallback).unwrap_or(0.0)
    }
}

fn mapped_channel(
    samples: &[f32],
    input_layout: AudioChannelLayout,
    output_layout: AudioChannelLayout,
    output_channel: usize,
) -> f32 {
    let input_channels = usize::from(input_layout.channels());
    if output_layout == AudioChannelLayout::Mono {
        return samples.iter().sum::<f32>() / f32::from(input_layout.channels());
    }
    if input_layout == output_layout {
        return samples.get(output_channel).copied().unwrap_or(0.0);
    }

    let (Some(input_roles), Some(output_roles)) =
        (standard_roles(input_layout), standard_roles(output_layout))
    else {
        return samples
            .get(output_channel.min(input_channels.saturating_sub(1)))
            .copied()
            .unwrap_or(0.0);
    };
    let role = output_roles[output_channel];
    if matches!(
        output_layout,
        AudioChannelLayout::Stereo | AudioChannelLayout::TwoPointOne
    ) && matches!(role, ChannelRole::Left | ChannelRole::Right)
    {
        let candidates = if role == ChannelRole::Left {
            &LEFT_DOWNMIX_ROLES
        } else {
            &RIGHT_DOWNMIX_ROLES
        };
        return average_roles(samples, input_roles, candidates)
            .unwrap_or_else(|| standard_role_sample(samples, input_roles, role));
    }
    standard_role_sample(samples, input_roles, role)
}

/// A deterministic linear resampler and channel mapper for interleaved buffers.
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
    /// Standard layouts are mapped by speaker role: mono output averages all
    /// input channels, stereo/2.1 downmixes preserve left/right families, and
    /// wider standard layouts synthesize only missing roles from their nearest
    /// available speaker. Discrete layouts retain the bounded index-based
    /// fallback for provider formats without speaker metadata.
    pub const fn new(input: AudioFormat, output: AudioFormat) -> Result<Self, AudioError> {
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
        let input_channels = usize::from(self.input.channels);
        let output_channels = usize::from(self.output.channels);
        let mut samples = vec![0.0; output_frames * output_channels];

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

        for output_frame in samples.chunks_exact_mut(output_channels) {
            let base = usize::try_from(position / 1_000_000).unwrap_or(usize::MAX);
            let fraction = (position % 1_000_000) as f32 / 1_000_000.0;
            let first = base.min(last_frame);
            let second = (first + 1).min(last_frame);
            let first_base = first * input_channels;
            let second_base = second * input_channels;
            let first_samples = &input_samples[first_base..first_base + input_channels];
            let second_samples = &input_samples[second_base..second_base + input_channels];

            for (channel, output) in output_frame.iter_mut().enumerate() {
                let first_sample = mapped_channel(
                    first_samples,
                    self.input.layout(),
                    self.output.layout(),
                    channel,
                );
                let second_sample = mapped_channel(
                    second_samples,
                    self.input.layout(),
                    self.output.layout(),
                    channel,
                );
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
