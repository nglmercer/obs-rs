use std::{error::Error, sync::Arc};

use obs_rs_audio::{AudioDeviceKind, AudioFormat, AudioInputProvider};
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
    pub(crate) fn with_audio(
        format: VideoFormat,
        audio_format: AudioFormat,
    ) -> Result<Self, Box<dyn Error>> {
        let audio_provider = Arc::new(PipeWireAudioProvider::new());
        let provider_for_engine: Arc<dyn AudioInputProvider> = audio_provider.clone();
        let config = EngineConfig::new(audio_format).with_audio_provider(provider_for_engine);
        let engine = EngineSession::for_format(format, config)?;
        Ok(Self {
            worker: EngineWorker::spawn(engine)?,
            audio_provider,
            format,
            last_revision: 0,
            format_drops: 0,
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
        Ok(())
    }

    pub(crate) fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        Ok(self.worker.finish_recording()?)
    }

    pub(crate) fn abort_recording(&mut self) {
        self.worker.abort_recording();
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

    pub(crate) fn diagnostics_document(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let devices = self.audio_provider.discover().map_or_else(
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

    pub(crate) fn audio_devices_summary(&self) -> String {
        match self.audio_provider.discover() {
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

    pub(crate) fn stream_failed(&self) -> bool {
        let snapshot = self.worker.snapshot();
        !snapshot.alive || snapshot.engine.stream_state == Some(StreamState::Failed)
    }
}
