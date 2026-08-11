use super::{
    buffer::AudioBuffer,
    error::AudioError,
    monitor::AudioMonitorTap,
    types::{AudioFormat, AudioMonitorTapId, AudioSourceId, MAX_AUDIO_FRAMES},
};
use obs_rs_media::Timestamp;
use std::{collections::BTreeMap, sync::Arc};
struct SourceControl {
    gain: f32,
    muted: bool,
    pan: f32,
    peak_milli: u16,
}

/// A deterministic mixer for registered audio sources.
pub struct AudioMixer {
    format: AudioFormat,
    sources: BTreeMap<AudioSourceId, SourceControl>,
    monitor_taps: BTreeMap<AudioMonitorTapId, AudioMonitorTap>,
    next_source_id: u64,
    next_monitor_tap_id: u64,
}

impl AudioMixer {
    /// Creates an empty mixer for one audio format.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            sources: BTreeMap::new(),
            monitor_taps: BTreeMap::new(),
            next_source_id: 1,
            next_monitor_tap_id: 1,
        }
    }

    /// Registers a source with an initial linear gain.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidGain`] for a non-finite gain or
    /// [`AudioError::SourceIdExhausted`] when no new ID is available.
    pub fn add_source(&mut self, gain: f32) -> Result<AudioSourceId, AudioError> {
        if !gain.is_finite() {
            return Err(AudioError::InvalidGain);
        }
        let id = AudioSourceId(self.next_source_id);
        self.next_source_id = self
            .next_source_id
            .checked_add(1)
            .ok_or(AudioError::SourceIdExhausted)?;
        self.sources.insert(
            id,
            SourceControl {
                gain,
                muted: false,
                pan: 0.0,
                peak_milli: 0,
            },
        );
        Ok(id)
    }

    /// Updates a source's gain.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidGain`] for a non-finite value or
    /// [`AudioError::UnknownSource`] for an unknown source.
    pub fn set_gain(&mut self, source: AudioSourceId, gain: f32) -> Result<(), AudioError> {
        if !gain.is_finite() {
            return Err(AudioError::InvalidGain);
        }
        let control = self
            .sources
            .get_mut(&source)
            .ok_or(AudioError::UnknownSource(source))?;
        control.gain = gain;
        Ok(())
    }

    /// Mutes or unmutes one source.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownSource`] for an unknown source.
    pub fn set_muted(&mut self, source: AudioSourceId, muted: bool) -> Result<(), AudioError> {
        let control = self
            .sources
            .get_mut(&source)
            .ok_or(AudioError::UnknownSource(source))?;
        control.muted = muted;
        Ok(())
    }

    /// Sets a source's stereo pan, where `-1` is left, `0` is center, and `1` is
    /// right. Channels after the first two are left unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidPan`] for a non-finite or out-of-range value,
    /// or [`AudioError::UnknownSource`] for an unknown source.
    pub fn set_pan(&mut self, source: AudioSourceId, pan: f32) -> Result<(), AudioError> {
        if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
            return Err(AudioError::InvalidPan);
        }
        let control = self
            .sources
            .get_mut(&source)
            .ok_or(AudioError::UnknownSource(source))?;
        control.pan = pan;
        Ok(())
    }

    /// Removes a source from future mixes.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownSource`] for an unknown source.
    pub fn remove_source(&mut self, source: AudioSourceId) -> Result<(), AudioError> {
        self.sources
            .remove(&source)
            .map(|_| ())
            .ok_or(AudioError::UnknownSource(source))
    }

    /// Adds a bounded post-mix monitoring tap.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroMonitorCapacity`] for a zero capacity or
    /// [`AudioError::MonitorTapIdExhausted`] when no new ID is available.
    pub fn add_monitor_tap(
        &mut self,
        capacity_buffers: usize,
    ) -> Result<AudioMonitorTapId, AudioError> {
        let tap = AudioMonitorTap::new(capacity_buffers)?;
        let id = AudioMonitorTapId(self.next_monitor_tap_id);
        self.next_monitor_tap_id = self
            .next_monitor_tap_id
            .checked_add(1)
            .ok_or(AudioError::MonitorTapIdExhausted)?;
        self.monitor_taps.insert(id, tap);
        Ok(id)
    }

    /// Removes a monitoring tap and returns its retained buffers.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownMonitorTap`] for an unknown tap.
    pub fn remove_monitor_tap(
        &mut self,
        tap: AudioMonitorTapId,
    ) -> Result<AudioMonitorTap, AudioError> {
        self.monitor_taps
            .remove(&tap)
            .ok_or(AudioError::UnknownMonitorTap(tap))
    }

    /// Removes the oldest buffer from one monitoring tap.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownMonitorTap`] for an unknown tap.
    pub fn pop_monitor_buffer(
        &mut self,
        tap: AudioMonitorTapId,
    ) -> Result<Option<Arc<AudioBuffer>>, AudioError> {
        self.monitor_taps
            .get_mut(&tap)
            .ok_or(AudioError::UnknownMonitorTap(tap))
            .map(AudioMonitorTap::pop)
    }

    /// Returns the number of buffers dropped by one monitoring tap.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownMonitorTap`] for an unknown tap.
    pub fn monitor_dropped_buffers(&self, tap: AudioMonitorTapId) -> Result<u64, AudioError> {
        self.monitor_taps
            .get(&tap)
            .map(AudioMonitorTap::dropped_buffers)
            .ok_or(AudioError::UnknownMonitorTap(tap))
    }

    /// Mixes `inputs` into an owned output buffer and clamps it to `[-1.0, 1.0]`.
    ///
    /// Missing registered inputs contribute silence. Every supplied input must be
    /// registered, have the mixer format, and contain exactly `frames` frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] when an input is duplicated, unknown, mismatched, or
    /// has a different frame count, or when the sum becomes non-finite.
    pub fn mix(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
        inputs: &[(AudioSourceId, &AudioBuffer)],
    ) -> Result<AudioBuffer, AudioError> {
        let mut output = AudioBuffer::silence(self.format, timestamp, frames)?;
        self.mix_into(timestamp, &mut output, inputs)?;
        Ok(output)
    }

    /// Mixes into a shared output buffer and shares that same allocation with
    /// every monitor tap.
    ///
    /// Consumers that already pass mixed audio between components should prefer
    /// this form when monitor taps are enabled: unlike [`AudioMixer::mix`], it
    /// does not have to clone the returned buffer to preserve both ownership and
    /// tap visibility.
    ///
    /// # Errors
    ///
    /// Returns the same validation and overflow errors as [`AudioMixer::mix`].
    pub fn mix_shared(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
        inputs: &[(AudioSourceId, &AudioBuffer)],
    ) -> Result<Arc<AudioBuffer>, AudioError> {
        let mut output = AudioBuffer::silence(self.format, timestamp, frames)?;
        self.mix_core(timestamp, &mut output, inputs)?;
        let snapshot = Arc::new(output);
        for tap in self.monitor_taps.values_mut() {
            tap.observe(&snapshot);
        }
        Ok(snapshot)
    }

    /// Mixes `inputs` into a caller-owned buffer, clamping to `[-1.0, 1.0]`.
    ///
    /// This is the allocation-free form of [`AudioMixer::mix`] and the one
    /// suitable for an audio callback: `output` supplies the storage, so the
    /// mix itself performs no allocation, takes no locks, and copies no buffer.
    /// Monitor taps, when registered, are the one exception — each observation
    /// retains a reference-counted snapshot, which costs one small allocation
    /// per mix regardless of tap count.
    ///
    /// `output` must already carry the mixer format and the requested frame
    /// count; its existing contents are overwritten. Its timestamp is set to
    /// `timestamp`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] when `output` does not match the mixer format or
    /// frame count, when an input is duplicated, unknown, mismatched, or has a
    /// different frame count, or when the sum becomes non-finite.
    pub fn mix_into(
        &mut self,
        timestamp: Timestamp,
        output: &mut AudioBuffer,
        inputs: &[(AudioSourceId, &AudioBuffer)],
    ) -> Result<(), AudioError> {
        self.mix_core(timestamp, output, inputs)?;

        if !self.monitor_taps.is_empty() {
            // A caller-owned output cannot be moved into an Arc without changing
            // this method's ownership contract. The shared form above lets
            // callback users avoid this compatibility copy entirely.
            let snapshot = Arc::new(output.clone());
            for tap in self.monitor_taps.values_mut() {
                tap.observe(&snapshot);
            }
        }
        Ok(())
    }

    fn mix_core(
        &mut self,
        timestamp: Timestamp,
        output: &mut AudioBuffer,
        inputs: &[(AudioSourceId, &AudioBuffer)],
    ) -> Result<(), AudioError> {
        if output.format() != self.format {
            return Err(AudioError::FormatMismatch {
                expected: self.format,
                actual: output.format(),
            });
        }
        let frames = output.frames();
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        let channels = usize::from(self.format.channels);

        // Validate every input before mutating `output`, so a rejected mix
        // leaves the caller's buffer and the source peaks unchanged.
        for (index, (source, buffer)) in inputs.iter().enumerate() {
            if inputs[..index]
                .iter()
                .any(|(previous, _)| previous == source)
            {
                return Err(AudioError::DuplicateInput(*source));
            }
            if !self.sources.contains_key(source) {
                return Err(AudioError::UnknownSource(*source));
            }
            if buffer.format() != self.format {
                return Err(AudioError::FormatMismatch {
                    expected: self.format,
                    actual: buffer.format(),
                });
            }
            if buffer.frames() != frames {
                return Err(AudioError::FrameCountMismatch {
                    expected: frames,
                    actual: buffer.frames(),
                });
            }
        }

        let mixed = output.samples_mut();
        mixed.fill(0.0);

        for (source, buffer) in inputs {
            let Some(control) = self.sources.get(source) else {
                return Err(AudioError::UnknownSource(*source));
            };
            let (gain, muted, pan) = (control.gain, control.muted, control.pan);
            if muted {
                if let Some(control) = self.sources.get_mut(source) {
                    control.peak_milli = 0;
                }
                continue;
            }

            // Pan is constant for the whole buffer, so the per-channel gains are
            // folded into the source gain once instead of being recomputed (with
            // a modulo) for every sample.
            let left_gain = gain * (1.0 - pan.max(0.0));
            let right_gain = gain * (1.0 + pan.min(0.0));

            for (output_frame, input_frame) in mixed
                .chunks_exact_mut(channels)
                .zip(buffer.samples().chunks_exact(channels))
            {
                for (channel, (output, input)) in
                    output_frame.iter_mut().zip(input_frame).enumerate()
                {
                    let gain = match channel {
                        0 => left_gain,
                        1 => right_gain,
                        _ => gain,
                    };
                    *output += *input * gain;
                }
            }

            // Peak bookkeeping is deliberately separate from the accumulation
            // loop so the hot mix pass has no finiteness branch or loop-carried
            // max dependency that would prevent vectorization.
            let peak = buffer
                .samples()
                .chunks_exact(channels)
                .flat_map(|input_frame| {
                    input_frame.iter().enumerate().map(|(channel, input)| {
                        let channel_gain = match channel {
                            0 => left_gain,
                            1 => right_gain,
                            _ => gain,
                        };
                        (*input * channel_gain).abs()
                    })
                })
                .fold(0.0_f32, f32::max);
            if let Some(control) = self.sources.get_mut(source) {
                control.peak_milli = peak_to_milli(peak);
            }
        }

        for sample in mixed.iter_mut() {
            if !sample.is_finite() {
                return Err(AudioError::MixOverflow);
            }
            *sample = sample.clamp(-1.0, 1.0);
        }
        output.set_timestamp(timestamp);
        Ok(())
    }

    /// Returns the mixer format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the number of registered sources.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Returns the latest bounded post-gain peak for one source.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownSource`] for an unknown source.
    pub fn source_peak_milli(&self, source: AudioSourceId) -> Result<u16, AudioError> {
        self.sources
            .get(&source)
            .map(|control| control.peak_milli)
            .ok_or(AudioError::UnknownSource(source))
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn peak_to_milli(peak: f32) -> u16 {
    let scaled = (peak.clamp(0.0, 1.0) * 1_000.0).round();
    u16::try_from(scaled as u32).unwrap_or(u16::MAX)
}
