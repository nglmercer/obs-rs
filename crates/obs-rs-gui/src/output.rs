use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use obs_rs_audio::{AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInputProvider};
#[cfg(not(target_os = "windows"))]
use obs_rs_audio_pipewire::PipeWireAudioProvider;
#[cfg(target_os = "windows")]
use obs_rs_audio_wasapi::WasapiAudioProvider;
use obs_rs_engine::{
    output_capabilities_snapshot, EngineAudioChannel, EngineConfig, EngineSession, EngineWorker,
    OutputCapabilitiesSnapshot, OutputEvent, OutputLifecycle,
};
use obs_rs_media::{FrameScaler, RawVideoFrame, ScaleFilter, VideoFormat, VideoFrame};
use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, EncoderImplementation, RtmpConfig, StreamProtocol, StreamState,
    StreamTarget, VideoCodec, VideoEncoderConfig,
};
use obs_rs_project::Project;

use crate::AppSettings;

#[cfg(target_os = "windows")]
const AUDIO_BACKEND_LABEL: &str = "WASAPI";
#[cfg(not(target_os = "windows"))]
const AUDIO_BACKEND_LABEL: &str = "PipeWire";

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
    audio_provider: Arc<dyn AudioInputProvider>,
    format: VideoFormat,
    last_revision: u64,
    format_drops: u64,
    audio_input_id: Option<String>,
    audio_devices_cache: Option<(Instant, Vec<AudioDeviceInfo>)>,
    recording_started_at: Option<Instant>,
    stream_protocol: Option<&'static str>,
    configured_stream: StreamTarget,
    configured_video_encoder: VideoEncoderConfig,
    configured_audio_encoder: AudioEncoderConfig,
    recording_video_encoder: VideoEncoderConfig,
    recording_audio_encoder: AudioEncoderConfig,
    /// A canvas change accepted while an output was running.
    ///
    /// Rebuilding the encoders mid-recording would break the container's frame
    /// geometry, so the change is held here and applied at the next idle
    /// boundary instead of being either silently dropped or forced through.
    staged_video_format: Option<VideoFormat>,
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
    pub(crate) fn with_audio_input(
        format: VideoFormat,
        audio_format: AudioFormat,
        audio_input_id: Option<&str>,
    ) -> Result<Self, Box<dyn Error>> {
        #[cfg(target_os = "windows")]
        let audio_provider: Arc<dyn AudioInputProvider> = Arc::new(WasapiAudioProvider::new());
        #[cfg(not(target_os = "windows"))]
        let audio_provider: Arc<dyn AudioInputProvider> = Arc::new(PipeWireAudioProvider::new());
        let provider_for_engine = Arc::clone(&audio_provider);
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
            stream_protocol: None,
            configured_stream: StreamTarget::Rtmp(RtmpConfig::default()),
            configured_video_encoder: VideoEncoderConfig::default(),
            configured_audio_encoder: AudioEncoderConfig::default(),
            recording_video_encoder: VideoEncoderConfig::default(),
            recording_audio_encoder: AudioEncoderConfig::default(),
            staged_video_format: None,
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

    pub(crate) fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        if path.to_ascii_lowercase().ends_with(".mkv") {
            self.worker.start_recording_configured(
                path,
                self.recording_video_encoder.clone(),
                self.recording_audio_encoder.clone(),
            )?;
        } else {
            self.worker.start_recording(path)?;
        }
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

    #[cfg(test)]
    pub(crate) fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        self.worker.start_streaming(address)?;
        self.stream_protocol = Some(stream_protocol_label(address));
        Ok(())
    }

    pub(crate) fn configure_stream(&mut self, settings: &AppSettings) {
        self.configured_stream = settings.stream_target();
        // The recording encoder is derived from the quality preset at the
        // encoded geometry, so a preset means the same thing to the encoder as
        // it does on the Output page.
        let codec = settings.effective_recording_codec();
        self.recording_video_encoder = VideoEncoderConfig {
            implementation: self
                .capabilities
                .video_encoders()
                .iter()
                .find(|encoder| encoder.codec() == codec)
                .map_or_else(EncoderImplementation::default, |encoder| {
                    EncoderImplementation::new(encoder.id())
                }),
            ..settings.recording_video_encoder(self.output_format)
        };
        self.recording_audio_encoder = AudioEncoderConfig {
            implementation: if settings.recording_audio_encoder.is_automatic() {
                self.capabilities
                    .audio_encoders()
                    .iter()
                    .find(|encoder| encoder.codec() == AudioCodec::Aac)
                    .map_or_else(EncoderImplementation::default, |encoder| {
                        EncoderImplementation::new(encoder.id())
                    })
            } else {
                settings.recording_audio_encoder.clone()
            },
            ..settings.recording_audio_encoder_config()
        };
        self.configured_video_encoder = settings.rtmp.video.clone();
        self.configured_audio_encoder = settings.rtmp.audio.clone();
        if settings.stream_protocol == StreamProtocol::Whip {
            self.configured_video_encoder.codec = VideoCodec::Vp8;
            self.configured_video_encoder.implementation = EncoderImplementation::default();
            self.configured_video_encoder.profile = None;
            self.configured_audio_encoder.codec = AudioCodec::Opus;
            self.configured_audio_encoder.implementation = EncoderImplementation::default();
        }
        if self.configured_video_encoder.implementation.is_automatic() {
            if let Some(encoder) = self
                .capabilities
                .video_encoders()
                .iter()
                .find(|encoder| encoder.codec() == self.configured_video_encoder.codec)
            {
                self.configured_video_encoder.implementation =
                    obs_rs_output::EncoderImplementation::new(encoder.id());
            }
        }
        if self.configured_audio_encoder.implementation.is_automatic() {
            if let Some(encoder) = self
                .capabilities
                .audio_encoders()
                .iter()
                .find(|encoder| encoder.codec() == self.configured_audio_encoder.codec)
            {
                self.configured_audio_encoder.implementation =
                    obs_rs_output::EncoderImplementation::new(encoder.id());
            }
        }
    }

    pub(crate) fn start_configured_stream(&mut self) -> Result<&'static str, Box<dyn Error>> {
        let protocol = stream_protocol_name(self.configured_stream.protocol());
        self.worker.start_streaming_target_configured(
            self.configured_stream.clone(),
            self.configured_video_encoder.clone(),
            self.configured_audio_encoder.clone(),
        )?;
        self.stream_protocol = Some(protocol);
        Ok(protocol)
    }

    pub(crate) fn finish_streaming(&mut self) -> Result<(), Box<dyn Error>> {
        self.worker.finish_streaming()?;
        Ok(())
    }

    pub(crate) fn take_output_events(&mut self) -> Vec<OutputEvent> {
        let events = self.worker.take_output_events();
        if events
            .iter()
            .any(|event| matches!(event, OutputEvent::Stopped | OutputEvent::Failed { .. }))
        {
            self.stream_protocol = None;
        }
        events
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
            "Output: recording {recording} · stream {streaming} · {audio} · {worker} · available {protocols}"
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
        if let Some(metrics) = engine.production_stream_metrics {
            sent = metrics
                .video_submitted
                .saturating_add(metrics.audio_submitted);
            dropped = metrics.dropped;
            reconnects = metrics.reconnects;
            native_submit_max = metrics.max_submit_latency_nanos;
        }
        format!(
            "frames={} · audio_blocks={} · audio_per_tick={} · submitted={} · dropped={} · queued={} B · worker_queued={} · reconnects={} · submit p50/p95/p99/max={}/{}/{}/{} µs · native_submit_max={} µs · video_encode p50/p95/p99/max={}/{}/{}/{} µs · audio_encode p95={} µs · frame_drops={} · format_drops={} · unscalable_drops={} · peak={}‰",
            engine.stats.video_frames,
            engine.stats.audio_blocks,
            engine.stats.audio_blocks_per_video_tick,
            sent,
            dropped,
            engine.stream_queued_bytes,
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
        if let Some(metrics) = engine.production_stream_metrics {
            sent = metrics
                .video_submitted
                .saturating_add(metrics.audio_submitted);
            dropped = metrics.dropped;
            reconnects = metrics.reconnects;
            native_submit_max = metrics.max_submit_latency_nanos;
        }
        format!(
            "worker_alive={} project_revision={} recording={} streaming={} stream_protocol={} recording_lifecycle={} streaming_lifecycle={} stream_state={:?} audio_backend={} audio_fallback={} desktop_audio_backend={} desktop_audio_active={} audio_devices={} worker_queued_frames={} stream_queue_bytes={} stream_submitted={} stream_dropped={} stream_reconnects={} native_submit_max_nanos={} output_submit_p50_nanos={} output_submit_p95_nanos={} output_submit_p99_nanos={} output_submit_max_nanos={} video_encode_p50_nanos={} video_encode_p95_nanos={} video_encode_p99_nanos={} video_encode_max_nanos={} audio_encode_p95_nanos={} audio_blocks_per_video_tick={} frame_drops={} format_drops={} unscalable_drops={} ticks={} video_frames={} audio_blocks={} audio_fallback_blocks={} audio_peak_milli={} last_error={}",
            snapshot.alive,
            self.last_revision,
            engine.recording,
            engine.streaming,
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
            snapshot.dropped_frames,
            self.format_drops,
            self.unscalable_drops,
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
                format!("{AUDIO_BACKEND_LABEL}: no audio devices; deterministic fallback available")
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
                    "{AUDIO_BACKEND_LABEL} unavailable: {error}; deterministic fallback available"
                )
            }
        }
    }

    /// Returns discoverable platform input devices as stable ID/label pairs.
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

