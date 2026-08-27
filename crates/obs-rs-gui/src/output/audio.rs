use std::{
    error::Error,
    time::{Duration, Instant},
};

use obs_rs_audio::{
    AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInputProvider, AudioMonitorMode,
};
use obs_rs_media::VideoFrame;

use super::{AudioInputEntry, AudioOutputEntry, OutputRuntime};

impl OutputRuntime {
    /// Samples live input for mixer meters while no output is encoding.
    pub(crate) fn monitor_audio(&self, frame: &VideoFrame) {
        let _ = self.worker.try_monitor_audio(frame.timestamp());
    }

    pub(crate) fn set_channel_gain_milli(
        &mut self,
        id: &str,
        gain_milli: u16,
    ) -> Result<(), Box<dyn Error>> {
        self.worker
            .set_channel_gain_milli(engine_channel(id), gain_milli)?;
        Ok(())
    }

    pub(crate) fn set_channel_pan_milli(
        &mut self,
        id: &str,
        pan_milli: i32,
    ) -> Result<(), Box<dyn Error>> {
        self.worker
            .set_channel_pan_milli(engine_channel(id), pan_milli)?;
        Ok(())
    }

    pub(crate) fn set_channel_muted(
        &mut self,
        id: &str,
        muted: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.worker.set_channel_muted(engine_channel(id), muted)?;
        Ok(())
    }

    /// Requests a live monitor-routing policy on one engine audio channel.
    pub(crate) fn set_channel_monitor_mode(
        &mut self,
        id: &str,
        mode: AudioMonitorMode,
    ) -> Result<(), Box<dyn Error>> {
        self.worker
            .set_channel_monitor_mode(engine_channel(id), mode)?;
        Ok(())
    }

    /// Requests a live microphone/input switch on the output worker.
    pub(crate) fn set_audio_input_id(
        &mut self,
        device_id: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        self.worker.set_audio_input_id(device_id)?;
        self.audio_input_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Ok(())
    }

    /// Rebuilds the worker-owned audio path for an idle format change.
    pub(crate) fn set_audio_format(&mut self, format: AudioFormat) -> Result<(), Box<dyn Error>> {
        if self.audio_format == format {
            return Ok(());
        }
        self.worker.set_audio_format(format)?;
        self.audio_format = format;
        Ok(())
    }

    /// Requests a bounded live synchronization-offset update on one audio
    /// channel. The worker owns the delay line, so this call never copies or
    /// blocks on audio data in the GUI thread.
    pub(crate) fn set_channel_sync_offset_millis(
        &mut self,
        id: &str,
        milliseconds: u32,
    ) -> Result<(), Box<dyn Error>> {
        self.worker
            .set_channel_sync_offset_millis(engine_channel(id), milliseconds)?;
        Ok(())
    }

    /// Selects or clears the asynchronous local monitor sink.
    pub(crate) fn set_monitor_output_id(
        &mut self,
        device_id: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        self.worker.set_monitor_output_id(device_id)?;
        self.monitor_output_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Ok(())
    }

    /// Returns the persisted/selected input ID, or an empty string for auto.
    #[cfg(test)]
    pub(crate) fn audio_input_id(&self) -> Option<&str> {
        self.audio_input_id.as_deref()
    }

    /// Returns the selected local monitor-output ID, or `None` when disabled.
    #[cfg(test)]
    pub(crate) fn monitor_output_id(&self) -> Option<&str> {
        self.monitor_output_id.as_deref()
    }

    /// Returns the live microphone meter tuple: current peak, held peak, and
    /// the bounded clip indication.
    pub(crate) fn input_meter(&self) -> (u16, u16, bool) {
        let stats = self.worker.snapshot().engine.stats;
        (
            stats.microphone_peak_milli,
            stats.microphone_peak_hold_milli,
            stats.microphone_clipped,
        )
    }

    /// Returns the live desktop-audio meter tuple: current peak, held peak,
    /// and the bounded clip indication.
    pub(crate) fn desktop_meter(&self) -> (u16, u16, bool) {
        let stats = self.worker.snapshot().engine.stats;
        (
            stats.desktop_peak_milli,
            stats.desktop_peak_hold_milli,
            stats.desktop_clipped,
        )
    }

    /// Returns whether the engine is running on the deterministic fallback
    /// generator instead of a real capture device.
    pub(crate) fn audio_is_fallback(&self) -> bool {
        self.worker.snapshot().engine.audio_fallback
    }

