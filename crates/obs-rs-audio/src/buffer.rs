use super::{
    error::AudioError,
    types::{AudioFormat, MAX_AUDIO_FRAMES},
};
use obs_rs_media::Timestamp;
/// An owned interleaved `f32` audio buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    format: AudioFormat,
    timestamp: Timestamp,
    samples: Vec<f32>,
}
impl AudioBuffer {
    /// Creates a buffer after checking interleaving and sample finiteness.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] when the sample count is not divisible by the channel
    /// count, a sample is non-finite, or the buffer exceeds the frame limit.
    pub fn new(
        format: AudioFormat,
        timestamp: Timestamp,
        samples: Vec<f32>,
    ) -> Result<Self, AudioError> {
        let channels = usize::from(format.channels);
        if !samples.len().is_multiple_of(channels) {
            return Err(AudioError::SamplesNotInterleaved {
                samples: samples.len(),
                channels: format.channels,
            });
        }
        let frames = samples.len() / channels;
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(AudioError::NonFiniteSample);
        }

        Ok(Self {
            format,
            timestamp,
            samples,
        })
    }

    /// Creates a silence buffer with `frames` interleaved frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::BufferTooLarge`] when `frames` exceeds the reference
    /// buffer limit.
    pub fn silence(
        format: AudioFormat,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<Self, AudioError> {
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        let sample_count = frames
            .checked_mul(usize::from(format.channels))
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let samples = vec![0.0; sample_count];
        Ok(Self {
            format,
            timestamp,
            samples,
        })
    }

    /// Returns the audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the first-sample timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns the number of interleaved audio frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels)
    }

    /// Returns the immutable interleaved sample slice.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Returns a sample if its frame and channel are in range.
    #[must_use]
    pub fn sample(&self, frame: usize, channel: usize) -> Option<f32> {
        if channel >= usize::from(self.format.channels) {
            return None;
        }
        self.samples
            .get(frame * usize::from(self.format.channels) + channel)
            .copied()
    }

    /// Returns the integer nanosecond duration represented by this buffer.
    #[must_use]
    pub fn duration_nanos(&self) -> Option<u64> {
        let duration = u128::try_from(self.frames())
            .ok()?
            .checked_mul(1_000_000_000)?
            / u128::from(self.format.sample_rate);
        u64::try_from(duration).ok()
    }

    /// Returns the exclusive timestamp at the end of this buffer.
    #[must_use]
    pub fn end_timestamp(&self) -> Option<Timestamp> {
        self.timestamp.checked_add(self.duration_nanos()?)
    }

    /// Drops complete leading sample frames and advances the timestamp.
    ///
    /// Returns `Ok(None)` when all frames were removed.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] when the new timestamp cannot be
    /// represented.
    pub fn trim_front(&self, frames: usize) -> Result<Option<Self>, AudioError> {
        if frames >= self.frames() {
            return Ok(None);
        }
        let timestamp = self
            .timestamp
            .checked_add(audio_duration_nanos(frames, self.format.sample_rate())?)
            .ok_or(AudioError::ScheduleOverflow)?;
        let channels = usize::from(self.format.channels);
        let start = frames
            .checked_mul(channels)
            .ok_or(AudioError::ScheduleOverflow)?;
        Ok(Some(Self::new(
            self.format,
            timestamp,
            self.samples[start..].to_vec(),
        )?))
    }

    /// Prefixes complete silence frames and assigns `timestamp` to the new buffer.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::BufferTooLarge`] if the combined buffer exceeds the
    /// reference limit.
    pub fn prepend_silence(&self, frames: usize, timestamp: Timestamp) -> Result<Self, AudioError> {
        let total_frames = frames
            .checked_add(self.frames())
            .filter(|total| *total <= MAX_AUDIO_FRAMES)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let channels = usize::from(self.format.channels);
        let silence_samples = frames
            .checked_mul(channels)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let mut samples = vec![0.0; silence_samples];
        samples.extend_from_slice(&self.samples);
        Self::new(self.format, timestamp, samples).inspect(|buffer| {
            debug_assert_eq!(buffer.frames(), total_frames);
        })
    }
}

fn audio_duration_nanos(frames: usize, sample_rate: u32) -> Result<u64, AudioError> {
    let duration = u128::try_from(frames)
        .ok()
        .and_then(|frames| frames.checked_mul(1_000_000_000))
        .ok_or(AudioError::ScheduleOverflow)?
        / u128::from(sample_rate);
    u64::try_from(duration).map_err(|_| AudioError::ScheduleOverflow)
}
