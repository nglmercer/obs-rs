use super::{
    buffer::AudioBuffer,
    error::AudioError,
    monitor::AudioMonitorTap,
    types::{AudioFormat, AudioMonitorTapId, AudioSourceId, MAX_AUDIO_FRAMES},
};
use obs_rs_media::Timestamp;
use std::collections::{BTreeMap, BTreeSet};
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
    ) -> Result<Option<AudioBuffer>, AudioError> {
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
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        let channels = usize::from(self.format.channels);
        let sample_count = frames
            .checked_mul(channels)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let mut mixed = vec![0.0; sample_count];
        let mut seen = BTreeSet::new();

        for (source, buffer) in inputs {
            if !seen.insert(*source) {
                return Err(AudioError::DuplicateInput(*source));
            }
            let control = self
                .sources
                .get(source)
                .ok_or(AudioError::UnknownSource(*source))?;
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
            if control.muted {
                if let Some(control) = self.sources.get_mut(source) {
                    control.peak_milli = 0;
                }
                continue;
            }
            let mut peak = 0.0_f32;
            for (sample_index, (output, input)) in
                mixed.iter_mut().zip(buffer.samples()).enumerate()
            {
                let channel = sample_index % channels;
                let pan_gain = match channel {
                    0 => 1.0 - control.pan.max(0.0),
                    1 => 1.0 + control.pan.min(0.0),
                    _ => 1.0,
                };
                let contribution = *input * control.gain * pan_gain;
                *output += contribution;
                peak = peak.max(contribution.abs());
                if !output.is_finite() {
                    return Err(AudioError::MixOverflow);
                }
            }
            if let Some(control) = self.sources.get_mut(source) {
                control.peak_milli = peak_to_milli(peak);
            }
        }

        for sample in &mut mixed {
            *sample = sample.clamp(-1.0, 1.0);
        }
        let output = AudioBuffer::new(self.format, timestamp, mixed)?;
        for tap in self.monitor_taps.values_mut() {
            tap.observe(&output);
        }
        Ok(output)
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
