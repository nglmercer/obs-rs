use std::{
    error::Error,
    path::PathBuf,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

use obs_rs_audio::{
    AudioDeviceInfo, AudioFormat, AudioInputProvider, AudioMonitorMode, AudioOutputProvider,
};
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_engine::{
    output_capabilities_snapshot, EngineConfig, EngineSession, EngineWorker,
    OutputCapabilitiesSnapshot, OutputLifecycle, RemuxRecovery,
};
use obs_rs_media::{FrameScaler, RawVideoFrame, ScaleFilter, VideoFormat, VideoFrame};
use obs_rs_output::{
    AudioEncoderConfig, OutputProfile, RtmpConfig, SegmentedRecordingPolicy, StreamState,
    StreamTarget, VideoEncoderConfig,
};
use obs_rs_project::Project;

use crate::settings::{REPLAY_BUFFER_CAPACITY_MIB_DEFAULT, REPLAY_BUFFER_DURATION_DEFAULT};

mod audio;
mod recording;
mod streaming;

use recording::{output_only_project, replay_capacity_bytes, replay_save_label};
#[cfg(test)]
pub(crate) use streaming::stream_protocol_label;

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

/// One entry in the settings window's local monitor-output picker.
pub(crate) struct AudioOutputEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) available: bool,
}

/// Bounded telemetry projected into the multiview overlay.
///
/// The preview worker does not inspect output state and the UI does not read
/// the engine snapshot directly. Keeping this projection at the output
/// boundary prevents a second status source while allowing the multiview to
/// show operator-facing health without opening another runtime.
pub(crate) struct MultiviewTelemetry {
    pub(crate) metrics: String,
    pub(crate) audio_peak_milli: u16,
}

/// GUI-owned handle over the portable engine output boundary.
pub(crate) struct OutputRuntime {
    worker: EngineWorker,
    audio_provider: Arc<PipeWireAudioProvider>,
    audio_format: AudioFormat,
    format: VideoFormat,
    last_revision: u64,
    format_drops: u64,
    audio_input_id: Option<String>,
    monitor_output_id: Option<String>,
    audio_devices_cache: Option<(Instant, Vec<AudioDeviceInfo>)>,
    recording_started_at: Option<Instant>,
    stream_protocol: Option<&'static str>,
    configured_stream: StreamTarget,
    configured_video_encoder: VideoEncoderConfig,
    configured_audio_encoder: AudioEncoderConfig,
    recording_video_encoder: VideoEncoderConfig,
    recording_audio_encoder: AudioEncoderConfig,
    replay_buffer_capacity_bytes: usize,
    replay_buffer_duration: Duration,
    segmented_recording_policy: Option<SegmentedRecordingPolicy>,
    segmented_recording_requested: bool,
    auto_remux_requested: bool,
    auto_remux_enabled: bool,
    remux_recovery: Option<mpsc::Receiver<Result<RemuxRecovery, String>>>,
    remux_candidates: Option<mpsc::Receiver<Result<Vec<PathBuf>, String>>>,
    recording_profile: Option<OutputProfile>,
    /// A canvas change accepted while an output was running.
    ///
    /// Rebuilding the encoders mid-recording would break the container's frame
    /// geometry, so the change is held here and applied at the next idle
    /// boundary instead of being either silently dropped or forced through.
    staged_video_format: Option<VideoFormat>,
    /// An audio-format change accepted while an output was running.
    ///
    /// Device negotiation and packet caps are rebuilt only after the output
    /// stops, so an active recording never changes format mid-container.
    staged_audio_format: Option<AudioFormat>,
    /// Resamples the canvas to the encoded output size.
    ///
    /// The engine encodes whatever it is handed, so scaling belongs on this
    /// side of the boundary: the canvas keeps its own size for preview and
    /// compositing, and only the frames on their way to the encoders are
    /// resized.
    scaler: Option<FrameScaler>,
    /// The geometry the encoders are configured for.
    output_format: VideoFormat,
    scale_filter: ScaleFilter,
    /// An output-scaling change accepted while an output was running.
    staged_scaling: Option<(u32, u32, ScaleFilter)>,
    /// The last project synced, kept so a scaling change can re-sync the
    /// engine without waiting for a project edit.
    project: Option<Project>,
    /// Frames dropped because only an accelerated frame was available while
    /// output scaling was active.
    unscalable_drops: u64,
    capabilities: OutputCapabilitiesSnapshot,
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
    #[cfg(test)]
    pub(crate) fn with_audio_input(
        format: VideoFormat,
        audio_format: AudioFormat,
        audio_input_id: Option<&str>,
    ) -> Result<Self, Box<dyn Error>> {
        Self::with_audio_input_and_sync_offsets(format, audio_format, audio_input_id, 0, 0)
    }