/// Builds the one-profile project an output-only engine session encodes with.
fn output_only_project(format: VideoFormat) -> Result<Project, Box<dyn Error>> {
    let mut project = Project::new("OBS-RS output session")?;
    project.add_profile(obs_rs_project::Profile::new(
        "default",
        "Default output",
        format,
    )?)?;
    Ok(project)
}

const fn stream_protocol_name(protocol: StreamProtocol) -> &'static str {
    match protocol {
        StreamProtocol::Rtmp => "RTMP",
        StreamProtocol::Rtmps => "RTMPS",
        StreamProtocol::Srt => "SRT",
        StreamProtocol::Whip => "WHIP",
        StreamProtocol::Hls => "HLS",
        StreamProtocol::Rist => "RIST",
        StreamProtocol::Reference => "Reference",
    }
}

#[cfg(test)]
pub(crate) fn stream_protocol_label(address: &str) -> &'static str {
    match address.trim().split(':').next() {
        Some("srt") => "SRT",
        Some("rtmp") => "RTMP",
        Some("rtmps") => "RTMPS",
        Some("ws" | "wss") => "OBSR-WebSocket",
        _ => "OBSR-TCP",
    }
}

fn engine_channel(id: &str) -> EngineAudioChannel {
    if id == crate::MIC_CHANNEL_ID {
        EngineAudioChannel::Microphone
    } else {
        EngineAudioChannel::Desktop
    }
}
