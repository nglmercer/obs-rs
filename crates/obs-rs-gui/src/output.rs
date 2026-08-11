use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use obs_rs_audio::{AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInputProvider};
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_engine::{
    EngineAudioChannel, EngineConfig, EngineSession, EngineWorker, OutputLifecycle,
};
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_output::StreamState;
use obs_rs_project::Project;

/// One entry in the settings window's audio-input picker.
///
/// `available` is carried separately from presence in the list because a
/// selected device that has gone missing must still be offered, so the list
/// alone cannot express whether the graph currently has it.
pub(crate) struct AudioInputEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) available: bool,
}

/// GUI-owned handle over the portable engine output boundary.
pub(crate) struct OutputRuntime {
    worker: EngineWorker,
    audio_provider: Arc<PipeWireAudioProvider>,
    format: VideoFormat,
    last_revision: u64,
    format_drops: u64,
    audio_input_id: Option<String>,
    audio_devices_cache: Option<(Instant, Vec<AudioDeviceInfo>)>,
    recording_started_at: Option<Instant>,
    /// A canvas change accepted while an output was running.
    ///
    /// Rebuilding the encoders mid-recording would break the container's frame
    /// geometry, so the change is held here and applied at the next idle
    /// boundary instead of being either silently dropped or forced through.
    staged_video_format: Option<VideoFormat>,
}

impl OutputRuntime {
    /// Creates an output with the reference 48 kHz stereo format.
    #[cfg(test)]
    pub(crate) fn new(format: VideoFormat) -> Self {
        let audio_format = AudioFormat::new(48_000, 2)
            .unwrap_or_else(|error| unreachable!("the built-in audio format is valid: {error}"));
        Self::with_audio(format, audio_format)
            .unwrap_or_else(|error| unreachable!("the built-in output session is valid: {error}"))
    }

    /// Creates an output using the audio format selected in settings.
    #[cfg(test)]
    pub(crate) fn with_audio(
        format: VideoFormat,
        audio_format: AudioFormat,
    ) -> Result<Self, Box<dyn Error>> {
        Self::with_audio_input(format, audio_format, None)
    }

    /// Creates an output with a persisted input selection. Device discovery and
    /// process startup remain inside the engine worker's construction path.
    pub(crate) fn with_audio_input(
        format: VideoFormat,
        audio_format: AudioFormat,
        audio_input_id: Option<&str>,
    ) -> Result<Self, Box<dyn Error>> {
        let audio_provider = Arc::new(PipeWireAudioProvider::new());
        let provider_for_engine: Arc<dyn AudioInputProvider> = audio_provider.clone();
        let mut config = EngineConfig::new(audio_format).with_audio_provider(provider_for_engine);
        if let Some(audio_input_id) = audio_input_id {
            config = config.with_audio_input_id(audio_input_id);
        }
        let engine = EngineSession::for_format(format, config)?;
        Ok(Self {
            worker: EngineWorker::spawn(engine)?,
            audio_provider,
            format,
            last_revision: 0,
            format_drops: 0,
            audio_input_id: audio_input_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            audio_devices_cache: None,
            recording_started_at: None,
            staged_video_format: None,
        })
    }

    /// Holds a canvas change until the running output stops.
    pub(crate) fn stage_video_format(&mut self, format: VideoFormat) {
        self.staged_video_format = Some(format);
    }

    /// Takes the staged canvas change, if the caller is at an idle boundary.
    pub(crate) fn take_staged_video_format(&mut self) -> Option<VideoFormat> {
        self.staged_video_format.take()
    }

    /// Returns whether a canvas change is waiting for the output to stop.
    #[cfg(test)]
    pub(crate) const fn has_staged_video_format(&self) -> bool {
        self.staged_video_format.is_some()
    }

    /// Returns the canvas format the engine is currently encoding at.
    #[cfg(test)]
    pub(crate) const fn video_format(&self) -> VideoFormat {
        self.format
    }

    pub(crate) fn needs_project_sync(&self, revision: u64) -> bool {
        revision != self.last_revision
    }

    pub(crate) fn sync_project(
        &mut self,
        project: Project,
        revision: u64,
    ) -> Result<(), Box<dyn Error>> {
        if !self.needs_project_sync(revision) {
            return Ok(());
        }
        let next_format = project
            .active_profile_spec()
            .map(obs_rs_project::Profile::video_format);
        self.worker.sync_project(project)?;
        if let Some(format) = next_format {
            self.format = format;
        }
        self.last_revision = revision;
        Ok(())
    }