    /// Creates an output with persisted input selection and bounded audio
    /// synchronization offsets.
    #[cfg(test)]
    pub(crate) fn with_audio_input_and_sync_offsets(
        format: VideoFormat,
        audio_format: AudioFormat,
        audio_input_id: Option<&str>,
        audio_input_sync_offset_millis: u32,
        desktop_audio_sync_offset_millis: u32,
    ) -> Result<Self, Box<dyn Error>> {
        Self::with_audio_settings(
            format,
            audio_format,
            audio_input_id,
            audio_input_sync_offset_millis,
            desktop_audio_sync_offset_millis,
            None,
            AudioMonitorMode::Off,
            AudioMonitorMode::Off,
        )
    }

    /// Creates an output with the complete persisted audio-control boundary.
    #[allow(
        clippy::too_many_arguments,
        reason = "this constructor mirrors the bounded persisted audio settings boundary"
    )]
    pub(crate) fn with_audio_settings(
        format: VideoFormat,
        audio_format: AudioFormat,
        audio_input_id: Option<&str>,
        audio_input_sync_offset_millis: u32,
        desktop_audio_sync_offset_millis: u32,
        monitor_output_id: Option<&str>,
        microphone_monitor_mode: AudioMonitorMode,
        desktop_monitor_mode: AudioMonitorMode,
    ) -> Result<Self, Box<dyn Error>> {
        let audio_provider = Arc::new(PipeWireAudioProvider::new());
        let provider_for_engine: Arc<dyn AudioInputProvider> = audio_provider.clone();
        let output_provider_for_engine: Arc<dyn AudioOutputProvider> = audio_provider.clone();
        let mut config = EngineConfig::new(audio_format)
            .with_audio_provider(provider_for_engine)
            .with_audio_output_provider(output_provider_for_engine)
            .with_audio_input_sync_offset_millis(audio_input_sync_offset_millis)
            .with_desktop_audio_sync_offset_millis(desktop_audio_sync_offset_millis)
            .with_audio_input_monitor_mode(microphone_monitor_mode)
            .with_desktop_monitor_mode(desktop_monitor_mode);
        if let Some(audio_input_id) = audio_input_id {
            config = config.with_audio_input_id(audio_input_id);
        }
        if let Some(monitor_output_id) = monitor_output_id {
            config = config.with_monitor_output_id(monitor_output_id);
        }
        let engine = EngineSession::for_format(format, config)?;
        Ok(Self {
            worker: EngineWorker::spawn(engine)?,
            audio_provider,
            audio_format,
            format,
            last_revision: 0,
            format_drops: 0,
            audio_input_id: audio_input_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            monitor_output_id: monitor_output_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            audio_devices_cache: None,
            recording_started_at: None,
            stream_protocol: None,
            configured_stream: StreamTarget::Rtmp(RtmpConfig::default()),
            configured_video_encoder: VideoEncoderConfig::default(),
            configured_audio_encoder: AudioEncoderConfig::default(),
            recording_video_encoder: VideoEncoderConfig::default(),
            recording_audio_encoder: AudioEncoderConfig::default(),
            replay_buffer_capacity_bytes: replay_capacity_bytes(REPLAY_BUFFER_CAPACITY_MIB_DEFAULT),
            replay_buffer_duration: Duration::from_secs(u64::from(REPLAY_BUFFER_DURATION_DEFAULT)),
            segmented_recording_policy: None,
            segmented_recording_requested: false,
            auto_remux_requested: false,
            auto_remux_enabled: false,
            remux_recovery: None,
            remux_candidates: None,
            recording_profile: None,
            staged_video_format: None,
            staged_audio_format: None,
            scaler: None,
            output_format: format,
            scale_filter: ScaleFilter::default(),
            staged_scaling: None,
            project: None,
            unscalable_drops: 0,
            capabilities: output_capabilities_snapshot(),
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

    /// Holds an audio-format change until the running output stops.
    pub(crate) fn stage_audio_format(&mut self, format: AudioFormat) {
        self.staged_audio_format = Some(format);
    }

    /// Takes the staged audio-format change, if the caller is at an idle
    /// boundary.
    pub(crate) fn take_staged_audio_format(&mut self) -> Option<AudioFormat> {
        self.staged_audio_format.take()
    }

    /// Returns whether an audio-format change is waiting for the output to
    /// stop.
    #[cfg(test)]
    pub(crate) const fn has_staged_audio_format(&self) -> bool {
        self.staged_audio_format.is_some()
    }

    /// Returns the audio format currently negotiated by the engine worker.
    pub(crate) const fn audio_format(&self) -> AudioFormat {
        self.audio_format
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

    pub(crate) const fn capabilities(&self) -> &OutputCapabilitiesSnapshot {
        &self.capabilities
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
        self.worker.sync_project(self.encoded_project(&project))?;
        if let Some(format) = next_format {
            self.format = format;
            self.output_format = self.encoded_format(format);
        }
        self.project = Some(project);
        self.last_revision = revision;
        Ok(())
    }

    /// Returns the format the encoders are configured for.
    ///
    /// This is the canvas format when nothing is scaled, and the scaled size
    /// otherwise; the frame rate always comes from the canvas, because scaling
    /// changes geometry and never pacing.
    fn encoded_format(&self, canvas: VideoFormat) -> VideoFormat {
        let (width, height) = (self.output_format.width(), self.output_format.height());
        if (width, height) == (canvas.width(), canvas.height()) {
            return canvas;
        }
        VideoFormat::new(width, height, canvas.frame_rate()).unwrap_or(canvas)
    }

    /// Returns the project the engine is given.
    ///
    /// The engine derives its encoder geometry from the active profile, so a
    /// scaled output is expressed by handing it a profile at the encoded size.
    /// The project the rest of the application holds is untouched: the canvas
    /// really is 1920x1080 even when the encoders run at 1280x720.
    fn encoded_project(&self, project: &Project) -> Project {
        let Some(canvas) = project
            .active_profile_spec()
            .map(obs_rs_project::Profile::video_format)
        else {
            return project.clone();
        };
        let encoded = self.encoded_format(canvas);
        if encoded == canvas {
            return project.clone();
        }
        let mut project = project.clone();
        let active = project.active_profile().clone();
        if let Some(profile) = project.profile_mut(&active) {
            profile.set_video_format(encoded);
        }
        project
    }

    /// Configures the geometry and filter the encoders receive.
    ///
    /// Returns whether the change was applied; a change that arrives while an
    /// output is running is staged instead, exactly like a canvas change,
    /// because rebuilding the encoders mid-recording would break the geometry
    /// of a container whose frames are already committed.
    ///
    /// # Errors
    ///
    /// Returns the engine error when the session could not be rebuilt at the
    /// new geometry.
    pub(crate) fn set_output_scaling(
        &mut self,
        width: u32,
        height: u32,
        filter: ScaleFilter,
    ) -> Result<(), Box<dyn Error>> {
        if (width, height) == (self.output_format.width(), self.output_format.height())
            && filter == self.scale_filter
        {
            return Ok(());
        }
        self.scale_filter = filter;
        self.output_format =
            VideoFormat::new(width, height, self.format.frame_rate()).unwrap_or(self.format);
        self.scaler = None;
        // Before the first project sync the engine is still the output-only
        // session the runtime was constructed with, so the rebuild uses an
        // equivalent one-profile project rather than skipping: leaving the
        // encoders at the canvas size while frames arrive scaled would drop
        // every frame.
        let project = match self.project.clone() {
            Some(project) => self.encoded_project(&project),
            None => output_only_project(self.encoded_format(self.format))?,
        };
        self.worker.sync_project(project)?;
        Ok(())
    }

    /// Holds an output-scaling change until the running output stops.
    pub(crate) fn stage_output_scaling(&mut self, width: u32, height: u32, filter: ScaleFilter) {
        self.staged_scaling = Some((width, height, filter));
    }

    /// Takes the staged output-scaling change, at an idle boundary.
    pub(crate) fn take_staged_output_scaling(&mut self) -> Option<(u32, u32, ScaleFilter)> {
        self.staged_scaling.take()
    }

    /// Returns the geometry the encoders are currently configured for.
    #[cfg(test)]
    pub(crate) const fn encoded_output_format(&self) -> VideoFormat {
        self.output_format
    }

    /// Returns whether an accelerated frame can be handed to the engine
    /// unchanged.
    ///
    /// Packed and planar frames are not resampled here, so while the output is
    /// scaled the RGBA path is the only one that can produce a frame at the
    /// encoded geometry.
    pub(crate) const fn accepts_raw_frames(&self) -> bool {
        self.output_format.width() == self.format.width()
            && self.output_format.height() == self.format.height()
    }

    /// Enqueues a program frame and its due audio without blocking the GUI.
    ///
    /// A frame at the canvas geometry is resampled to the encoded geometry
    /// first, so the settings window's output resolution is what actually
    /// reaches the encoders.
    pub(crate) fn push_frame(&mut self, frame: &VideoFrame) {
        if frame.format() != self.format {
            self.format_drops = self.format_drops.saturating_add(1);
            return;
        }
        let encoded = self.encoded_format(self.format);
        if encoded == self.format {
            // Queue pressure is observable in output_metrics; dropping an
            // animation frame is preferable to stalling scene editing or
            // preview rendering.
            let _ = self.worker.try_push_frame(frame.clone());
            return;
        }
        let scaler = self
            .scaler
            .get_or_insert_with(|| FrameScaler::new(self.format, encoded, self.scale_filter));
        scaler.reconfigure(self.format, encoded, self.scale_filter);
        match scaler.scale(frame) {
            Ok(resampled) => {
                let _ = self.worker.try_push_frame(resampled);
            }
            Err(_) => self.format_drops = self.format_drops.saturating_add(1),
        }
    }

    pub(crate) fn push_raw_frame(&mut self, frame: RawVideoFrame) {
        if !self.accepts_raw_frames() {
            self.unscalable_drops = self.unscalable_drops.saturating_add(1);
            return;
        }
        if frame.format() != self.format {
            self.format_drops = self.format_drops.saturating_add(1);
            return;
        }
        let _ = self.worker.try_push_raw_frame(frame);
    }

    pub(crate) fn output_status(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        // The phase is what the operator needs: "starting" and "failed" are
        // both invisible in the open/closed boolean the handle exposes.
        let recording = engine.recording_lifecycle.label();
        let mut streaming = engine
            .stream_state
            .map_or_else(
                || engine.streaming_lifecycle.label(),
                |state| match state {
                    StreamState::Connected => "connected",
                    StreamState::Disconnected => "reconnecting",
                    StreamState::Failed => "failed",
                    StreamState::Closed => "closed",
                },
            )
            .to_owned();
        if let Some(protocol) = self.stream_protocol {
            streaming = format!("{streaming} {protocol}");
        }
        let audio = if engine.audio_fallback {
            "audio fallback"
        } else {
            "audio live"
        };
        let replay_save = replay_save_label(&engine.replay_save_status);
        let worker = if snapshot.alive {
            "worker live"
        } else {
            "worker stopped"
        };
        let protocols = self
            .capabilities
            .protocols()
            .iter()
            .filter(|capability| capability.available())
            .map(|capability| capability.protocol().display_name())
            .collect::<Vec<_>>()
            .join("/");
        format!(
            "Output: recording {recording} · stream {streaming} · replay {} ({} packets) · replay save {replay_save} · {audio} · {worker} · available {protocols}",
            engine.replay_lifecycle.label(),
            engine.replay_buffer_packets,
        )
    }

    pub(crate) fn output_metrics(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let (mut sent, mut dropped, mut reconnects) =
            engine.stream_metrics.map_or((0, 0, 0), |metrics| {
                (
                    metrics.sent_packets(),
                    metrics.dropped_packets(),
                    metrics.reconnects(),
                )
            });
        let mut native_submit_max = 0;
        let mut native_queue_bytes = 0;
        if let Some(metrics) = engine.production_stream_metrics {
            sent = metrics
                .video_submitted
                .saturating_add(metrics.audio_submitted);
            dropped = metrics.dropped;
            reconnects = metrics.reconnects;
            native_queue_bytes = metrics
                .video_queue_bytes
                .saturating_add(metrics.audio_queue_bytes);
            native_submit_max = metrics.max_submit_latency_nanos;
        }
        format!(
            "frames={} · audio_blocks={} · audio_per_tick={} · av_sync_obs={} · av_sync_in_sync={} · av_sync_behind={} · av_sync_ahead={} · av_sync_max_ns={} · submitted={} · dropped={} · queued={} B · native_queue={} B · worker_queued={} · reconnects={} · submit p50/p95/p99/max={}/{}/{}/{} µs · native_submit_max={} µs · video_encode p50/p95/p99/max={}/{}/{}/{} µs · audio_encode p95={} µs · frame_drops={} · format_drops={} · unscalable_drops={} · peak={}‰",
            engine.stats.video_frames,
            engine.stats.audio_blocks,
            engine.stats.audio_blocks_per_video_tick,
            engine.stats.av_sync.observations(),
            engine.stats.av_sync.in_sync(),
            engine.stats.av_sync.audio_behind(),
            engine.stats.av_sync.audio_ahead(),
            engine.stats.av_sync.max_abs_delta_nanos(),
            sent,
            dropped,
            engine.stream_queued_bytes,
            native_queue_bytes,
            snapshot.queued_frames,
            reconnects,
            engine.stats.output_submit_latency.percentile_nanos(50) / 1_000,
            engine.stats.output_submit_latency.percentile_nanos(95) / 1_000,
            engine.stats.output_submit_latency.percentile_nanos(99) / 1_000,
            engine.stats.output_submit_latency.max_nanos() / 1_000,
            native_submit_max / 1_000,
            engine.stats.video_encode_latency.percentile_nanos(50) / 1_000,
            engine.stats.video_encode_latency.percentile_nanos(95) / 1_000,
            engine.stats.video_encode_latency.percentile_nanos(99) / 1_000,
            engine.stats.video_encode_latency.max_nanos() / 1_000,
            engine.stats.audio_encode_latency.percentile_nanos(95) / 1_000,
            snapshot.dropped_frames,
            self.format_drops,
            self.unscalable_drops,
            engine.stats.audio_peak_milli
        )
    }

    /// Returns the small, bounded telemetry slice needed by multiview.
    pub(crate) fn multiview_telemetry(&self) -> MultiviewTelemetry {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        MultiviewTelemetry {
            metrics: format!(
                "frames={} · dropped={} · audio blocks={} · queued={} B · replay={} packets",
                engine.stats.video_frames,
                snapshot.dropped_frames,
                engine.stats.audio_blocks,
                engine.stream_queued_bytes,
                engine.replay_buffer_packets,
            ),
            audio_peak_milli: engine.stats.audio_peak_milli,
        }
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
        let (mut sent, mut dropped, mut reconnects) =
            engine.stream_metrics.map_or((0, 0, 0), |metrics| {
                (
                    metrics.sent_packets(),
                    metrics.dropped_packets(),
                    metrics.reconnects(),
                )
            });
        let mut native_submit_max = 0;
        let mut native_queue_bytes = 0;
        if let Some(metrics) = engine.production_stream_metrics {
            sent = metrics
                .video_submitted
                .saturating_add(metrics.audio_submitted);
            dropped = metrics.dropped;
            reconnects = metrics.reconnects;
            native_queue_bytes = metrics
                .video_queue_bytes
                .saturating_add(metrics.audio_queue_bytes);
            native_submit_max = metrics.max_submit_latency_nanos;
        }
        let replay_save = replay_save_label(&engine.replay_save_status);
        format!(
            "worker_alive={} project_revision={} recording={} streaming={} replay_lifecycle={} replay_save={} replay_packets={} stream_protocol={} recording_lifecycle={} streaming_lifecycle={} stream_state={:?} audio_backend={} audio_fallback={} desktop_audio_backend={} desktop_audio_active={} audio_devices={} worker_queued_frames={} stream_queue_bytes={} stream_submitted={} stream_dropped={} stream_reconnects={} native_queue_bytes={} native_submit_max_nanos={} output_submit_p50_nanos={} output_submit_p95_nanos={} output_submit_p99_nanos={} output_submit_max_nanos={} video_encode_p50_nanos={} video_encode_p95_nanos={} video_encode_p99_nanos={} video_encode_max_nanos={} audio_encode_p95_nanos={} audio_blocks_per_video_tick={} av_sync_obs={} av_sync_in_sync={} av_sync_behind={} av_sync_ahead={} av_sync_max_ns={} frame_drops={} format_drops={} unscalable_drops={} ticks={} video_frames={} audio_blocks={} audio_fallback_blocks={} audio_peak_milli={} filter_diagnostics={:?} last_error={}",
            snapshot.alive,
            self.last_revision,
            engine.recording,
            engine.streaming,
            engine.replay_lifecycle.label(),
            replay_save,
            engine.replay_buffer_packets,
            self.stream_protocol.unwrap_or("none"),
            engine.recording_lifecycle.label(),
            engine.streaming_lifecycle.label(),
            engine.stream_state,
            engine.audio_backend,
            engine.audio_fallback,
            engine.desktop_audio.label(),
            engine.desktop_audio.is_capturing(),
            devices,
            snapshot.queued_frames,
            engine.stream_queued_bytes,
            sent,
            dropped,
            reconnects,
            native_queue_bytes,
            native_submit_max,
            engine.stats.output_submit_latency.percentile_nanos(50),
            engine.stats.output_submit_latency.percentile_nanos(95),
            engine.stats.output_submit_latency.percentile_nanos(99),
            engine.stats.output_submit_latency.max_nanos(),
            engine.stats.video_encode_latency.percentile_nanos(50),
            engine.stats.video_encode_latency.percentile_nanos(95),
            engine.stats.video_encode_latency.percentile_nanos(99),
            engine.stats.video_encode_latency.max_nanos(),
            engine.stats.audio_encode_latency.percentile_nanos(95),
            engine.stats.audio_blocks_per_video_tick,
            engine.stats.av_sync.observations(),
            engine.stats.av_sync.in_sync(),
            engine.stats.av_sync.audio_behind(),
            engine.stats.av_sync.audio_ahead(),
            engine.stats.av_sync.max_abs_delta_nanos(),
            snapshot.dropped_frames,
            self.format_drops,
            self.unscalable_drops,
            engine.stats.ticks,
            engine.stats.video_frames,
            engine.stats.audio_blocks,
            engine.stats.audio_fallback_blocks,
            engine.stats.audio_peak_milli,
            engine.filter_diagnostics,
            engine.last_error.as_deref().unwrap_or("none")
        )
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
