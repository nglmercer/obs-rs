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

/// Stateful linear resampler for a continuous stream of interleaved buffers.
///
/// [`AudioResampler::process`] is intentionally a standalone-buffer
/// operation. Device adapters, however, receive a sequence of blocks and
/// must carry the fractional sample position and one frame of look-ahead
/// across those boundaries. This type keeps that state so a rate-converted
/// WASAPI route does not restart interpolation at every callback block.
#[derive(Debug)]
pub struct StreamingAudioResampler {
    resampler: AudioResampler,
    /// The last input frame is retained for interpolation across blocks.
    previous_frame: Option<Vec<f32>>,
    /// Number of input frames accepted by the stream.
    input_frames: u64,
    /// Rational input position of the next output sample, in units of the
    /// output sample rate.
    position: u64,
    /// Number of output frames already emitted.
    output_frames: u64,
    /// Timestamp corresponding to input frame zero.
    start_timestamp: Option<obs_rs_media::Timestamp>,
}

impl StreamingAudioResampler {
    /// Creates a stateful converter between two audio formats.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] when either format is invalid.
    pub fn new(input: AudioFormat, output: AudioFormat) -> Result<Self, AudioError> {
        Ok(Self {
            resampler: AudioResampler::new(input, output)?,
            previous_frame: None,
            input_frames: 0,
            position: 0,
            output_frames: 0,
            start_timestamp: None,
        })
    }

    /// Converts the next contiguous input block and preserves the fractional
    /// phase for the following block.
    ///
    /// A final input frame is held as look-ahead until the next call. Device
    /// adapters are live streams, so this avoids inventing an endpoint sample
    /// at every block boundary. Callers that need to flush a finite clip can
    /// append one final duplicate frame before the last call.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FormatMismatch`] for a wrong source format,
    /// [`AudioError::BufferTooLarge`] for an oversized result, or
    /// [`AudioError::ScheduleOverflow`] when the continuous sample clock is
    /// too large to represent.
    #[allow(clippy::cast_precision_loss)]
    pub fn process(&mut self, input: &AudioBuffer) -> Result<AudioBuffer, AudioError> {
        if input.format() != self.resampler.input {
            return Err(AudioError::FormatMismatch {
                expected: self.resampler.input,
                actual: input.format(),
            });
        }
        if input.frames() == 0 {
            let timestamp = self.start_timestamp.unwrap_or(input.timestamp());
            return AudioBuffer::silence(self.resampler.output, timestamp, 0);
        }

        let start_frame = self.input_frames;
        let input_frames =
            u64::try_from(input.frames()).map_err(|_| AudioError::ScheduleOverflow)?;
        let end_frame = start_frame
            .checked_add(input_frames)
            .ok_or(AudioError::ScheduleOverflow)?;
        let input_rate = u64::from(self.resampler.input.sample_rate);
        let output_rate = u64::from(self.resampler.output.sample_rate);
        let input_channels = usize::from(self.resampler.input.channels);
        let output_channels = usize::from(self.resampler.output.channels);
        let previous_frame = self.previous_frame.as_deref();
        let first_output_frame = self.output_frames;
        let mut samples = Vec::new();

        // The next output is usable only when both interpolation endpoints
        // have arrived. The final input frame becomes the retained endpoint
        // for the next block.
        while self.position / output_rate < end_frame.saturating_sub(1) {
            let source_index = self.position / output_rate;
            let fraction = (self.position % output_rate) as f32 / output_rate as f32;
            let first = frame_at(
                previous_frame,
                input,
                start_frame,
                source_index,
                input_channels,
            );
            let second = frame_at(
                previous_frame,
                input,
                start_frame,
                source_index.saturating_add(1),
                input_channels,
            );
            let (Some(first), Some(second)) = (first, second) else {
                // This is an internal stream invariant: all positions older
                // than the retained frame should already have been emitted.
                // Stop safely if a provider violates contiguity instead of
                // indexing outside the bounded input buffer.
                break;
            };
            for channel in 0..output_channels {
                let first_sample = mapped_channel(
                    first,
                    self.resampler.input.layout,
                    self.resampler.output.layout,
                    channel,
                );
                let second_sample = mapped_channel(
                    second,
                    self.resampler.input.layout,
                    self.resampler.output.layout,
                    channel,
                );
                samples.push(first_sample + (second_sample - first_sample) * fraction);
            }
            self.position = self
                .position
                .checked_add(input_rate)
                .ok_or(AudioError::ScheduleOverflow)?;
            self.output_frames = self
                .output_frames
                .checked_add(1)
                .ok_or(AudioError::ScheduleOverflow)?;
            if samples.len() / output_channels > MAX_AUDIO_FRAMES {
                return Err(AudioError::BufferTooLarge {
                    frames: samples.len() / output_channels,
                });
            }
        }

        self.input_frames = end_frame;
        let last_frame = input.frames() - 1;
        let last_start = last_frame
            .checked_mul(input_channels)
            .ok_or(AudioError::ScheduleOverflow)?;
        self.previous_frame =
            Some(input.samples()[last_start..last_start + input_channels].to_vec());
        let start_timestamp = *self.start_timestamp.get_or_insert(input.timestamp());
        let timestamp = sample_timestamp(
            start_timestamp,
            first_output_frame,
            self.resampler.output.sample_rate,
        )?;
        AudioBuffer::new(self.resampler.output, timestamp, samples)
    }

    /// Returns the source format accepted by this converter.
    #[must_use]
    pub const fn input_format(&self) -> AudioFormat {
        self.resampler.input
    }

    /// Returns the converted format emitted by this converter.
    #[must_use]
    pub const fn output_format(&self) -> AudioFormat {
        self.resampler.output
    }
}

fn frame_at<'a>(
    previous_frame: Option<&'a [f32]>,
    input: &'a AudioBuffer,
    start_frame: u64,
    index: u64,
    channels: usize,
) -> Option<&'a [f32]> {
    if index < start_frame {
        return if index.saturating_add(1) == start_frame {
            previous_frame
        } else {
            None
        };
    }
    let local = usize::try_from(index.checked_sub(start_frame)?).ok()?;
    let start = local.checked_mul(channels)?;
    input.samples().get(start..start.checked_add(channels)?)
}

fn sample_timestamp(
    start: obs_rs_media::Timestamp,
    output_frame: u64,
    sample_rate: u32,
) -> Result<obs_rs_media::Timestamp, AudioError> {
    let offset = output_frame
        .checked_mul(1_000_000_000_u64)
        .map(u128::from)
        .ok_or(AudioError::ScheduleOverflow)?
        / u128::from(sample_rate);
    let offset = u64::try_from(offset).map_err(|_| AudioError::ScheduleOverflow)?;
    start
        .checked_add(offset)
        .ok_or(AudioError::ScheduleOverflow)
}