    pub(crate) fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        self.worker.start_recording(path)?;
        self.recording_started_at = Some(Instant::now());
        Ok(())
    }

    pub(crate) fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        let bytes = self.worker.finish_recording()?;
        self.recording_started_at = None;
        Ok(bytes)
    }

    pub(crate) fn abort_recording(&mut self) {
        self.worker.abort_recording();
        self.recording_started_at = None;
    }

    pub(crate) fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        self.worker.start_streaming(address)?;
        Ok(())
    }

    pub(crate) fn finish_streaming(&mut self) {
        self.worker.finish_streaming();
    }

    /// Enqueues a program frame and its due audio without blocking the GUI.
    pub(crate) fn push_frame(&mut self, frame: &VideoFrame) {
        if frame.format() != self.format {
            self.format_drops = self.format_drops.saturating_add(1);
            return;
        }
        // Queue pressure is observable in output_metrics; dropping an animation
        // frame is preferable to stalling scene editing or preview rendering.
        let _ = self.worker.try_push_frame(frame.clone());
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

    pub(crate) fn set_channel_muted(
        &mut self,
        id: &str,
        muted: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.worker.set_channel_muted(engine_channel(id), muted)?;
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

    /// Returns the persisted/selected input ID, or an empty string for auto.
    #[cfg(test)]
    pub(crate) fn audio_input_id(&self) -> Option<&str> {
        self.audio_input_id.as_deref()
    }

    /// Returns the live input peak in thousandths of full scale.
    ///
    /// This is what the mixer meter shows: the engine measures it on the block
    /// it actually captured, so the meter moves with the real microphone.
    pub(crate) fn input_peak_milli(&self) -> u16 {
        self.worker.snapshot().engine.stats.microphone_peak_milli
    }

    pub(crate) fn desktop_peak_milli(&self) -> u16 {
        self.worker.snapshot().engine.stats.desktop_peak_milli
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

    pub(crate) fn output_status(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        // The phase is what the operator needs: "starting" and "failed" are
        // both invisible in the open/closed boolean the handle exposes.
        let recording = engine.recording_lifecycle.label();
        let streaming = engine.stream_state.map_or_else(
            || engine.streaming_lifecycle.label(),
            |state| match state {
                StreamState::Connected => "connected",
                StreamState::Disconnected => "reconnecting",
                StreamState::Failed => "failed",
                StreamState::Closed => "closed",
            },
        );
        let audio = if engine.audio_fallback {
            "audio fallback"
        } else {
            "audio live"
        };
        let worker = if snapshot.alive {
            "worker live"
        } else {
            "worker stopped"
        };
        format!("Output: recording {recording} · stream {streaming} · {audio} · {worker}")
    }

    pub(crate) fn output_metrics(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let (sent, dropped, reconnects) = engine.stream_metrics.map_or((0, 0, 0), |metrics| {
            (
                metrics.sent_packets(),
                metrics.dropped_packets(),
                metrics.reconnects(),
            )
        });
        format!(
            "frames={} · audio_blocks={} · sent={} · dropped={} · queued={} B · worker_queued={} · reconnects={} · frame_drops={} · format_drops={} · peak={}‰",
            engine.stats.video_frames,
            engine.stats.audio_blocks,
            sent,
            dropped,
            engine.stream_queued_bytes,
            snapshot.queued_frames,
            reconnects,
            snapshot.dropped_frames,
            self.format_drops,
            engine.stats.audio_peak_milli
        )
    }

    /// Returns the live recording duration in the status-bar format.
    pub(crate) fn recording_elapsed(&self) -> String {
        let seconds = self
            .recording_started_at
            .map_or(0, |started| started.elapsed().as_secs());
        let hours = seconds / 3_600;
        let minutes = (seconds % 3_600) / 60;
        let seconds = seconds % 60;
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }

    pub(crate) fn diagnostics_document(&mut self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let devices = self.discover_audio_devices().map_or_else(
            |error| format!("unavailable:{error}"),
            |devices| {
                devices
                    .iter()
                    .map(|device| {
                        format!(
                            "{}:{}:{}",
                            device.id(),
                            device.name(),
                            if device.available() {
                                "available"
                            } else {
                                "unavailable"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            },
        );
        let (sent, dropped, reconnects) = engine.stream_metrics.map_or((0, 0, 0), |metrics| {
            (
                metrics.sent_packets(),
                metrics.dropped_packets(),
                metrics.reconnects(),
            )
        });
        format!(
            "worker_alive={} project_revision={} recording={} streaming={} recording_lifecycle={} streaming_lifecycle={} stream_state={:?} audio_backend={} audio_fallback={} audio_devices={} worker_queued_frames={} stream_queue_bytes={} stream_sent={} stream_dropped={} stream_reconnects={} frame_drops={} format_drops={} ticks={} video_frames={} audio_blocks={} audio_fallback_blocks={} audio_peak_milli={} last_error={}",
            snapshot.alive,
            self.last_revision,
            engine.recording,
            engine.streaming,
            engine.recording_lifecycle.label(),
            engine.streaming_lifecycle.label(),
            engine.stream_state,
            engine.audio_backend,
            engine.audio_fallback,
            devices,
            snapshot.queued_frames,
            engine.stream_queued_bytes,
            sent,
            dropped,
            reconnects,
            snapshot.dropped_frames,
            self.format_drops,
            engine.stats.ticks,
            engine.stats.video_frames,
            engine.stats.audio_blocks,
            engine.stats.audio_fallback_blocks,
            engine.stats.audio_peak_milli,
            engine.last_error.as_deref().unwrap_or("none")
        )
    }

    pub(crate) fn audio_devices_summary(&mut self) -> String {
        match self.discover_audio_devices() {
            Ok(devices) if devices.is_empty() => {
                "PipeWire: no audio devices; deterministic fallback available".to_owned()
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
                format!("PipeWire unavailable: {error}; deterministic fallback available")
            }
        }
    }

    /// Returns discoverable `PipeWire` input devices as `(stable_id, label)`.
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

    /// Discards the discovery cache so the next read re-runs `pw-dump`.
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

    fn discover_audio_devices(
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

    /// Returns the recording and streaming phases the desktop reconciles against.
    ///
    /// A dead worker is reported as `Failed` for both, because a session whose
    /// worker has gone is not recording or streaming no matter what its last
    /// published snapshot said.
    pub(crate) fn lifecycles(&self) -> (OutputLifecycle, OutputLifecycle) {
        let snapshot = self.worker.snapshot();
        if snapshot.alive {
            (
                snapshot.engine.recording_lifecycle,
                snapshot.engine.streaming_lifecycle,
            )
        } else {
            (OutputLifecycle::Failed, OutputLifecycle::Failed)
        }
    }
}

fn engine_channel(id: &str) -> EngineAudioChannel {
    if id == crate::MIC_CHANNEL_ID {
        EngineAudioChannel::Microphone
    } else {
        EngineAudioChannel::Desktop
    }
}