    /// Returns the display name of the selected input, for the mixer row.
    pub(crate) fn audio_input_name(&mut self) -> String {
        let Some(id) = self.audio_input_id.clone() else {
            return "Default input".to_owned();
        };
        self.discover_audio_devices()
            .ok()
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|device| device.id() == id)
                    .map(|device| device.name().to_owned())
            })
            .unwrap_or(id)
    }

    /// Returns the playback monitor the desktop channel captures, if any.
    ///
    /// `None` means the channel is genuinely silent, which the mixer row shows
    /// as such instead of naming a device that is not being read.
    pub(crate) fn desktop_audio_name(&self) -> Option<String> {
        match self.worker.snapshot().engine.desktop_audio {
            obs_rs_engine::DesktopAudioSource::Monitor(name) => Some(name),
            obs_rs_engine::DesktopAudioSource::Silent(_) => None,
        }
    }

    pub(crate) fn audio_devices_summary(&mut self) -> String {
        match self.discover_audio_devices() {
            Ok(devices) if devices.is_empty() => {
                format!(
                    "{}: no audio devices; deterministic fallback available",
                    super::AUDIO_BACKEND_LABEL
                )
            }
            Ok(devices) => devices
                .iter()
                .map(|device| {
                    let kind = match device.kind() {
                        AudioDeviceKind::Input => "input",
                        AudioDeviceKind::Output => "output",
                    };
                    let availability = if device.available() {
                        "ready"
                    } else {
                        "missing"
                    };
                    format!(
                        "{kind}: {} ({}) [{availability}]",
                        device.name(),
                        device.id()
                    )
                })
                .collect::<Vec<_>>()
                .join(" · "),
            Err(error) => {
                format!(
                    "{} unavailable: {error}; deterministic fallback available",
                    super::AUDIO_BACKEND_LABEL
                )
            }
        }
    }

    /// Returns discoverable platform input devices as `(stable_id, label)`.
    ///
    /// Discovery is cached briefly because opening Settings should not invoke
    /// `pw-dump` repeatedly while the user moves between fields.
    pub(crate) fn audio_input_devices(&mut self) -> Vec<(String, String)> {
        self.discover_audio_devices()
            .unwrap_or_default()
            .into_iter()
            .filter(|device| device.kind() == AudioDeviceKind::Input && device.available())
            .map(|device| (device.id().to_owned(), device.name().to_owned()))
            .collect()
    }

    /// Returns discoverable `PipeWire` output devices as `(stable_id, label)`.
    pub(crate) fn audio_output_devices(&mut self) -> Vec<(String, String)> {
        self.discover_audio_devices()
            .unwrap_or_default()
            .into_iter()
            .filter(|device| device.kind() == AudioDeviceKind::Output && device.available())
            .map(|device| (device.id().to_owned(), device.name().to_owned()))
            .collect()
    }

    /// Returns the input picker's entries, keeping `selected` even if it is gone.
    ///
    /// A device that is unplugged, or whose service has restarted, disappears
    /// from discovery. Dropping the user's selection at that moment would
    /// silently rewrite it to "automatic" the next time settings were applied,
    /// so the missing device stays in the list marked unavailable and is only
    /// forgotten when the user picks something else.
    pub(crate) fn audio_input_entries(&mut self, selected: &str) -> Vec<AudioInputEntry> {
        let mut entries = self
            .audio_input_devices()
            .into_iter()
            .map(|(id, name)| AudioInputEntry {
                id,
                name,
                available: true,
            })
            .collect::<Vec<_>>();
        let selected = selected.trim();
        if !selected.is_empty() && !entries.iter().any(|entry| entry.id == selected) {
            entries.push(AudioInputEntry {
                // The stored ID is all that is left of a device that is not in
                // the graph, so it is also the only label available for it.
                name: selected.to_owned(),
                id: selected.to_owned(),
                available: false,
            });
        }
        entries
    }

    /// Returns output-picker entries, keeping a selected missing sink visible.
    pub(crate) fn audio_output_entries(&mut self, selected: &str) -> Vec<AudioOutputEntry> {
        let mut entries = self
            .audio_output_devices()
            .into_iter()
            .map(|(id, name)| AudioOutputEntry {
                id,
                name,
                available: true,
            })
            .collect::<Vec<_>>();
        let selected = selected.trim();
        if !selected.is_empty() && !entries.iter().any(|entry| entry.id == selected) {
            entries.push(AudioOutputEntry {
                name: selected.to_owned(),
                id: selected.to_owned(),
                available: false,
            });
        }
        entries
    }

    /// Returns whether the selected monitor sink is currently discoverable.
    pub(crate) fn audio_monitor_output_available(&mut self) -> bool {
        let Some(selected) = self.monitor_output_id.clone() else {
            return true;
        };
        self.audio_output_devices()
            .iter()
            .any(|(id, _)| *id == selected)
    }

    /// Discards the discovery cache so the next read re-queries the platform.
    ///
    /// This is what makes a hot-plug visible without waiting for the cache to
    /// expire, and it is why the refresh action is explicit rather than a poll.
    pub(crate) fn refresh_audio_devices(&mut self) {
        self.audio_devices_cache = None;
    }

    /// Returns whether the selected input is currently present in the graph.
    ///
    /// `true` for the automatic route, which is by definition always resolvable.
    pub(crate) fn audio_input_available(&mut self) -> bool {
        let Some(selected) = self.audio_input_id.clone() else {
            return true;
        };
        self.audio_input_devices()
            .iter()
            .any(|(id, _)| *id == selected)
    }

    pub(super) fn discover_audio_devices(
        &mut self,
    ) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        let now = Instant::now();
        if let Some((discovered_at, devices)) = self.audio_devices_cache.as_ref() {
            if now.saturating_duration_since(*discovered_at) < Duration::from_secs(2) {
                return Ok(devices.clone());
            }
        }
        let devices = self.audio_provider.discover()?;
        self.audio_devices_cache = Some((now, devices.clone()));
        Ok(devices)
    }
}

fn engine_channel(id: &str) -> obs_rs_engine::EngineAudioChannel {
    if id == crate::MIC_CHANNEL_ID {
        obs_rs_engine::EngineAudioChannel::Microphone
    } else {
        obs_rs_engine::EngineAudioChannel::Desktop
    }
}
