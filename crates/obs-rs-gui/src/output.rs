use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use obs_rs_audio::{AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInputProvider};
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_engine::{EngineConfig, EngineSession, EngineWorker};
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_output::StreamState;
use obs_rs_project::Project;

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
        })
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

    pub(crate) fn set_input_gain_milli(&mut self, gain_milli: u16) -> Result<(), Box<dyn Error>> {
        self.worker.set_input_gain_milli(gain_milli)?;
        Ok(())
    }

    pub(crate) fn set_input_muted(&mut self, muted: bool) -> Result<(), Box<dyn Error>> {
        self.worker.set_input_muted(muted)?;
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

    pub(crate) fn output_status(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let recording = if engine.recording {
            "recording open"
        } else {
            "recording stopped"
        };
        let streaming = engine
            .stream_state
            .map_or("stream stopped", |state| match state {
                StreamState::Connected => "stream connected",
                StreamState::Disconnected => "stream reconnecting",
                StreamState::Failed => "stream failed",
                StreamState::Closed => "stream closed",
            });
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
        format!("Output: {recording} · {streaming} · {audio} · {worker}")
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
            "worker_alive={} project_revision={} recording={} streaming={} stream_state={:?} audio_backend={} audio_fallback={} audio_devices={} worker_queued_frames={} stream_queue_bytes={} stream_sent={} stream_dropped={} stream_reconnects={} frame_drops={} format_drops={} ticks={} video_frames={} audio_blocks={} audio_fallback_blocks={} audio_peak_milli={} last_error={}",
            snapshot.alive,
            self.last_revision,
            engine.recording,
            engine.streaming,
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

    pub(crate) fn stream_failed(&self) -> bool {
        let snapshot = self.worker.snapshot();
        !snapshot.alive || snapshot.engine.stream_state == Some(StreamState::Failed)
    }
}
