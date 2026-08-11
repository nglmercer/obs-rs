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

    /// Consumes the buffer and returns its owned interleaved samples.
    #[must_use]
    pub fn into_samples(self) -> Vec<f32> {
        self.samples
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

    /// Drops complete leading sample frames in place, advancing the timestamp.
    ///
    /// The owning buffer is reused, so unlike [`AudioBuffer::trim_front`] this
    /// performs no allocation. Returns `Ok(false)` and leaves the buffer
    /// untouched when the trim would consume every frame.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] when the new timestamp cannot be
    /// represented.
    pub fn trim_front_in_place(&mut self, frames: usize) -> Result<bool, AudioError> {
        if frames >= self.frames() {
            return Ok(false);
        }
        let timestamp = self
            .timestamp
            .checked_add(audio_duration_nanos(frames, self.format.sample_rate())?)
            .ok_or(AudioError::ScheduleOverflow)?;
        let channels = usize::from(self.format.channels);
        let start = frames
            .checked_mul(channels)
            .ok_or(AudioError::ScheduleOverflow)?;
        self.samples.drain(..start);
        self.timestamp = timestamp;
        Ok(true)
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
        // One exact allocation for the combined buffer instead of allocating the
        // silence prefix and then growing it with the payload.
        let mut samples = Vec::with_capacity(silence_samples + self.samples.len());
        samples.resize(silence_samples, 0.0);
        samples.extend_from_slice(&self.samples);
        Self::new(self.format, timestamp, samples).inspect(|buffer| {
            debug_assert_eq!(buffer.frames(), total_frames);
        })
    }

    /// Prefixes silence frames in place, reusing this buffer's allocation.
    ///
    /// Grows the existing buffer rather than building a second one, so it suits
    /// the A/V sync path where the corrected buffer replaces the original.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::BufferTooLarge`] if the combined buffer exceeds the
    /// reference limit.
    pub fn prepend_silence_in_place(
        &mut self,
        frames: usize,
        timestamp: Timestamp,
    ) -> Result<(), AudioError> {
        frames
            .checked_add(self.frames())
            .filter(|total| *total <= MAX_AUDIO_FRAMES)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let channels = usize::from(self.format.channels);
        let silence_samples = frames
            .checked_mul(channels)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let old_len = self.samples.len();
        self.samples.reserve(silence_samples);
        self.samples.resize(old_len + silence_samples, 0.0);
        self.samples.copy_within(..old_len, silence_samples);
        self.timestamp = timestamp;
        Ok(())
    }

    /// Replaces the timestamp without touching the sample payload.
    pub const fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp = timestamp;
    }

    /// Returns the interleaved samples for in-place mixing.
    ///
    /// Crate-internal: the caller is responsible for restoring the finiteness
    /// invariant that [`AudioBuffer::new`] enforces before the buffer escapes.
    pub(crate) fn samples_mut(&mut self) -> &mut [f32] {
        &mut self.samples
    }
}

/// Reuses interleaved sample allocations for a fixed audio format.
///
/// A queue deliberately owns buffers that are still scheduled for playback,
/// so recycling happens at the producer/consumer boundary: acquire a buffer,
/// fill it, submit it, then release it after the consumer is done. Keeping the
/// pool explicit avoids hidden ownership or synchronization costs in
/// [`super::queue::AudioQueue`].
pub struct AudioBufferPool {
    format: AudioFormat,
    capacity: usize,
    buffers: Vec<Vec<f32>>,
}

impl AudioBufferPool {
    /// Creates an empty pool that retains at most `capacity` sample vectors.
    #[must_use]
    pub fn new(format: AudioFormat, capacity: usize) -> Self {
        Self {
            format,
            capacity,
            buffers: Vec::with_capacity(capacity),
        }
    }

    /// Acquires a zeroed buffer, reusing a retained allocation when possible.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::BufferTooLarge`] when the requested frame count
    /// cannot be represented or exceeds the workspace limit.
    pub fn acquire(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioError> {
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        let samples = frames
            .checked_mul(usize::from(self.format.channels))
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let mut payload = self.buffers.pop().unwrap_or_default();
        payload.clear();
        payload.resize(samples, 0.0);
        Ok(AudioBuffer {
            format: self.format,
            timestamp,
            samples: payload,
        })
    }

    /// Returns a buffer's sample allocation to the pool.
    ///
    /// A buffer with a different format is rejected because its interleaving
    /// cannot safely be reused for this pool.
    pub fn release(&mut self, buffer: AudioBuffer) -> bool {
        if buffer.format != self.format || self.buffers.len() >= self.capacity {
            return false;
        }
        let mut payload = buffer.samples;
        payload.clear();
        self.buffers.push(payload);
        true
    }

    /// Returns the number of retained allocations available for reuse.
    #[must_use]
    pub fn available(&self) -> usize {
        self.buffers.len()
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
