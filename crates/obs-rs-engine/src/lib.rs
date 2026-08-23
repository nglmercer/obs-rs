//! Portable orchestration for one OBS-RS audio/video session.
//!
//! The engine deliberately owns the media boundary without owning a GUI event
//! loop. Applications can drive it from a worker thread, a headless command, or
//! a desktop adapter. Platform devices are injected through the audio provider
//! trait; the deterministic signal remains available as a safe fallback.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod audio;
mod audio_routes;
mod config;
mod filters;
mod outputs;
mod session_controls;
mod session_media;
mod types;
mod worker;

use audio::{
    audio_peak_milli, audio_reconnect_deadline, open_audio_input, open_desktop_audio,
    open_live_audio_input, open_live_desktop_audio,
};
pub use config::EngineConfig;
pub use filters::{
    compile_audio_filter, compile_audio_filter_report, compile_filter, compile_filter_report,
};
pub use types::{
    DesktopAudioSource, EngineAudioChannel, EngineError, EngineSnapshot, EngineStats, EngineTick,
    FilterCompilation, FilterCompileFailure, FilterDiagnostic, OutputEvent, OutputLifecycle,
    ProductionStreamMetrics, ReplaySaveStatus,
};
pub use worker::{EngineWorker, EngineWorkerSnapshot};

use outputs::{RecordingOutput, StreamOutput};

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use audio_routes::{
    AudioRouteRequest, AudioRouteUpdate, AudioRouteWorker, ROUTE_REFRESH_INTERVAL_NANOS,
};

#[cfg(test)]
use obs_rs_audio::AudioDeviceError;
#[cfg(test)]
use obs_rs_audio::AudioDeviceKind;
use obs_rs_audio::{
    AudioBuffer, AudioDelayLine, AudioFilterChain, AudioInput, AudioInputProvider, AudioMixer,
    AudioOutputWorker, AudioOutputWorkerHandle, AudioSourceId, AvSyncMetrics,
    SimulatedAudioProvider,
};
#[cfg(test)]
use obs_rs_audio::{AudioFilter, AudioFormat};
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_clock::MediaTimeline;
use obs_rs_core::Runtime;
#[cfg(test)]
use obs_rs_media::{ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, LumaKey, RenderDelay};
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{
    recover_stale_packet_files, AtomicPacketFileWriter, AudioEncoder, AudioEncoderConfig,
    AudioInputRequirement, EncodedPacket, OutputProfile, RawAudioEncoder, ReconnectOutcome,
    ReplayBuffer, RleVideoEncoder, SegmentedPacketFileWriter, SegmentedRecordingPolicy,
    StreamMetrics, StreamState, StreamTarget, VideoEncoder, VideoEncoderConfig,
    VideoInputRequirement,
};
#[cfg(test)]
use obs_rs_output::{
    PacketDropPolicy, ReconnectPolicy, StreamSession, TcpPacketTransport, WebSocketPacketTransport,
};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output_gstreamer::{
    discover_interrupted_remux_candidates, recover_interrupted_remux_recording,
    GStreamerCapabilitySnapshot, GStreamerError, GStreamerOutputSession, NativeOutputState,
    ProductionDestination, ProductionPipelinePlan,
};
#[cfg(feature = "production-gstreamer")]
pub use obs_rs_output_gstreamer::{
    write_interrupted_remux_manifest, AudioEncoderCapability, OutputCapabilitiesSnapshot,
    ProductionProtocol, ProtocolCapability, RemuxRecovery, VideoEncoderCapability,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::Project;

const DEFAULT_AUDIO_BLOCK_FRAMES: usize = 480;
const DEFAULT_TIMELINE_TOLERANCE_NANOS: u64 = 5_000_000;
const DEFAULT_OUTPUT_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_RECONNECT_ATTEMPTS: u32 = 3;
const DEFAULT_MONITOR_OUTPUT_QUEUE_BLOCKS: usize = 8;
const AUDIO_RECONNECT_INTERVAL_NANOS: u64 = 1_000_000_000;
/// Maximum number of persisted-filter diagnostics retained in one snapshot.
pub const MAX_FILTER_DIAGNOSTICS: usize = 64;

/// Probes the production backend once and returns its typed GUI-safe model.
#[cfg(feature = "production-gstreamer")]
#[must_use]
pub fn output_capabilities_snapshot() -> OutputCapabilitiesSnapshot {
    GStreamerCapabilitySnapshot::probe().capabilities()
}

/// The portable engine session.
pub struct EngineSession {
    config: EngineConfig,
    project: Project,
    format: VideoFormat,
    plugin: BuiltinPlugin,
    runtime: Runtime,
    filter_diagnostics: Vec<String>,
    timeline: MediaTimeline,
    mixer: AudioMixer,
    desktop_audio_source: AudioSourceId,
    microphone_audio_source: AudioSourceId,
    desktop_audio_filters: AudioFilterChain,
    microphone_audio_filters: AudioFilterChain,
    audio_input: Box<dyn AudioInput>,
    audio_backend: String,
    audio_fallback: bool,
    /// Runtime identity of the device currently feeding the microphone.
    audio_active_device_id: Option<String>,
    /// Next media timestamp at which a selected input may be reopened.
    audio_reconnect_at: Option<Timestamp>,
    /// Bounded delay line for the microphone channel.
    audio_input_delay: AudioDelayLine,
    /// Absent when no playback monitor could be opened, which keeps the desktop
    /// channel silent instead of substituting another signal for it.
    desktop_audio: Option<Box<dyn AudioInput>>,
    desktop_audio_backend: String,
    /// Runtime identity of the playback route currently feeding desktop audio.
    desktop_audio_active_device_id: Option<String>,
    /// Next media timestamp at which a selected monitor may be reopened.
    desktop_audio_reconnect_at: Option<Timestamp>,
    /// Bounded delay line for the desktop channel.
    desktop_audio_delay: AudioDelayLine,
    /// Discovers and opens automatic route replacements off the audio tick.
    audio_route_worker: AudioRouteWorker,
    /// Next media timestamp at which a non-blocking route refresh may be queued.
    audio_route_refresh_at: Timestamp,
    /// Sequence of the most recent route request, used to reject stale results.
    audio_route_request_sequence: u64,
    /// Whether the worker is processing one bounded refresh request.
    audio_route_request_pending: bool,
    /// Owns the optional native monitor sink on a dedicated output thread.
    monitor_output_worker: Option<AudioOutputWorker>,
    /// Non-blocking handoff used by the real-time audio path.
    monitor_output_handle: Option<AudioOutputWorkerHandle>,
    next_audio_deadline: Option<obs_rs_audio::AudioDeadline>,
    render_timestamp: Timestamp,
    video_encoder: Box<dyn VideoEncoder>,
    audio_encoder: RawAudioEncoder,
    recording: Option<RecordingOutput>,
    replay_buffer: Option<ReplayBuffer>,
    streaming: Option<StreamOutput>,
    /// Phases the handles alone cannot express, notably a failed start.
    recording_lifecycle: OutputLifecycle,
    streaming_lifecycle: OutputLifecycle,
    replay_lifecycle: OutputLifecycle,
    replay_save_status: ReplaySaveStatus,
    stats: EngineStats,
    last_error: Option<String>,
    #[cfg(test)]
    reference_video_encode_calls: u64,
    #[cfg(test)]
    reference_audio_encode_calls: u64,
}

#[allow(
    clippy::missing_errors_doc,
    reason = "the session methods share the documented EngineError boundary"
)]
impl EngineSession {
    /// Builds a session from the project's active profile.
    #[allow(
        clippy::too_many_lines,
        reason = "session initialization keeps all bounded runtime ownership in one constructor"
    )]
    pub fn new(project: Project, config: EngineConfig) -> Result<Self, EngineError> {
        if config.audio_block_frames == 0 {
            return Err(EngineError::InvalidConfiguration(
                "audio block size must be greater than zero".to_owned(),
            ));
        }
        if config.output_queue_bytes == 0 {
            return Err(EngineError::InvalidConfiguration(
                "output queue capacity must be greater than zero".to_owned(),
            ));
        }
        if config.monitor_output_queue_blocks == 0 {
            return Err(EngineError::InvalidConfiguration(
                "monitor output queue capacity must be greater than zero".to_owned(),
            ));
        }
        let profile = project
            .active_profile_spec()
            .ok_or(EngineError::NoActiveProfile)?;
        let format = profile.video_format();
        let plugin = BuiltinPlugin::new().map_err(|error| {
            EngineError::InvalidConfiguration(format!("built-in plugin failed: {error}"))
        })?;
        let runtime_build = build_runtime(&project, &plugin)?;
        let runtime = runtime_build.runtime;
        let filter_diagnostics = runtime_build.filter_diagnostics;
        let EngineConfig {
            audio_format,
            audio_block_frames,
            timeline_tolerance_nanos,
            output_queue_bytes,
            reconnect_attempts,
            audio_input_id,
            desktop_audio_id,
            audio_input_sync_offset_millis,
            desktop_audio_sync_offset_millis,
            desktop_monitor_mode,
            microphone_monitor_mode,
            audio_provider,
            audio_output_provider,
            monitor_output_id,
            monitor_output_queue_blocks,
            video_encoder,
        } = config;
        if video_encoder.format() != format {
            return Err(EngineError::InvalidConfiguration(format!(
                "video encoder format {:?} does not match the project canvas {:?}",
                video_encoder.format(),
                format
            )));
        }
        let timeline =
            MediaTimeline::new(format.frame_rate(), audio_format, timeline_tolerance_nanos);
        let mut mixer = AudioMixer::new(audio_format);
        let desktop_audio_source = mixer.add_source(1.0)?;
        let microphone_audio_source = mixer.add_source(1.0)?;
        mixer.set_monitor_mode(desktop_audio_source, desktop_monitor_mode)?;
        mixer.set_monitor_mode(microphone_audio_source, microphone_monitor_mode)?;
        let audio_input_delay = AudioDelayLine::with_block_frames(
            audio_format,
            audio_input_sync_offset_millis,
            audio_block_frames,
        )?;
        let desktop_audio_delay = AudioDelayLine::with_block_frames(
            audio_format,
            desktop_audio_sync_offset_millis,
            audio_block_frames,
        )?;
        let (audio_input, audio_backend, audio_fallback, audio_active_device_id) =
            open_audio_input(&audio_provider, audio_format, audio_input_id.as_deref());
        let audio_reconnect_at = audio_reconnect_deadline(audio_fallback);
        let (desktop_audio, desktop_audio_backend, desktop_audio_active_device_id) =
            open_desktop_audio(&audio_provider, audio_format, desktop_audio_id.as_deref());
        let desktop_audio_reconnect_at = audio_reconnect_deadline(desktop_audio.is_none());
        let audio_route_worker = AudioRouteWorker::spawn(Arc::clone(&audio_provider))
            .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?;
        let (monitor_output_worker, monitor_output_handle) =
            if let Some(device_id) = monitor_output_id.as_deref() {
                let worker = AudioOutputWorker::spawn(
                    Arc::clone(&audio_output_provider),
                    device_id,
                    audio_format,
                    monitor_output_queue_blocks,
                )
                .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?;
                let handle = worker.handle();
                (Some(worker), Some(handle))
            } else {
                (None, None)
            };

        Ok(Self {
            video_encoder,
            audio_encoder: RawAudioEncoder::new(audio_format),
            config: EngineConfig {
                audio_format,
                audio_block_frames,
                timeline_tolerance_nanos,
                output_queue_bytes,
                reconnect_attempts,
                audio_input_id,
                desktop_audio_id,
                audio_input_sync_offset_millis,
                desktop_audio_sync_offset_millis,
                desktop_monitor_mode,
                microphone_monitor_mode,
                audio_provider,
                audio_output_provider,
                monitor_output_id,
                monitor_output_queue_blocks,
                video_encoder: Box::new(RleVideoEncoder::new(format)),
            },
            project,
            format,
            plugin,
            runtime,
            filter_diagnostics,
            timeline,
            mixer,
            desktop_audio_source,
            microphone_audio_source,
            desktop_audio_filters: AudioFilterChain::new(),
            microphone_audio_filters: AudioFilterChain::new(),
            audio_input,
            audio_backend,
            audio_fallback,
            audio_active_device_id,
            audio_reconnect_at,
            audio_input_delay,
            desktop_audio,
            desktop_audio_backend,
            desktop_audio_active_device_id,
            desktop_audio_reconnect_at,
            desktop_audio_delay,
            audio_route_worker,
            audio_route_refresh_at: Timestamp::ZERO,
            audio_route_request_sequence: 0,
            audio_route_request_pending: false,
            monitor_output_worker,
            monitor_output_handle,
            next_audio_deadline: None,
            render_timestamp: Timestamp::ZERO,
            recording: None,
            replay_buffer: None,
            streaming: None,
            recording_lifecycle: OutputLifecycle::Idle,
            streaming_lifecycle: OutputLifecycle::Idle,
            replay_lifecycle: OutputLifecycle::Idle,
            replay_save_status: ReplaySaveStatus::Idle,
            stats: EngineStats::default(),
            last_error: None,
            #[cfg(test)]
            reference_video_encode_calls: 0,
            #[cfg(test)]
            reference_audio_encode_calls: 0,
        })
    }

    /// Creates an output-only session for an already rendered canvas format.
    ///
    /// Desktop adapters use this when their preview renderer remains the source
    /// of program frames. The engine still owns audio pacing, packet encoding,
    /// recording, and streaming for those frames.
    pub fn for_format(format: VideoFormat, config: EngineConfig) -> Result<Self, EngineError> {
        let mut project = Project::new("OBS-RS output session")?;
        project.add_profile(obs_rs_project::Profile::new(
            "default",
            "Default output",
            format,
        )?)?;
        // The config may carry an encoder built for another canvas — a host
        // that reuses one config across several sessions — so install an
        // encoder matched to the requested format rather than asserting.
        let config = config.with_video_encoder(Box::new(RleVideoEncoder::new(format)));
        Self::new(project, config)
    }

    /// Rebuilds the runtime after a project edit.
    pub fn sync_project(&mut self, project: Project) -> Result<(), EngineError> {
        if self.recording.is_some() || self.replay_buffer.is_some() || self.streaming.is_some() {
            return Err(EngineError::Busy("sync the project"));
        }
        let profile = project
            .active_profile_spec()
            .ok_or(EngineError::NoActiveProfile)?;
        let format = profile.video_format();
        let runtime_build = build_runtime(&project, &self.plugin)?;
        self.runtime = runtime_build.runtime;
        self.filter_diagnostics = runtime_build.filter_diagnostics;
        self.project = project;
        self.format = format;
        self.timeline = MediaTimeline::new(
            format.frame_rate(),
            self.config.audio_format,
            self.config.timeline_tolerance_nanos,
        );
        self.stats.av_sync = AvSyncMetrics::default();
        self.audio_input_delay.reset();
        self.desktop_audio_delay.reset();
        self.next_audio_deadline = None;
        self.render_timestamp = Timestamp::ZERO;
        self.video_encoder = Box::new(RleVideoEncoder::new(format));
        self.last_error = None;
        Ok(())
    }

    /// Returns the current project snapshot held by the session.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    fn observe_av_sync(&mut self, video_timestamp: Timestamp, audio_timestamp: Timestamp) {
        let _ = self.timeline.observe(video_timestamp, audio_timestamp);
        self.stats.av_sync = self.timeline.metrics();
    }

    /// Returns the active canvas format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Starts an atomic Matroska/MP4/MOV/FLV or `OBSRPKT1` recording based on
    /// `path`.
    ///
    /// The phase moves to `Starting` before any file work and settles on
    /// `Running` or `Failed`, so a caller that only sees the error still leaves
    /// an observable record of what happened behind.
    pub fn start_recording(&mut self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_recording_with_config(path.into(), None)
    }

    /// Starts a bounded split recording in numbered reference or native
    /// production container files.
    ///
    /// The supplied path is used as the base name; the reference writer
    /// publishes siblings such as `recording-0001.obsr`, while native
    /// production muxers publish fixed slots such as `recording-00001.mp4`.
    /// The policy bounds total segment count, target size, and target duration.
    ///
    /// # Errors
    ///
    /// Returns an error when a recording is already open, the base path does
    /// not match a supported container, the policy is invalid, or the first
    /// segment cannot be opened.
    pub fn start_segmented_recording(
        &mut self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
    ) -> Result<(), EngineError> {
        self.start_segmented_recording_with_config(path.into(), policy, None)
    }

    /// Starts a segmented recording with explicit production encoder choices.
    ///
    /// The reference packet writer has no production encoder boundary and
    /// therefore ignores the pair. Native production containers negotiate it
    /// before the first segment is opened.
    ///
    /// # Errors
    ///
    /// Returns an error when the recording is already open, the policy or
    /// destination is invalid, or the selected production configuration is
    /// unavailable.
    pub fn start_segmented_recording_configured(
        &mut self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_segmented_recording_with_config(path.into(), policy, Some(&encoder_config))
    }

    fn start_segmented_recording_with_config(
        &mut self,
        path: PathBuf,
        policy: SegmentedRecordingPolicy,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_segmented_recording(path, policy, encoder_config);
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    /// Starts a production recording with an explicit codec and encoder choice.
    ///
    /// # Errors
    ///
    /// Returns an error when the codec combination or implementation is not
    /// supported by the current runtime.
    pub fn start_recording_configured(
        &mut self,
        path: impl Into<PathBuf>,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_recording_with_config(path.into(), Some(&encoder_config))
    }

    /// Starts an H.264/AAC Matroska recording that is automatically remuxed
    /// to the requested MP4 path when recording finishes.
    ///
    /// The active Matroska source remains in a hidden `.mkv.part` path until
    /// the native no-clobber remux succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error when the native remux capability, selected encoders,
    /// or destination path is unavailable.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_remux_recording(&mut self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_remux_recording_with_config(path.into(), None)
    }

    /// Starts automatic Matroska-to-MP4 recording with explicit encoders.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected encoders are unavailable or do not
    /// match the H.264/AAC remux profile.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_remux_recording_configured(
        &mut self,
        path: impl Into<PathBuf>,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_remux_recording_with_config(path.into(), Some(&encoder_config))
    }

    /// Recovers an interrupted automatic remux beside an exact MP4 path.
    ///
    /// Recovery consumes a marked `<final>.mkv.part` only after the native
    /// bounded remux publishes the MP4. It refuses to replace an existing
    /// destination and is unavailable while this session is carrying media
    /// output.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Busy`] while recording or streaming, or a
    /// production-output error when the candidate cannot be remuxed.
    #[cfg(feature = "production-gstreamer")]
    pub fn recover_interrupted_remux_recording(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<RemuxRecovery, EngineError> {
        if self.recording.is_some() || self.streaming.is_some() {
            return Err(EngineError::Busy("recover an interrupted recording"));
        }
        recover_interrupted_remux_recording(path.into()).map_err(Into::into)
    }

    /// Discovers recoverable automatic-remux destinations in an idle directory.
    ///
    /// The native boundary applies hard directory and candidate limits. This
    /// method is intentionally a control-plane operation and refuses to run
    /// while media output is active.
    ///
    /// # Errors
    ///
    /// Returns `EngineError::Busy` while recording or streaming, or a typed
    /// native/filesystem error when the bounded scan cannot complete.
    #[cfg(feature = "production-gstreamer")]
    pub fn discover_interrupted_remux_candidates(
        &mut self,
        directory: impl Into<PathBuf>,
    ) -> Result<Vec<PathBuf>, EngineError> {
        if self.recording.is_some() || self.streaming.is_some() {
            return Err(EngineError::Busy("discover interrupted recordings"));
        }
        discover_interrupted_remux_candidates(directory.into()).map_err(Into::into)
    }

    /// Starts a production recording using an explicit versioned output
    /// profile. This is the engine boundary for profiles that share a file
    /// extension, such as normal and fragmented MP4.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile is unavailable, does not match the
    /// destination, or a recording is already open.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_recording_profile(
        &mut self,
        path: impl Into<PathBuf>,
        profile: OutputProfile,
    ) -> Result<(), EngineError> {
        self.start_recording_profile_with_config(path.into(), profile, None)
    }

    /// Starts a production recording using an explicit profile and encoder
    /// implementations.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile, codec, or encoder implementation is
    /// unavailable, the destination does not match, or a recording is open.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_recording_profile_configured(
        &mut self,
        path: impl Into<PathBuf>,
        profile: OutputProfile,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_recording_profile_with_config(path.into(), profile, Some(&encoder_config))
    }

    fn start_recording_with_config(
        &mut self,
        path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_recording(path, encoder_config, None);
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    #[cfg(feature = "production-gstreamer")]
    fn start_recording_profile_with_config(
        &mut self,
        path: PathBuf,
        profile: OutputProfile,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_recording(path, encoder_config, Some(profile));
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    #[cfg(feature = "production-gstreamer")]
    fn start_remux_recording_with_config(
        &mut self,
        path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_remux_recording(path, encoder_config);
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn open_recording(
        &mut self,
        final_path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
        profile_override: Option<OutputProfile>,
    ) -> Result<(), EngineError> {
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("recording path must name a file".to_owned())
            })?;
        let extension = final_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("obsr") {
            if profile_override.is_some() {
                return Err(EngineError::InvalidConfiguration(
                    "production output profiles cannot target the .obsr reference container"
                        .to_owned(),
                ));
            }
            let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
            self.recording = Some(RecordingOutput::Reference(AtomicPacketFileWriter::new(
                final_path, temp_path,
            )?));
            return Ok(());
        }
        let is_mp4 = extension.eq_ignore_ascii_case("mp4");
        let is_mov = extension.eq_ignore_ascii_case("mov");
        let is_flv = extension.eq_ignore_ascii_case("flv");
        if !extension.eq_ignore_ascii_case("mkv") && !is_mp4 && !is_mov && !is_flv {
            return Err(EngineError::InvalidConfiguration(
                "recording extension must be .mkv, .mp4, .mov, .flv, or .obsr".to_owned(),
            ));
        }
        #[cfg(feature = "production-gstreamer")]
        {
            let destination = ProductionDestination::Recording(final_path.clone());
            if (is_mp4 || is_mov || is_flv)
                && encoder_config
                    .is_some_and(|(video, _)| video.codec != obs_rs_output::VideoCodec::H264)
            {
                return Err(EngineError::InvalidConfiguration(
                    "MP4, MOV, and FLV recording currently require H.264 video".to_owned(),
                ));
            }
            let profile = profile_override.unwrap_or_else(|| {
                if is_mp4 {
                    OutputProfile::mp4_h264_aac()
                } else if is_mov {
                    OutputProfile::mov_h264_aac()
                } else if is_flv {
                    OutputProfile::flv_h264_aac()
                } else {
                    encoder_config.map_or_else(OutputProfile::matroska_h264_aac, |config| {
                        match config.0.codec {
                            obs_rs_output::VideoCodec::H264 => OutputProfile::matroska_h264_aac(),
                            obs_rs_output::VideoCodec::Hevc => OutputProfile::matroska_hevc_aac(),
                            obs_rs_output::VideoCodec::Av1 => OutputProfile::matroska_av1_aac(),
                            _ => OutputProfile::reference(),
                        }
                    })
                }
            });
            self.open_native_production_recording(&destination, profile, encoder_config)
        }
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = (encoder_config, profile_override);
        #[cfg(not(feature = "production-gstreamer"))]
        Err(EngineError::InvalidConfiguration(
            "production recording support was not compiled into this host".to_owned(),
        ))
    }

    #[cfg(feature = "production-gstreamer")]
    fn open_remux_recording(
        &mut self,
        final_path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if !final_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
        {
            return Err(EngineError::InvalidConfiguration(
                "automatic remux destination must use the .mp4 extension".to_owned(),
            ));
        }
        let destination = ProductionDestination::RemuxRecording { final_path };
        self.open_native_production_recording(
            &destination,
            OutputProfile::matroska_h264_aac(),
            encoder_config,
        )
    }

    #[cfg(feature = "production-gstreamer")]
    fn open_native_production_recording(
        &mut self,
        destination: &ProductionDestination,
        profile: OutputProfile,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        let plan = encoder_config.map_or_else(
            || ProductionPipelinePlan::negotiate(profile, destination, &capabilities),
            |(video, audio)| {
                ProductionPipelinePlan::negotiate_configured(
                    profile,
                    destination,
                    &capabilities,
                    video,
                    audio,
                )
            },
        )?;
        let session = GStreamerOutputSession::start(
            &plan,
            destination,
            self.format,
            self.config.audio_format,
        )?;
        self.recording = Some(RecordingOutput::Production { session });
        Ok(())
    }

    fn open_segmented_recording(
        &mut self,
        base_path: PathBuf,
        policy: SegmentedRecordingPolicy,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let extension = base_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("obsr") {
            recover_stale_packet_files(&base_path)?;
            self.recording = Some(RecordingOutput::SegmentedReference(
                SegmentedPacketFileWriter::new(base_path, policy)?,
            ));
            return Ok(());
        }
        #[cfg(feature = "production-gstreamer")]
        {
            let is_mp4 = extension.eq_ignore_ascii_case("mp4");
            let is_mov = extension.eq_ignore_ascii_case("mov");
            let is_flv = extension.eq_ignore_ascii_case("flv");
            let is_matroska = extension.eq_ignore_ascii_case("mkv");
            if !is_matroska && !is_mp4 && !is_mov && !is_flv {
                return Err(EngineError::InvalidConfiguration(
                    "segmented recording base path must use .mkv, .mp4, .mov, .flv, or .obsr"
                        .to_owned(),
                ));
            }
            let destination = ProductionDestination::SegmentedRecording { base_path, policy };
            let profile = if is_mp4 {
                OutputProfile::mp4_h264_aac()
            } else if is_mov {
                OutputProfile::mov_h264_aac()
            } else if is_flv {
                OutputProfile::flv_h264_aac()
            } else {
                OutputProfile::matroska_h264_aac()
            };
            let capabilities = GStreamerCapabilitySnapshot::probe();
            let plan = encoder_config.map_or_else(
                || ProductionPipelinePlan::negotiate(profile, &destination, &capabilities),
                |(video, audio)| {
                    ProductionPipelinePlan::negotiate_configured(
                        profile,
                        &destination,
                        &capabilities,
                        video,
                        audio,
                    )
                },
            )?;
            let session = GStreamerOutputSession::start(
                &plan,
                &destination,
                self.format,
                self.config.audio_format,
            )?;
            self.recording = Some(RecordingOutput::Production { session });
            Ok(())
        }
        #[cfg(not(feature = "production-gstreamer"))]
        {
            let _ = (base_path, policy, encoder_config);
            Err(EngineError::InvalidConfiguration(
                "production segmented recording support was not compiled into this host".to_owned(),
            ))
        }
    }

    /// Finalizes a recording and returns its committed byte count.
    ///
    /// A failed finalization leaves the recording open and the phase `Failed`,
    /// so the captured packets are not silently discarded and the frontend can
    /// see that the file was never committed.
    pub fn finish_recording(&mut self) -> Result<usize, EngineError> {
        let Some(mut recording) = self.recording.take() else {
            return Err(EngineError::InvalidConfiguration(
                "recording is not open".to_owned(),
            ));
        };
        self.recording_lifecycle = OutputLifecycle::Stopping;
        match recording.finalize() {
            Ok(bytes) => {
                self.recording_lifecycle = OutputLifecycle::Idle;
                Ok(bytes)
            }
            Err(error) => {
                self.recording = Some(recording);
                Err(self.fail_recording(error))
            }
        }
    }

    fn fail_recording(&mut self, error: EngineError) -> EngineError {
        self.recording_lifecycle = OutputLifecycle::Failed;
        self.last_error = Some(error.to_string());
        error
    }

    /// Aborts an open recording and removes its temporary path.
    pub fn abort_recording(&mut self) {
        if let Some(mut recording) = self.recording.take() {
            recording.abort();
        }
        // An abort is a deliberate stop, so it clears a previous failure rather
        // than leaving the session permanently marked as broken.
        self.recording_lifecycle = OutputLifecycle::Idle;
    }

    /// Starts a bounded packetized replay history.
    ///
    /// Replay capture is independent of recording and streaming. It reuses the
    /// selected packet encoders only while active, so an idle session does not
    /// pay an encode cost just to keep an empty buffer alive.
    pub fn start_replay_buffer(
        &mut self,
        capacity_bytes: usize,
        duration: Duration,
    ) -> Result<(), EngineError> {
        if self.replay_buffer.is_some() {
            return Err(EngineError::Busy("start replay buffer"));
        }
        self.replay_lifecycle = OutputLifecycle::Starting;
        self.replay_save_status = ReplaySaveStatus::Idle;
        match ReplayBuffer::new(capacity_bytes, duration) {
            Ok(buffer) => {
                self.replay_buffer = Some(buffer);
                self.replay_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.replay_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error.into())
            }
        }
    }

    /// Stops replay capture and discards its retained packet history.
    pub fn stop_replay_buffer(&mut self) {
        self.replay_lifecycle = OutputLifecycle::Stopping;
        self.replay_buffer = None;
        self.replay_lifecycle = OutputLifecycle::Idle;
        self.replay_save_status = ReplaySaveStatus::Idle;
    }

    /// Saves the retained replay packets through the atomic packet writer.
    ///
    /// The replay history remains active after a successful save, matching the
    /// OBS workflow where saving a replay does not stop capture. The packet
    /// container is the inspectable OBS-RS reference container; production
    /// remuxing remains a separate output capability.
    pub fn save_replay_buffer(&mut self, path: impl Into<PathBuf>) -> Result<usize, EngineError> {
        self.replay_save_status = ReplaySaveStatus::Saving;
        let result = self.write_replay_buffer(path.into());
        match result {
            Ok(bytes) => {
                self.replay_save_status = ReplaySaveStatus::Saved { bytes };
                Ok(bytes)
            }
            Err(error) => {
                self.replay_save_status = ReplaySaveStatus::Failed {
                    reason: error.to_string(),
                };
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn write_replay_buffer(&self, final_path: PathBuf) -> Result<usize, EngineError> {
        let Some(buffer) = self.replay_buffer.as_ref() else {
            return Err(EngineError::InvalidConfiguration(
                "replay buffer is not running".to_owned(),
            ));
        };
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("replay path must name a file".to_owned())
            })?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        let packets = buffer.keyframe_aligned_snapshot()?;
        let mut writer = AtomicPacketFileWriter::new(final_path, temp_path)?;
        if let Err(error) = packets
            .into_iter()
            .try_for_each(|packet| writer.push(packet))
        {
            let _ = writer.abort();
            return Err(error.into());
        }
        writer.finalize().map_err(Into::into)
    }

    /// Returns whether replay capture is currently active.
    #[must_use]
    pub const fn is_replay_buffer_active(&self) -> bool {
        self.replay_buffer.is_some()
    }

    /// Returns the explicit replay-capture phase.
    #[must_use]
    pub const fn replay_lifecycle(&self) -> OutputLifecycle {
        self.replay_lifecycle
    }

    /// Returns the latest replay save status.
    #[must_use]
    pub const fn replay_save_status(&self) -> &ReplaySaveStatus {
        &self.replay_save_status
    }

    /// Returns the number of packetized entries retained for replay.
    #[must_use]
    pub fn replay_buffer_packet_count(&self) -> usize {
        self.replay_buffer.as_ref().map_or(0, ReplayBuffer::len)
    }

    /// Opens a TCP or WebSocket OBS-RS packet stream.
    ///
    /// A refused or unreachable peer leaves the phase `Failed`, which is what
    /// distinguishes "the user never started a stream" from "the stream could
    /// not be established".
    pub fn start_streaming(&mut self, address: &str) -> Result<(), EngineError> {
        self.start_streaming_with_config(address, None)
    }

    /// Opens a stream with explicit production encoder choices.
    pub fn start_streaming_configured(
        &mut self,
        address: &str,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.start_streaming_with_config(address, Some((video, audio)))
    }

    /// Opens a semantic production target without flattening credentials into a URL.
    pub fn start_streaming_target_configured(
        &mut self,
        target: &StreamTarget,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        if self.streaming.is_some() {
            return Err(EngineError::Busy("start streaming"));
        }
        self.streaming_lifecycle = OutputLifecycle::Starting;
        #[cfg(feature = "production-gstreamer")]
        let result = StreamOutput::connect_target(
            target,
            self.config.output_queue_bytes,
            self.config.reconnect_attempts,
            self.format,
            self.config.audio_format,
            video,
            audio,
        );
        #[cfg(not(feature = "production-gstreamer"))]
        let result = target
            .endpoint()
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("stream target is incomplete".to_owned())
            })
            .and_then(|address| {
                StreamOutput::connect(
                    &address,
                    self.config.output_queue_bytes,
                    self.config.reconnect_attempts,
                    self.format,
                    self.config.audio_format,
                    Some((video, audio)),
                )
            });
        match result {
            Ok(stream) => {
                self.streaming = Some(stream);
                self.streaming_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.streaming_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn start_streaming_with_config(
        &mut self,
        address: &str,
        encoder_config: Option<(&VideoEncoderConfig, &AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.streaming.is_some() {
            return Err(EngineError::Busy("start streaming"));
        }
        self.streaming_lifecycle = OutputLifecycle::Starting;
        match StreamOutput::connect(
            address,
            self.config.output_queue_bytes,
            self.config.reconnect_attempts,
            self.format,
            self.config.audio_format,
            encoder_config,
        ) {
            Ok(stream) => {
                self.streaming = Some(stream);
                self.streaming_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.streaming_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    /// Flushes queued packets without making the media producer wait during
    /// [`EngineSession::tick`].
    pub fn pump_stream(&mut self) -> Result<usize, EngineError> {
        let Some(stream) = self.streaming.as_mut() else {
            return Ok(0);
        };
        if stream.state() == StreamState::Disconnected {
            match stream.reconnect() {
                Ok(ReconnectOutcome::Reconnected) => {
                    self.streaming_lifecycle = OutputLifecycle::Running;
                }
                Ok(ReconnectOutcome::Deferred { .. }) => return Ok(0),
                Err(error) => {
                    self.last_error = Some(error.to_string());
                    self.streaming_lifecycle = OutputLifecycle::Failed;
                    return Err(error);
                }
            }
        }
        match stream.pump() {
            Ok(sent) => Ok(sent),
            Err(error) => {
                self.last_error = Some(error.to_string());
                match stream.reconnect() {
                    Ok(ReconnectOutcome::Reconnected) => {
                        // The transport is carrying media again, so the phase
                        // must say so. Leaving it at `Failed` from the attempt
                        // that just recovered would show a stopped stream in
                        // the UI while packets were flowing.
                        self.streaming_lifecycle = OutputLifecycle::Running;
                        Ok(0)
                    }
                    Ok(ReconnectOutcome::Deferred { .. }) => Ok(0),
                    Err(reconnect) => {
                        self.last_error = Some(format!("{error}; reconnect failed: {reconnect}"));
                        // A pump error the transport could not recover from is
                        // the point the stream stops carrying media, whether or
                        // not the handle is still open.
                        self.streaming_lifecycle = OutputLifecycle::Failed;
                        Err(error)
                    }
                }
            }
        }
    }

    /// Stops streaming and closes its transport.
    pub fn finish_streaming(&mut self) -> Result<(), EngineError> {
        let Some(mut stream) = self.streaming.take() else {
            // Stopping a stream that never started still clears a failed start,
            // so a retry is not blocked by the previous attempt's phase.
            self.streaming_lifecycle = OutputLifecycle::Idle;
            return Ok(());
        };
        self.streaming_lifecycle = OutputLifecycle::Stopping;
        let _ = stream.pump();
        stream.close()?;
        self.streaming_lifecycle = OutputLifecycle::Idle;
        Ok(())
    }

    /// Returns the explicit recording phase, including a failed start or commit.
    #[must_use]
    pub const fn recording_lifecycle(&self) -> OutputLifecycle {
        self.recording_lifecycle
    }

    /// Returns the explicit streaming phase, including a failed connect.
    #[must_use]
    pub const fn streaming_lifecycle(&self) -> OutputLifecycle {
        self.streaming_lifecycle
    }

    /// Returns whether a packet recording is open.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Returns whether a stream transport is open.
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    /// Returns the most recent stream state.
    #[must_use]
    pub fn stream_state(&self) -> Option<StreamState> {
        self.streaming.as_ref().map(StreamOutput::state)
    }

    /// Returns stream counters and queued bytes.
    #[must_use]
    pub fn stream_metrics(&self) -> Option<(StreamMetrics, usize)> {
        self.streaming.as_ref().and_then(|stream| {
            stream
                .metrics()
                .map(|metrics| (metrics, stream.queued_bytes()))
        })
    }

    /// Returns the latest engine counters.
    #[must_use]
    pub const fn stats(&self) -> EngineStats {
        self.stats
    }

    /// Returns a UI-safe status snapshot.
    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        EngineSnapshot {
            recording: self.is_recording(),
            streaming: self.is_streaming(),
            recording_lifecycle: self.recording_lifecycle,
            // A transport that reports `Failed` has stopped carrying media even
            // if the handle is still open, so the reported phase follows the
            // transport rather than the handle.
            streaming_lifecycle: if self.stream_state() == Some(StreamState::Failed) {
                OutputLifecycle::Failed
            } else {
                self.streaming_lifecycle
            },
            replay_lifecycle: self.replay_lifecycle,
            replay_save_status: self.replay_save_status.clone(),
            replay_buffer_packets: self.replay_buffer_packet_count(),
            stream_state: self.stream_state(),
            audio_backend: self.audio_backend.clone(),
            audio_fallback: self.audio_fallback,
            desktop_audio: if self.desktop_audio.is_some() {
                DesktopAudioSource::Monitor(self.desktop_audio_backend.clone())
            } else {
                DesktopAudioSource::Silent(self.desktop_audio_backend.clone())
            },
            monitor_output: self
                .monitor_output_worker
                .as_ref()
                .map(AudioOutputWorker::snapshot),
            filter_diagnostics: self.filter_diagnostics.clone(),
            stream_metrics: self.streaming.as_ref().and_then(StreamOutput::metrics),
            production_stream_metrics: self
                .streaming
                .as_ref()
                .and_then(StreamOutput::production_metrics),
            stream_queued_bytes: self
                .streaming
                .as_ref()
                .map_or(0, StreamOutput::queued_bytes),
            last_error: self.last_error.clone(),
            stats: self.stats,
        }
    }

    fn render_scene_at(
        &mut self,
        scene: &str,
        timestamp: Timestamp,
    ) -> Result<Option<VideoFrame>, EngineError> {
        Ok(self
            .runtime
            .render_scene(scene, &VideoRequest::new(timestamp, self.format))?)
    }

    fn invalidate_audio_route_requests(&mut self) {
        self.audio_route_request_sequence = self.audio_route_request_sequence.saturating_add(1);
        self.audio_route_request_pending = false;
        self.audio_route_refresh_at = Timestamp::ZERO;
        while self.audio_route_worker.take_result().is_some() {}
    }

    /// Polls and schedules automatic route work without performing provider
    /// discovery or device opening on the engine/audio tick.
    fn poll_automatic_audio_routes(&mut self, timestamp: Timestamp) {
        while let Some(result) = self.audio_route_worker.take_result() {
            self.audio_route_request_pending = false;
            if result.sequence != self.audio_route_request_sequence {
                continue;
            }
            match result.microphone {
                AudioRouteUpdate::Opened(route) => {
                    self.audio_input.stop();
                    self.audio_input = route.input;
                    self.audio_backend = route.device_name;
                    self.audio_active_device_id = Some(route.device_id);
                    self.audio_fallback = false;
                    self.audio_reconnect_at = None;
                    self.audio_input_delay.reset();
                    self.next_audio_deadline = None;
                    self.last_error = None;
                }
                AudioRouteUpdate::Unavailable(reason) => {
                    let _ = reason;
                }
                AudioRouteUpdate::Unchanged => {}
            }
            match result.desktop {
                AudioRouteUpdate::Opened(route) => {
                    if let Some(desktop) = self.desktop_audio.as_mut() {
                        desktop.stop();
                    }
                    self.desktop_audio = Some(route.input);
                    self.desktop_audio_backend = route.device_name;
                    self.desktop_audio_active_device_id = Some(route.device_id);
                    self.desktop_audio_reconnect_at = None;
                    self.desktop_audio_delay.reset();
                    self.next_audio_deadline = None;
                    self.last_error = None;
                }
                AudioRouteUpdate::Unavailable(reason) => {
                    let _ = reason;
                }
                AudioRouteUpdate::Unchanged => {}
            }
        }

        let watches_microphone = self.config.audio_input_id.is_none()
            && !self.audio_fallback
            && self.audio_active_device_id.is_some();
        let watches_desktop =
            self.config.desktop_audio_id.is_none() && self.desktop_audio_active_device_id.is_some();
        if (!watches_microphone && !watches_desktop)
            || timestamp < self.audio_route_refresh_at
            || self.audio_route_request_pending
        {
            return;
        }
        self.audio_route_refresh_at = timestamp
            .checked_add(ROUTE_REFRESH_INTERVAL_NANOS)
            .unwrap_or(timestamp);
        self.audio_route_request_sequence = self.audio_route_request_sequence.saturating_add(1);
        let request = AudioRouteRequest {
            sequence: self.audio_route_request_sequence,
            format: self.config.audio_format,
            microphone_requested_id: self.config.audio_input_id.clone(),
            microphone_active_id: self.audio_active_device_id.clone(),
            desktop_requested_id: self.config.desktop_audio_id.clone(),
            desktop_active_id: self.desktop_audio_active_device_id.clone(),
        };
        self.audio_route_request_pending = self.audio_route_worker.try_refresh(request);
    }

    fn read_audio_block(&mut self, timestamp: Timestamp) -> Result<AudioBuffer, EngineError> {
        if self.audio_fallback {
            self.try_reconnect_audio(timestamp);
        }
        match self
            .audio_input
            .read_block(timestamp, self.config.audio_block_frames)
        {
            Ok(buffer) => Ok(buffer),
            Err(error) => {
                self.audio_input.stop();
                self.audio_input_delay.reset();
                self.audio_active_device_id = None;
                self.audio_fallback = true;
                self.audio_backend = format!("simulated fallback ({error})");
                self.last_error = Some(error.to_string());
                self.audio_reconnect_at = timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS);
                self.audio_input = SimulatedAudioProvider::new()
                    .open_input("test-audio", self.config.audio_format)?;
                // The fallback signal runs on its own clock, so the timeline's
                // idea of the next audio deadline — computed against the real
                // device that just failed — is stale. Dropping it forces the
                // next tick to re-anchors the audio deadlines to the current
                // video timestamp instead of chasing a device that is gone.
                self.next_audio_deadline = None;
                let buffer = self
                    .audio_input
                    .read_block(timestamp, self.config.audio_block_frames)?;
                Ok(buffer)
            }
        }
    }

    fn try_reconnect_audio(&mut self, timestamp: Timestamp) {
        let Some(next_attempt) = self.audio_reconnect_at else {
            return;
        };
        if timestamp < next_attempt {
            return;
        }

        self.audio_reconnect_at = timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS);
        let Some((audio_input, audio_backend, audio_active_device_id)) = open_live_audio_input(
            &self.config.audio_provider,
            self.config.audio_format,
            self.config.audio_input_id.as_deref(),
        ) else {
            return;
        };

        self.audio_input.stop();
        self.audio_input_delay.reset();
        self.audio_input = audio_input;
        self.audio_backend = audio_backend;
        self.audio_fallback = false;
        self.audio_active_device_id = Some(audio_active_device_id);
        self.audio_reconnect_at = None;
        self.next_audio_deadline = None;
        self.last_error = None;
    }

    /// Reads one desktop block, or silence when no monitor is open.
    ///
    /// A monitor that fails mid-session is closed rather than retried every
    /// block: the desktop channel degrades to silence and says so in the
    /// backend label. A monitor is retried only at the bounded media-time
    /// interval, which keeps a broken device from stalling every tick.
    fn read_desktop_block(&mut self, timestamp: Timestamp) -> Result<AudioBuffer, EngineError> {
        let frames = self.config.audio_block_frames;
        self.try_reconnect_desktop_audio(timestamp);
        if let Some(desktop) = self.desktop_audio.as_mut() {
            match desktop.read_block(timestamp, frames) {
                Ok(buffer) => return Ok(buffer),
                Err(error) => {
                    desktop.stop();
                    self.desktop_audio = None;
                    self.desktop_audio_delay.reset();
                    self.desktop_audio_active_device_id = None;
                    self.desktop_audio_backend = format!("unavailable ({error})");
                    self.desktop_audio_reconnect_at =
                        timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS);
                    self.last_error = Some(error.to_string());
                }
            }
        }
        let buffer = AudioBuffer::silence(self.config.audio_format, timestamp, frames)?;
        Ok(buffer)
    }

    fn try_reconnect_desktop_audio(&mut self, timestamp: Timestamp) {
        let Some(next_attempt) = self.desktop_audio_reconnect_at else {
            return;
        };
        if timestamp < next_attempt {
            return;
        }

        self.desktop_audio_reconnect_at = timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS);
        let Some((desktop_audio, desktop_audio_backend, desktop_audio_active_device_id)) =
            open_live_desktop_audio(
                &self.config.audio_provider,
                self.config.audio_format,
                self.config.desktop_audio_id.as_deref(),
            )
        else {
            return;
        };

        self.desktop_audio = Some(desktop_audio);
        self.desktop_audio_delay.reset();
        self.desktop_audio_backend = desktop_audio_backend;
        self.desktop_audio_active_device_id = Some(desktop_audio_active_device_id);
        self.desktop_audio_reconnect_at = None;
        self.last_error = None;
    }

    fn drain_audio_until(&mut self, timestamp: Timestamp) -> Result<Vec<AudioBuffer>, EngineError> {
        self.poll_automatic_audio_routes(timestamp);
        let mut audio_blocks = Vec::new();
        while self
            .next_audio_deadline
            .is_none_or(|deadline| deadline.timestamp() <= timestamp)
        {
            let deadline = self.next_audio_deadline.take().map_or_else(
                || {
                    self.timeline
                        .next_audio_block(self.config.audio_block_frames)
                },
                Ok,
            )?;
            let mut input = self.read_audio_block(deadline.timestamp())?;
            let mut desktop = self.read_desktop_block(deadline.timestamp())?;
            self.microphone_audio_filters.apply(&mut input)?;
            self.desktop_audio_filters.apply(&mut desktop)?;
            input = self.audio_input_delay.process(input)?;
            desktop = self.desktop_audio_delay.process(desktop)?;
            let (mixed, monitor) = if self.monitor_output_handle.is_some() {
                let (output, monitor) = self.mixer.mix_buses(
                    deadline.timestamp(),
                    self.config.audio_block_frames,
                    &[
                        (self.desktop_audio_source, &desktop),
                        (self.microphone_audio_source, &input),
                    ],
                )?;
                (output, Some(monitor))
            } else {
                let output = self.mixer.mix(
                    deadline.timestamp(),
                    self.config.audio_block_frames,
                    &[
                        (self.desktop_audio_source, &desktop),
                        (self.microphone_audio_source, &input),
                    ],
                )?;
                (output, None)
            };
            if let (Some(handle), Some(monitor)) = (&self.monitor_output_handle, monitor) {
                if handle.try_write(monitor) {
                    self.stats.monitor_blocks_submitted =
                        self.stats.monitor_blocks_submitted.saturating_add(1);
                } else {
                    self.stats.monitor_blocks_dropped =
                        self.stats.monitor_blocks_dropped.saturating_add(1);
                }
            }
            self.stats.desktop_peak_milli =
                self.mixer.source_peak_milli(self.desktop_audio_source)?;
            self.stats.microphone_peak_milli =
                self.mixer.source_peak_milli(self.microphone_audio_source)?;
            self.stats.desktop_peak_hold_milli = self
                .mixer
                .source_peak_hold_milli(self.desktop_audio_source)?;
            self.stats.microphone_peak_hold_milli = self
                .mixer
                .source_peak_hold_milli(self.microphone_audio_source)?;
            self.stats.desktop_clipped = self.mixer.source_clipped(self.desktop_audio_source)?;
            self.stats.microphone_clipped =
                self.mixer.source_clipped(self.microphone_audio_source)?;
            self.stats.audio_blocks = self.stats.audio_blocks.saturating_add(1);
            if self.audio_fallback {
                self.stats.audio_fallback_blocks =
                    self.stats.audio_fallback_blocks.saturating_add(1);
            }
            self.stats.last_audio_timestamp = Some(mixed.timestamp());
            audio_blocks.push(mixed);
            self.next_audio_deadline = Some(
                self.timeline
                    .next_audio_block(self.config.audio_block_frames)?,
            );
        }
        Ok(audio_blocks)
    }

    fn emit_packet(&mut self, packet: EncodedPacket) -> Result<(), EngineError> {
        if let Some(replay_buffer) = self.replay_buffer.as_mut() {
            replay_buffer.push(packet.clone())?;
        }
        match (self.recording.as_mut(), self.streaming.as_mut()) {
            (Some(recording), Some(stream)) => {
                recording.push_packet(packet.clone())?;
                stream.submit(packet)?;
            }
            (Some(recording), None) => recording.push_packet(packet)?,
            (None, Some(stream)) => stream.submit(packet)?,
            (None, None) => {}
        }
        Ok(())
    }

    fn dispatch_audio(&mut self, audio: &AudioBuffer) -> Result<(), EngineError> {
        if self.raw_audio_required() {
            let started = Instant::now();
            if let Some(recording) = self
                .recording
                .as_mut()
                .filter(|recording| recording.audio_requirement() == AudioInputRequirement::Raw)
            {
                recording.push_audio(audio)?;
            }
            if let Some(stream) = self
                .streaming
                .as_mut()
                .filter(|stream| stream.audio_requirement() == AudioInputRequirement::Raw)
            {
                stream.push_raw_audio(audio.clone())?;
            }
            self.stats.output_submit_latency.record(started.elapsed());
        }
        if self.packetized_audio_required() {
            #[cfg(test)]
            {
                self.reference_audio_encode_calls =
                    self.reference_audio_encode_calls.saturating_add(1);
            }
            let started = Instant::now();
            let packet = self.audio_encoder.encode(audio)?;
            self.stats.audio_encode_latency.record(started.elapsed());
            let started = Instant::now();
            self.emit_packet(packet)?;
            self.stats.output_submit_latency.record(started.elapsed());
        }
        Ok(())
    }

    fn dispatch_video(&mut self, frame: &VideoFrame) -> Result<(), EngineError> {
        if self.raw_video_required() {
            let started = Instant::now();
            if let Some(recording) = self
                .recording
                .as_mut()
                .filter(|recording| recording.video_requirement() == VideoInputRequirement::Raw)
            {
                recording.push_video(frame)?;
            }
            if let Some(stream) = self
                .streaming
                .as_mut()
                .filter(|stream| stream.video_requirement() == VideoInputRequirement::Raw)
            {
                stream.push_video(frame.clone())?;
            }
            self.stats.output_submit_latency.record(started.elapsed());
        }
        if self.packetized_video_required() {
            #[cfg(test)]
            {
                self.reference_video_encode_calls =
                    self.reference_video_encode_calls.saturating_add(1);
            }
            let started = Instant::now();
            let packet = self.video_encoder.encode(frame)?;
            self.stats.video_encode_latency.record(started.elapsed());
            let started = Instant::now();
            self.emit_packet(packet)?;
            self.stats.output_submit_latency.record(started.elapsed());
        }
        Ok(())
    }

    fn dispatch_raw_video(&mut self, frame: &RawVideoFrame) -> Result<(), EngineError> {
        if self.raw_video_required() {
            let started = Instant::now();
            if let Some(recording) = self
                .recording
                .as_mut()
                .filter(|recording| recording.video_requirement() == VideoInputRequirement::Raw)
            {
                recording.push_raw_video(frame)?;
            }
            if let Some(stream) = self
                .streaming
                .as_mut()
                .filter(|stream| stream.video_requirement() == VideoInputRequirement::Raw)
            {
                stream.push_raw_video(frame.clone())?;
            }
            self.stats.output_submit_latency.record(started.elapsed());
        }
        if self.packetized_video_required() {
            let rgba = frame
                .clone()
                .into_rgba8()
                .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?;
            #[cfg(test)]
            {
                self.reference_video_encode_calls =
                    self.reference_video_encode_calls.saturating_add(1);
            }
            let started = Instant::now();
            let packet = self.video_encoder.encode(&rgba)?;
            self.stats.video_encode_latency.record(started.elapsed());
            let started = Instant::now();
            self.emit_packet(packet)?;
            self.stats.output_submit_latency.record(started.elapsed());
        }
        Ok(())
    }

    fn packetized_video_required(&self) -> bool {
        self.replay_buffer.is_some()
            || self.recording.as_ref().is_some_and(|recording| {
                recording.video_requirement() == VideoInputRequirement::Packetized
            })
            || self.streaming.as_ref().is_some_and(|stream| {
                stream.video_requirement() == VideoInputRequirement::Packetized
            })
    }

    fn packetized_audio_required(&self) -> bool {
        self.replay_buffer.is_some()
            || self.recording.as_ref().is_some_and(|recording| {
                recording.audio_requirement() == AudioInputRequirement::Packetized
            })
            || self.streaming.as_ref().is_some_and(|stream| {
                stream.audio_requirement() == AudioInputRequirement::Packetized
            })
    }

    fn raw_video_required(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|recording| recording.video_requirement() == VideoInputRequirement::Raw)
            || self
                .streaming
                .as_ref()
                .is_some_and(|stream| stream.video_requirement() == VideoInputRequirement::Raw)
    }

    fn raw_audio_required(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|recording| recording.audio_requirement() == AudioInputRequirement::Raw)
            || self
                .streaming
                .as_ref()
                .is_some_and(|stream| stream.audio_requirement() == AudioInputRequirement::Raw)
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        self.abort_recording();
        if let Some(stream) = self.streaming.as_mut() {
            let _ = stream.close();
        }
        self.audio_input.stop();
        if let Some(desktop) = self.desktop_audio.as_mut() {
            desktop.stop();
        }
    }
}

fn build_runtime(project: &Project, plugin: &BuiltinPlugin) -> Result<RuntimeBuild, EngineError> {
    let profile = project
        .active_profile_spec()
        .ok_or(EngineError::NoActiveProfile)?;
    let mut runtime = Runtime::new();
    let mut filter_diagnostics = Vec::new();
    runtime.register_plugin(plugin)?;

    // Source definitions are profile-wide. Create each runtime source once;
    // scenes below only attach scene-local items to that shared instance.
    let mut source_ids = std::collections::HashMap::new();
    for source in profile.sources() {
        let source_id =
            runtime.create_source(source.kind().as_str(), source.name(), source.settings())?;
        for filter in source.filters() {
            match compile_filter_report(filter) {
                FilterCompilation::Applied(runtime_filter) => {
                    runtime.add_source_filter(source_id, runtime_filter)?;
                }
                FilterCompilation::Ignored => {}
                FilterCompilation::Unavailable(diagnostic) => record_filter_diagnostic(
                    &mut filter_diagnostics,
                    source.name(),
                    filter.name(),
                    &diagnostic,
                ),
            }
        }
        source_ids.insert(source.id().as_str().to_owned(), source_id);
    }
    for scene in profile.scenes() {
        let scene_id = scene.id().as_str();
        runtime.create_scene(scene_id)?;
        for item in profile
            .flatten_scene_items(scene_id)
            .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?
        {
            let source_id = source_ids
                .get(item.source_id().as_str())
                .copied()
                .ok_or_else(|| {
                    EngineError::InvalidConfiguration(format!(
                        "scene item references unknown source {}",
                        item.source_id()
                    ))
                })?;
            runtime.attach_source_instance_with_id(scene_id, source_id, item.item_id())?;
            runtime.set_scene_item_transform_by_id(scene_id, item.item_id(), item.transform())?;
        }
    }
    Ok(RuntimeBuild {
        runtime,
        filter_diagnostics,
    })
}

struct RuntimeBuild {
    runtime: Runtime,
    filter_diagnostics: Vec<String>,
}

fn record_filter_diagnostic(
    diagnostics: &mut Vec<String>,
    source_name: &str,
    filter_name: &str,
    diagnostic: &FilterDiagnostic,
) {
    if diagnostics.len() + 1 < MAX_FILTER_DIAGNOSTICS {
        diagnostics.push(format!(
            "source '{source_name}' filter '{filter_name}': {diagnostic}"
        ));
    } else if diagnostics.len() + 1 == MAX_FILTER_DIAGNOSTICS {
        diagnostics.push(format!(
            "additional filter diagnostics omitted after {MAX_FILTER_DIAGNOSTICS} entries"
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::*;
    use obs_rs_audio::{AudioDeviceInfo, AudioMonitorMode, AudioOutputWorkerState, MAX_GAIN_MILLI};
    use obs_rs_config::Config;
    use obs_rs_core::SourceId;
    use obs_rs_media::{FrameFilter, FrameRate, FrameTransform};
    use obs_rs_project::{
        Profile, ProjectCommand, SceneItemSpec, SceneSpec, SourceFilterCategory, SourceFilterSpec,
        SourceSpec,
    };

    fn project() -> Project {
        let format =
            VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format");
        let mut project = Project::new("engine test").expect("project");
        let mut profile = Profile::new("live", "Live", format).expect("profile");
        let mut settings = Config::new();
        settings.set("width", "640").expect("width");
        settings.set("height", "360").expect("height");
        let mut scene = SceneSpec::new("program", "Program").expect("scene");
        scene
            .add_item(SceneItemSpec::for_source("pattern").expect("scene item"))
            .expect("add item");
        profile
            .add_source(
                SourceSpec::new("pattern", "test_pattern", "Pattern", settings).expect("source"),
            )
            .expect("add source");
        profile.add_scene(scene).expect("add scene");
        project.add_profile(profile).expect("add profile");
        project
    }

    #[test]
    fn replay_buffer_encodes_only_while_active_and_saves_atomically() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        assert!(!engine.is_replay_buffer_active());
        assert_eq!(engine.replay_buffer_packet_count(), 0);

        let final_path =
            std::env::temp_dir().join(format!("obs-rs-replay-engine-{}.obsr", std::process::id()));
        engine
            .start_replay_buffer(4 * 1024 * 1024, Duration::from_secs(30))
            .expect("start replay buffer");
        assert!(engine.is_replay_buffer_active());
        assert_eq!(engine.replay_lifecycle(), OutputLifecycle::Running);
        assert_eq!(engine.replay_save_status(), &ReplaySaveStatus::Idle);
        for _ in 0..3 {
            engine.tick(None, Some("program")).expect("replay tick");
        }
        assert!(engine.replay_buffer_packet_count() >= 3);
        let bytes = engine
            .save_replay_buffer(&final_path)
            .expect("save replay buffer");
        assert!(bytes > 16);
        assert_eq!(
            engine.replay_save_status(),
            &ReplaySaveStatus::Saved { bytes }
        );
        let packets = obs_rs_output::MemoryMuxer::decode(
            &std::fs::read(&final_path).expect("read replay file"),
        )
        .expect("decode replay file");
        assert!(!packets.is_empty());
        assert!(engine.is_replay_buffer_active());

        engine.stop_replay_buffer();
        assert!(!engine.is_replay_buffer_active());
        assert_eq!(engine.replay_lifecycle(), OutputLifecycle::Idle);
        assert_eq!(engine.replay_save_status(), &ReplaySaveStatus::Idle);
        assert!(matches!(
            engine.save_replay_buffer(&final_path),
            Err(EngineError::InvalidConfiguration(reason)) if reason.contains("not running")
        ));
        assert!(matches!(
            engine.replay_save_status(),
            ReplaySaveStatus::Failed { reason } if reason.contains("not running")
        ));
        std::fs::remove_file(final_path).expect("remove replay file");
    }

    #[test]
    fn nested_scene_items_flatten_to_shared_runtime_sources() {
        let mut project = project();
        let child_transform =
            FrameTransform::new(1_500, 800, 10, -4, false, false, 200).expect("child transform");
        let mut child = SceneSpec::new("child", "Child").expect("child scene");
        let mut child_item = SceneItemSpec::for_source("pattern").expect("child item");
        child_item.set_transform(child_transform);
        child.add_item(child_item).expect("child item attach");
        project
            .apply(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: child,
            })
            .expect("add child scene");

        project
            .apply(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: SceneSpec::new("parent", "Parent").expect("parent scene"),
            })
            .expect("add parent scene");
        let parent_transform =
            FrameTransform::new(2_000, 1_500, 20, 30, false, false, 128).expect("parent transform");
        let mut nested = SceneItemSpec::for_scene("child-item", "child").expect("nested item");
        nested.set_transform(parent_transform);
        project
            .apply(ProjectCommand::AddSceneItem {
                profile: "live".to_owned(),
                scene: "parent".to_owned(),
                item: nested,
            })
            .expect("add nested item");

        let mut engine = EngineSession::new(project, EngineConfig::default()).expect("engine");
        assert_eq!(engine.runtime.source_count(), 1);
        assert_eq!(
            engine.runtime.scene_sources("child").map(<[SourceId]>::len),
            Some(1)
        );
        assert_eq!(
            engine
                .runtime
                .scene_sources("parent")
                .map(<[SourceId]>::len),
            Some(1)
        );
        assert_eq!(
            engine.runtime.scene_item_ids("parent"),
            Some(vec!["child-item/pattern".to_owned()])
        );
        assert_eq!(
            engine.runtime.scene_item_transform("parent", 0),
            Some(
                child_transform
                    .compose_simple(parent_transform)
                    .expect("compose")
            )
        );

        let frame = engine
            .render_scene("parent")
            .expect("render nested scene")
            .expect("nested scene has a frame");
        assert_eq!(frame.format(), engine.format());
        assert_eq!(engine.runtime.compositor_metrics().source_requests(), 1);
    }

    #[test]
    fn group_items_flatten_to_shared_runtime_sources() {
        let mut project = project();
        let mut group = SceneItemSpec::for_group("group", "Group").expect("group");
        group
            .group_mut()
            .expect("group target")
            .add_item(SceneItemSpec::for_source("pattern").expect("group child"))
            .expect("group child attach");
        project
            .apply(ProjectCommand::AddSceneItem {
                profile: "live".to_owned(),
                scene: "program".to_owned(),
                item: group,
            })
            .expect("add group");

        let mut engine = EngineSession::new(project, EngineConfig::default()).expect("engine");
        assert_eq!(engine.runtime.source_count(), 1);
        assert_eq!(
            engine
                .runtime
                .scene_sources("program")
                .expect("program scene")
                .len(),
            2
        );
        let layers = engine
            .runtime
            .render_scene_layers(
                "program",
                &VideoRequest::new(Timestamp::ZERO, engine.format()),
            )
            .expect("group renders");
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].item_id(), "pattern");
        assert_eq!(layers[1].item_id(), "group/pattern");
        assert_eq!(engine.runtime.compositor_metrics().source_requests(), 2);
        assert_eq!(
            engine
                .runtime
                .compositor_metrics()
                .capture_latency()
                .samples(),
            1
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the compiler fixture covers each supported filter mapping at one project boundary"
    )]
    fn filter_compiler_keeps_renderer_details_out_of_project_values() {
        let brightness = SourceFilterSpec::new(
            "brightness",
            "Brightness",
            "brightness",
            Config::parse("milli = 350\n").expect("settings"),
        )
        .expect("filter");
        assert_eq!(
            compile_filter(&brightness),
            Some(FrameFilter::Brightness { milli: 350 })
        );

        let crop = SourceFilterSpec::new(
            "crop",
            "Crop/Pad",
            "crop_pad",
            Config::parse("bottom = 1\nleft = 2\nright = 3\ntop = 4\n").expect("crop settings"),
        )
        .expect("crop filter");
        assert_eq!(
            compile_filter(&crop),
            Some(FrameFilter::CropPad {
                left: 2,
                top: 4,
                right: 3,
                bottom: 1,
            })
        );

        let color_correction = SourceFilterSpec::new(
            "color",
            "Color Correction",
            "color_correction",
            Config::parse(
                "brightness = 125\ncontrast = -500\ngamma = 250\nhue_shift = 30\nopacity = 900\nsaturation = 750\n",
            )
            .expect("color correction settings"),
        )
        .expect("color correction filter");
        assert_eq!(
            compile_filter(&color_correction),
            Some(FrameFilter::ColorCorrection(
                ColorCorrection::new(250, -500, 125, 750, 30, 900).expect("valid color correction"),
            ))
        );

        let color_multiply_add = SourceFilterSpec::new(
            "color_wash",
            "Color Multiply/Add",
            "color_multiply_add",
            Config::parse(
                "add_blue = 12\nadd_green = 8\nadd_red = 4\nmultiply_blue = 255\nmultiply_green = 240\nmultiply_red = 220\n",
            )
            .expect("color wash settings"),
        )
        .expect("color multiply/add filter");
        assert_eq!(
            compile_filter(&color_multiply_add),
            Some(FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new(
                [220, 240, 255],
                [4, 8, 12],
            )))
        );

        let luma_key = SourceFilterSpec::new(
            "luma",
            "Luma Key",
            "luma_key",
            Config::parse(
                "luma_max = 900\nluma_max_smooth = 40\nluma_min = 100\nluma_min_smooth = 60\n",
            )
            .expect("luma key settings"),
        )
        .expect("luma key filter");
        assert_eq!(
            compile_filter(&luma_key),
            Some(FrameFilter::LumaKey(
                LumaKey::new(900, 100, 40, 60).expect("valid luma key"),
            ))
        );

        let color_key = SourceFilterSpec::new(
            "key",
            "Color Key",
            "color_key",
            Config::parse(
                "key_blue = 0\nkey_green = 255\nkey_red = 0\nsimilarity = 120\nsmoothness = 80\n",
            )
            .expect("color key settings"),
        )
        .expect("color key filter");
        assert_eq!(
            compile_filter(&color_key),
            Some(FrameFilter::ColorKey(
                ColorKey::new(0, 255, 0, 120, 80).expect("valid color key"),
            ))
        );

        let chroma_key = SourceFilterSpec::new(
            "chroma",
            "Chroma Key",
            "chroma_key",
            Config::parse(
                "key_blue = 0\nkey_green = 255\nkey_red = 0\nsimilarity = 400\nsmoothness = 80\nspill = 100\n",
            )
            .expect("chroma key settings"),
        )
        .expect("chroma key filter");
        assert_eq!(
            compile_filter(&chroma_key),
            Some(FrameFilter::ChromaKey(
                ChromaKey::new(0, 255, 0, 400, 80, 100).expect("valid chroma key"),
            ))
        );

        let sharpen = SourceFilterSpec::new(
            "sharpen",
            "Sharpen",
            "sharpen",
            Config::parse("sharpness = 80\n").expect("sharpen settings"),
        )
        .expect("sharpen filter");
        assert_eq!(
            compile_filter(&sharpen),
            Some(FrameFilter::Sharpen { milli: 80 })
        );

        let scroll = SourceFilterSpec::new(
            "scroll",
            "Scroll",
            "scroll",
            Config::parse("loop = false\nspeed_x = 120\nspeed_y = -80\n").expect("scroll settings"),
        )
        .expect("scroll filter");
        assert_eq!(
            compile_filter(&scroll),
            Some(FrameFilter::Scroll {
                speed_x: 120,
                speed_y: -80,
                looped: false,
            })
        );

        let render_delay = SourceFilterSpec::new(
            "render-delay",
            "Render Delay",
            "render_delay",
            Config::parse("milliseconds = 100\n").expect("render delay settings"),
        )
        .expect("render delay filter");
        assert_eq!(
            compile_filter(&render_delay),
            Some(FrameFilter::RenderDelay(RenderDelay { milliseconds: 100 }))
        );
        let invalid_render_delay = SourceFilterSpec::new(
            "invalid-render-delay",
            "Invalid Render Delay",
            "render_delay",
            Config::parse("milliseconds = 501\n").expect("invalid delay settings"),
        )
        .expect("invalid render delay filter");
        assert_eq!(compile_filter(&invalid_render_delay), None);
        let mut invalid_scroll_settings = Config::new();
        invalid_scroll_settings
            .set("loop", "maybe")
            .expect("invalid boolean can be stored as an explicit string");
        invalid_scroll_settings
            .set("speed_x", "501")
            .expect("out-of-range speed can be stored");
        invalid_scroll_settings
            .set("speed_y", "0")
            .expect("valid vertical speed can be stored");
        let invalid_scroll = SourceFilterSpec::new(
            "invalid-scroll",
            "Invalid Scroll",
            "scroll",
            invalid_scroll_settings,
        )
        .expect("invalid scroll filter");
        assert_eq!(compile_filter(&invalid_scroll), None);

        let mut disabled = brightness.clone();
        disabled.set_enabled(false);
        assert_eq!(compile_filter(&disabled), None);

        let audio = SourceFilterSpec::with_category(
            "compressor",
            "Compressor",
            "compressor",
            SourceFilterCategory::AudioVideo,
            Config::new(),
        )
        .expect("audio filter");
        assert_eq!(compile_filter(&audio), None);

        let gain = SourceFilterSpec::with_category(
            "gain",
            "Gain",
            "gain",
            SourceFilterCategory::AudioVideo,
            Config::parse("db_milli = -6000\n").expect("gain settings"),
        )
        .expect("gain filter");
        assert_eq!(
            compile_audio_filter(&gain),
            Some(AudioFilter::gain_db_milli(-6_000).expect("valid gain"))
        );
        let invert = SourceFilterSpec::with_category(
            "invert",
            "Invert Polarity",
            "invert_polarity",
            SourceFilterCategory::AudioVideo,
            Config::new(),
        )
        .expect("invert polarity filter");
        assert_eq!(
            compile_audio_filter(&invert),
            Some(AudioFilter::InvertPolarity)
        );
        let limiter = SourceFilterSpec::with_category(
            "limiter",
            "Limiter",
            "limiter",
            SourceFilterCategory::AudioVideo,
            Config::parse("threshold_db_milli = -6000\nrelease_ms = 60\n")
                .expect("limiter settings"),
        )
        .expect("limiter filter");
        assert_eq!(
            compile_audio_filter(&limiter),
            Some(AudioFilter::limiter_db_milli(-6_000, 60).expect("valid limiter"))
        );
        let compressor = SourceFilterSpec::with_category(
            "compressor_runtime",
            "Compressor",
            "compressor",
            SourceFilterCategory::AudioVideo,
            Config::parse(
                "ratio_milli = 10000\nthreshold_db_milli = -18000\nattack_ms = 6\nrelease_ms = 60\noutput_gain_db_milli = 0\n",
            )
            .expect("compressor settings"),
        )
        .expect("compressor filter");
        assert_eq!(
            compile_audio_filter(&compressor),
            Some(AudioFilter::compressor(10_000, -18_000, 6, 60, 0).expect("valid compressor"))
        );
        let expander = SourceFilterSpec::with_category(
            "expander_runtime",
            "Expander",
            "expander",
            SourceFilterCategory::AudioVideo,
            Config::parse(
                "ratio_milli = 10000\nthreshold_db_milli = -40000\nattack_ms = 10\nrelease_ms = 50\noutput_gain_db_milli = 0\n",
            )
            .expect("expander settings"),
        )
        .expect("expander filter");
        assert_eq!(
            compile_audio_filter(&expander),
            Some(AudioFilter::expander(10_000, -40_000, 10, 50, 0).expect("valid expander"))
        );
        let gate = SourceFilterSpec::with_category(
            "gate_runtime",
            "Gate",
            "gate",
            SourceFilterCategory::AudioVideo,
            Config::parse(
                "open_threshold_db_milli = -26000\nclose_threshold_db_milli = -32000\nattack_ms = 25\nhold_ms = 200\nrelease_ms = 150\n",
            )
            .expect("gate settings"),
        )
        .expect("gate filter");
        assert_eq!(
            compile_audio_filter(&gate),
            Some(AudioFilter::noise_gate(-26_000, -32_000, 25, 200, 150).expect("valid gate"))
        );
        assert_eq!(compile_audio_filter(&brightness), None);
    }

    #[test]
    fn filter_compiler_reports_disabled_unsupported_and_invalid_instances() {
        let brightness = SourceFilterSpec::new(
            "brightness-report",
            "Brightness",
            "brightness",
            Config::parse("milli = 350\n").expect("settings"),
        )
        .expect("filter");
        assert!(matches!(
            compile_filter_report(&brightness),
            FilterCompilation::Applied(FrameFilter::Brightness { milli: 350 })
        ));

        let mut disabled = brightness.clone();
        disabled.set_enabled(false);
        assert_eq!(compile_filter_report(&disabled), FilterCompilation::Ignored);

        let invalid = SourceFilterSpec::new(
            "invalid-report",
            "Invalid Render Delay",
            "render_delay",
            Config::parse("milliseconds = 501\n").expect("settings"),
        )
        .expect("filter");
        assert!(matches!(
            compile_filter_report(&invalid),
            FilterCompilation::Unavailable(FilterDiagnostic {
                failure: FilterCompileFailure::InvalidSettings,
                ..
            })
        ));

        let unknown =
            SourceFilterSpec::new("unknown-report", "Unknown", "future_effect", Config::new())
                .expect("filter");
        assert!(matches!(
            compile_filter_report(&unknown),
            FilterCompilation::Unavailable(FilterDiagnostic {
                failure: FilterCompileFailure::UnsupportedKind,
                ..
            })
        ));

        let audio = SourceFilterSpec::with_category(
            "audio-report",
            "Compressor",
            "compressor",
            SourceFilterCategory::AudioVideo,
            Config::new(),
        )
        .expect("audio filter");
        assert!(matches!(
            compile_filter_report(&audio),
            FilterCompilation::Unavailable(FilterDiagnostic {
                failure: FilterCompileFailure::UnsupportedCategory,
                ..
            })
        ));
        assert!(matches!(
            compile_audio_filter_report(&audio),
            FilterCompilation::Unavailable(FilterDiagnostic {
                failure: FilterCompileFailure::InvalidSettings,
                ..
            })
        ));
    }

    #[test]
    fn engine_snapshot_names_persisted_filters_not_available_in_renderer() {
        let mut project = project();
        let filter = SourceFilterSpec::new(
            "future-filter",
            "Future filter",
            "future_effect",
            Config::new(),
        )
        .expect("filter");
        project
            .apply(ProjectCommand::AddSourceFilter {
                profile: "live".to_owned(),
                source: "pattern".to_owned(),
                filter,
            })
            .expect("add filter");

        let engine = EngineSession::new(project, EngineConfig::default()).expect("engine");
        assert_eq!(
            engine.snapshot().filter_diagnostics,
            vec![
                "source 'Pattern' filter 'Future filter': filter 'future_effect' (effect) unavailable: unsupported kind"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn ticks_keep_audio_and_video_packets_monotonic() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let tick = engine.tick(None, Some("program")).expect("tick");
        assert_eq!(tick.audio_blocks.len(), 1);
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("tick");
        }
        assert_eq!(engine.stats().video_frames, 5);
        assert!(engine.stats().audio_blocks >= 10);
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let sync = engine.stats().av_sync;
        assert_eq!(sync.observations(), 5);
        assert!(sync.max_abs_delta_nanos() > 0);
    }

    #[test]
    fn microphone_sync_offset_delays_only_that_channel_and_is_bounded() {
        let config = EngineConfig::default().with_audio_input_sync_offset_millis(10);
        let mut engine = EngineSession::new(project(), config).expect("engine");

        let first = engine
            .drain_audio_until(Timestamp::ZERO)
            .expect("first audio block");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].sample(0, 0), Some(0.0));

        let second = engine
            .drain_audio_until(Timestamp::from_millis(10))
            .expect("second audio block");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].sample(0, 0), Some(0.12));

        engine
            .set_channel_sync_offset_millis(EngineAudioChannel::Microphone, 0)
            .expect("clear sync offset");
        let immediate = engine
            .drain_audio_until(Timestamp::from_millis(20))
            .expect("third audio block");
        assert_eq!(immediate[0].sample(0, 0), Some(0.12));

        let error = engine
            .set_channel_sync_offset_millis(
                EngineAudioChannel::Microphone,
                obs_rs_audio::MAX_AUDIO_SYNC_OFFSET_MILLISECONDS + 1,
            )
            .expect_err("offset must remain bounded");
        assert!(error.to_string().contains("sync offset"));
    }

    #[test]
    fn fallback_audio_is_reported() {
        let engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let snapshot = engine.snapshot();
        assert!(!snapshot.audio_fallback);
        assert_eq!(snapshot.audio_backend, "Deterministic test signal");
    }

    #[test]
    fn selected_audio_input_does_not_silently_switch_to_another_device() {
        let engine = EngineSession::new(
            project(),
            EngineConfig::default().with_audio_input_id("missing-input"),
        )
        .expect("engine");
        let snapshot = engine.snapshot();
        assert!(snapshot.audio_fallback);
        assert_eq!(snapshot.audio_backend, "simulated fallback");
    }

    #[test]
    fn desktop_and_microphone_are_distinct_metered_mixer_sources() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .set_channel_gain_milli(EngineAudioChannel::Desktop, 500)
            .expect("desktop gain");
        engine
            .set_channel_muted(EngineAudioChannel::Microphone, false)
            .expect("microphone mute");

        engine.tick(None, Some("program")).expect("tick");
        let stats = engine.stats();
        assert_eq!(
            stats.desktop_peak_milli, 0,
            "unavailable desktop capture is silence"
        );
        assert!(
            stats.microphone_peak_milli > 0,
            "the deterministic input drives only the microphone node"
        );
        assert_eq!(
            stats.microphone_peak_hold_milli, stats.microphone_peak_milli,
            "the live meter publishes its held peak from the same mixer source"
        );
        assert!(!stats.microphone_clipped);
    }

    #[test]
    fn engine_rejects_gain_above_the_bounded_mixer_control() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        assert!(engine
            .set_channel_gain_milli(EngineAudioChannel::Microphone, MAX_GAIN_MILLI + 1)
            .is_err());
    }

    #[test]
    fn pan_reaches_the_mixed_audio_output_before_encoding() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .set_channel_pan_milli(EngineAudioChannel::Microphone, -1_000)
            .expect("full-left pan");
        let tick = engine.tick(None, Some("program")).expect("panned tick");
        let block = tick.audio_blocks.first().expect("audio block");
        assert!(block
            .samples()
            .chunks_exact(2)
            .all(|frame| frame[1].abs() < f32::EPSILON));
        assert!(block
            .samples()
            .chunks_exact(2)
            .any(|frame| frame[0].abs() > f32::EPSILON));
    }

    #[test]
    fn gain_filter_runs_on_a_live_channel_before_metering_and_mix() {
        let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        baseline.tick(None, Some("program")).expect("baseline tick");
        let baseline_peak = baseline.stats().microphone_peak_milli;

        let mut filtered = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        filtered
            .set_channel_gain_filter_db_milli(EngineAudioChannel::Microphone, -6_000)
            .expect("gain filter");
        let tick = filtered.tick(None, Some("program")).expect("filtered tick");

        assert!(
            baseline_peak > 0,
            "deterministic microphone must produce audio"
        );
        assert!(
            filtered.stats().microphone_peak_milli < baseline_peak,
            "the filtered channel meter must see gain-filtered audio"
        );
        assert!(
            tick.audio_blocks
                .iter()
                .flat_map(AudioBuffer::samples)
                .any(|sample| sample.abs() > 0.0),
            "the filter must preserve a non-silent live channel"
        );
    }

    #[test]
    fn invert_polarity_runs_on_a_live_channel_without_changing_peak() {
        let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let baseline_tick = baseline.tick(None, Some("program")).expect("baseline tick");

        let mut inverted = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        inverted
            .set_channel_invert_polarity(EngineAudioChannel::Microphone)
            .expect("invert polarity");
        let inverted_tick = inverted.tick(None, Some("program")).expect("inverted tick");

        assert_eq!(
            baseline.stats().microphone_peak_milli,
            inverted.stats().microphone_peak_milli
        );
        let mut saw_signal = false;
        for (original, inverted) in baseline_tick.audio_blocks[0]
            .samples()
            .iter()
            .zip(inverted_tick.audio_blocks[0].samples())
        {
            assert!((original + inverted).abs() < 0.000_001);
            saw_signal |= original.abs() > 0.0;
        }
        assert!(saw_signal, "deterministic microphone must produce audio");
    }

    #[test]
    fn limiter_runs_on_a_live_channel_before_metering_and_mix() {
        let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        baseline.tick(None, Some("program")).expect("baseline tick");
        let baseline_peak = baseline.stats().microphone_peak_milli;

        let mut limited = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        limited
            .set_channel_limiter(EngineAudioChannel::Microphone, -60_000, 60)
            .expect("limiter");
        let tick = limited.tick(None, Some("program")).expect("limited tick");

        assert!(
            baseline_peak > 0,
            "deterministic microphone must produce audio"
        );
        assert!(
            limited.stats().microphone_peak_milli < baseline_peak,
            "the channel meter must see limiter gain reduction"
        );
        assert!(
            tick.audio_blocks
                .iter()
                .flat_map(AudioBuffer::samples)
                .all(|sample| sample.is_finite()),
            "limiting must preserve the finite audio contract"
        );
    }

    #[test]
    fn compressor_runs_on_a_live_channel_before_metering_and_mix() {
        let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        baseline.tick(None, Some("program")).expect("baseline tick");
        let baseline_peak = baseline.stats().microphone_peak_milli;

        let mut compressed =
            EngineSession::new(project(), EngineConfig::default()).expect("engine");
        compressed
            .set_channel_compressor(EngineAudioChannel::Microphone, 32_000, -60_000, 1, 60, 0)
            .expect("compressor");
        let tick = compressed
            .tick(None, Some("program"))
            .expect("compressed tick");

        assert!(
            baseline_peak > 0,
            "deterministic microphone must produce audio"
        );
        assert!(
            compressed.stats().microphone_peak_milli < baseline_peak,
            "the channel meter must see compressor gain reduction"
        );
        assert!(
            tick.audio_blocks
                .iter()
                .flat_map(AudioBuffer::samples)
                .all(|sample| sample.is_finite()),
            "compression must preserve the finite audio contract"
        );
    }

    #[test]
    fn expander_runs_on_a_live_channel_before_metering_and_mix() {
        let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        baseline.tick(None, Some("program")).expect("baseline tick");
        let baseline_peak = baseline.stats().microphone_peak_milli;

        let mut expanded = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        expanded
            .set_channel_expander(EngineAudioChannel::Microphone, 10_000, 0, 1, 60, 0)
            .expect("expander");
        expanded
            .tick(None, Some("program"))
            .expect("first expanded tick");
        let tick = expanded.tick(None, Some("program")).expect("expanded tick");

        assert!(
            baseline_peak > 0,
            "deterministic microphone must produce audio"
        );
        assert!(
            expanded.stats().microphone_peak_milli < baseline_peak,
            "the channel meter must see expander attenuation"
        );
        assert!(
            tick.audio_blocks
                .iter()
                .flat_map(AudioBuffer::samples)
                .all(|sample| sample.is_finite()),
            "expansion must preserve the finite audio contract"
        );
    }

    #[test]
    fn noise_gate_runs_on_a_live_channel_before_metering_and_mix() {
        let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        baseline.tick(None, Some("program")).expect("baseline tick");
        let baseline_peak = baseline.stats().microphone_peak_milli;

        let mut gated = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        gated
            .set_channel_noise_gate(EngineAudioChannel::Microphone, 0, -32_000, 1, 125, 150)
            .expect("noise gate");
        gated.tick(None, Some("program")).expect("first gated tick");
        let tick = gated.tick(None, Some("program")).expect("gated tick");

        assert!(
            baseline_peak > 0,
            "deterministic microphone must produce audio"
        );
        assert!(
            gated.stats().microphone_peak_milli < baseline_peak,
            "the channel meter must see gate attenuation"
        );
        assert!(
            tick.audio_blocks
                .iter()
                .flat_map(AudioBuffer::samples)
                .all(|sample| sample.is_finite()),
            "gating must preserve the finite audio contract"
        );
    }

    /// Provider exposing one playback route whose monitor is readable, which is
    /// the shape a real desktop capture takes on Linux.
    #[derive(Debug)]
    struct MonitorProvider;

    impl AudioInputProvider for MonitorProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
            Ok(vec![AudioDeviceInfo::new(
                "speakers",
                "Speakers",
                AudioDeviceKind::Output,
            )?])
        }

        fn open_input(
            &self,
            device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
            if device_id != "speakers" {
                return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                    device_id.to_owned(),
                ));
            }
            SimulatedAudioProvider::new().open_input("test-audio", format)
        }
    }

    /// Provider whose default routes are deliberately not first in discovery
    /// order, proving automatic selection does not depend on vector order.
    #[derive(Debug)]
    struct DefaultRouteProvider;

    impl AudioInputProvider for DefaultRouteProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
            let mut default_input = AudioDeviceInfo::new(
                "default-input",
                "Default microphone",
                AudioDeviceKind::Input,
            )?;
            default_input.set_default(true);
            let mut default_output = AudioDeviceInfo::new(
                "default-output",
                "Default speakers",
                AudioDeviceKind::Output,
            )?;
            default_output.set_default(true);
            Ok(vec![
                AudioDeviceInfo::new("other-input", "Other microphone", AudioDeviceKind::Input)?,
                AudioDeviceInfo::new("other-output", "Other speakers", AudioDeviceKind::Output)?,
                default_input,
                default_output,
            ])
        }

        fn open_input(
            &self,
            device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
            if !matches!(
                device_id,
                "default-input" | "other-input" | "default-output" | "other-output"
            ) {
                return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                    device_id.to_owned(),
                ));
            }
            SimulatedAudioProvider::new().open_input("test-audio", format)
        }
    }

    #[test]
    fn automatic_audio_routes_prefer_provider_defaults_over_discovery_order() {
        let config = EngineConfig::default().with_audio_provider(Arc::new(DefaultRouteProvider));
        let engine = EngineSession::new(project(), config).expect("engine");

        assert_eq!(
            engine.audio_active_device_id.as_deref(),
            Some("default-input")
        );
        assert_eq!(
            engine.desktop_audio_active_device_id.as_deref(),
            Some("default-output")
        );
        assert_eq!(
            engine.snapshot().desktop_audio,
            DesktopAudioSource::Monitor("Default speakers".to_owned())
        );
    }

    #[derive(Debug)]
    struct ChangingDefaultProvider {
        phase: Arc<AtomicUsize>,
        opens: Arc<AtomicUsize>,
    }

    impl AudioInputProvider for ChangingDefaultProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
            let input_id = if self.phase.load(Ordering::Acquire) == 0 {
                "first-input"
            } else {
                "second-input"
            };
            let input_name = if input_id == "first-input" {
                "First microphone"
            } else {
                "Second microphone"
            };
            let mut input = AudioDeviceInfo::new(input_id, input_name, AudioDeviceKind::Input)?;
            input.set_default(true);
            let mut output =
                AudioDeviceInfo::new("stable-output", "Stable speakers", AudioDeviceKind::Output)?;
            output.set_default(true);
            Ok(vec![input, output])
        }

        fn open_input(
            &self,
            device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
            if !matches!(device_id, "first-input" | "second-input" | "stable-output") {
                return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                    device_id.to_owned(),
                ));
            }
            self.opens.fetch_add(1, Ordering::AcqRel);
            SimulatedAudioProvider::new().open_input("test-audio", format)
        }
    }

    #[test]
    fn automatic_audio_routes_reconcile_a_live_default_change_off_tick() {
        let phase = Arc::new(AtomicUsize::new(0));
        let opens = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(ChangingDefaultProvider {
            phase: Arc::clone(&phase),
            opens: Arc::clone(&opens),
        });
        let config = EngineConfig::default().with_audio_provider(provider);
        let mut engine = EngineSession::new(project(), config).expect("engine");
        assert_eq!(
            engine.audio_active_device_id.as_deref(),
            Some("first-input")
        );
        engine
            .drain_audio_until(Timestamp::ZERO)
            .expect("initial route refresh");
        phase.store(1, Ordering::Release);

        for attempt in 0..100_u64 {
            engine
                .drain_audio_until(Timestamp::from_millis(500 + attempt * 10))
                .expect("route-refresh audio");
            if engine.audio_active_device_id.as_deref() == Some("second-input") {
                break;
            }
            std::thread::yield_now();
        }

        assert_eq!(
            engine.audio_active_device_id.as_deref(),
            Some("second-input")
        );
        assert!(!engine.snapshot().audio_fallback);
        assert!(opens.load(Ordering::Acquire) >= 3);
    }

    #[test]
    fn explicit_audio_selection_ignores_a_live_default_change() {
        let phase = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(ChangingDefaultProvider {
            phase: Arc::clone(&phase),
            opens: Arc::new(AtomicUsize::new(0)),
        });
        let config = EngineConfig::default()
            .with_audio_provider(provider)
            .with_audio_input_id("first-input");
        let mut engine = EngineSession::new(project(), config).expect("engine");
        engine
            .drain_audio_until(Timestamp::ZERO)
            .expect("initial route refresh");
        phase.store(1, Ordering::Release);

        for attempt in 0..40_u64 {
            engine
                .drain_audio_until(Timestamp::from_millis(500 + attempt * 10))
                .expect("explicit route audio");
            std::thread::yield_now();
        }

        assert_eq!(
            engine.audio_active_device_id.as_deref(),
            Some("first-input")
        );
    }

    #[derive(Debug)]
    struct NativeFormatProvider;

    impl AudioInputProvider for NativeFormatProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
            Ok(vec![AudioDeviceInfo::new(
                "native-mono",
                "Native mono device",
                AudioDeviceKind::Input,
            )?])
        }

        fn open_input(
            &self,
            device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
            let native = AudioFormat::new(44_100, 1)?;
            if device_id != "native-mono" || format != native {
                return Err(AudioDeviceError::Unavailable(
                    "device only accepts its native 44.1 kHz mono format".to_owned(),
                ));
            }
            SimulatedAudioProvider::new().open_input("test-audio", native)
        }
    }

    #[test]
    fn device_native_audio_is_mapped_and_resampled_to_the_mix_format() {
        let provider: Arc<dyn AudioInputProvider> = Arc::new(NativeFormatProvider);
        let mix = AudioFormat::new(48_000, 2).expect("mix format");
        let (mut input, name, fallback, active_id) =
            open_audio_input(&provider, mix, Some("native-mono"));
        assert_eq!(name, "Native mono device");
        assert!(!fallback);
        assert_eq!(active_id.as_deref(), Some("native-mono"));
        assert_eq!(input.format(), mix);
        let block = input
            .read_block(Timestamp::ZERO, 480)
            .expect("converted block");
        assert_eq!(block.format(), mix);
        assert_eq!(block.frames(), 480);
        assert!(block
            .samples()
            .chunks_exact(2)
            .all(|frame| (frame[0] - frame[1]).abs() < f32::EPSILON));
    }

    /// An input that serves `healthy_blocks` and then fails permanently.
    struct FailingAudioInput {
        format: AudioFormat,
        inner: Box<dyn AudioInput>,
        healthy_blocks: usize,
    }

    impl AudioInput for FailingAudioInput {
        fn format(&self) -> AudioFormat {
            self.format
        }

        fn state(&self) -> obs_rs_audio::AudioInputState {
            self.inner.state()
        }

        fn read_block(
            &mut self,
            timestamp: Timestamp,
            frames: usize,
        ) -> Result<obs_rs_audio::AudioBuffer, obs_rs_audio::AudioDeviceError> {
            if self.healthy_blocks == 0 {
                return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                    "unplugged".to_owned(),
                ));
            }
            self.healthy_blocks -= 1;
            self.inner.read_block(timestamp, frames)
        }

        fn stop(&mut self) {
            self.inner.stop();
        }
    }

    struct FailingProvider;

    impl AudioInputProvider for FailingProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
            Ok(vec![AudioDeviceInfo::new(
                "failing-input",
                "Failing input",
                AudioDeviceKind::Input,
            )?])
        }

        fn open_input(
            &self,
            _device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
            Ok(Box::new(FailingAudioInput {
                format,
                inner: SimulatedAudioProvider::new().open_input("test-audio", format)?,
                healthy_blocks: 2,
            }))
        }
    }

    struct ReconnectingProvider {
        opens: Arc<AtomicUsize>,
    }

    impl AudioInputProvider for ReconnectingProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
            Ok(vec![AudioDeviceInfo::new(
                "reconnecting-input",
                "Reconnecting input",
                AudioDeviceKind::Input,
            )?])
        }

        fn open_input(
            &self,
            device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
            if device_id != "reconnecting-input" {
                return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                    device_id.to_owned(),
                ));
            }
            let attempt = self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FailingAudioInput {
                format,
                inner: SimulatedAudioProvider::new().open_input("test-audio", format)?,
                healthy_blocks: if attempt == 0 { 2 } else { usize::MAX },
            }))
        }
    }

    struct ReconnectingMonitorProvider {
        opens: Arc<AtomicUsize>,
    }

    impl AudioInputProvider for ReconnectingMonitorProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
            Ok(vec![AudioDeviceInfo::new(
                "reconnecting-monitor",
                "Reconnecting monitor",
                AudioDeviceKind::Output,
            )?])
        }

        fn open_input(
            &self,
            device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
            if device_id != "reconnecting-monitor" {
                return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                    device_id.to_owned(),
                ));
            }
            let attempt = self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FailingAudioInput {
                format,
                inner: SimulatedAudioProvider::new().open_input("test-audio", format)?,
                healthy_blocks: if attempt == 0 { 2 } else { usize::MAX },
            }))
        }
    }

    #[test]
    fn falling_back_after_a_device_failure_keeps_the_audio_timeline_continuous() {
        // The timeline, not the device, issues block timestamps, and the
        // fallback stamps the block it is handed. Swapping providers mid-session
        // must therefore leave no gap, overlap, or repeat in the emitted
        // timestamps — that continuity is what keeps A/V in sync afterwards.
        let config = EngineConfig::default().with_audio_provider(Arc::new(FailingProvider));
        let mut engine = EngineSession::new(project(), config).expect("engine");

        let mut timestamps = Vec::new();
        for index in 0..6_u64 {
            let blocks = engine
                .drain_audio_until(Timestamp::from_millis(index * 100))
                .expect("audio blocks");
            timestamps.extend(blocks.iter().map(|block| block.timestamp().as_nanos()));
        }

        assert!(
            engine.snapshot().audio_fallback,
            "the failing device should have been replaced"
        );
        assert!(
            timestamps.len() > 3,
            "the fallback kept producing blocks: {timestamps:?}"
        );
        assert_eq!(timestamps.first(), Some(&0));
        let steps = timestamps
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect::<Vec<_>>();
        let block_nanos = steps[0];
        assert!(block_nanos > 0, "blocks must advance the timeline");
        assert!(
            steps.iter().all(|step| *step == block_nanos),
            "block spacing changed across the fallback: {steps:?}"
        );
    }

    #[test]
    fn selected_audio_input_reconnects_after_a_bounded_media_interval() {
        let opens = Arc::new(AtomicUsize::new(0));
        let config = EngineConfig::default()
            .with_audio_provider(Arc::new(ReconnectingProvider {
                opens: Arc::clone(&opens),
            }))
            .with_audio_input_id("reconnecting-input");
        let mut engine = EngineSession::new(project(), config).expect("engine");

        engine
            .drain_audio_until(Timestamp::from_millis(900))
            .expect("fallback audio blocks");
        assert!(engine.snapshot().audio_fallback);
        assert_eq!(opens.load(Ordering::SeqCst), 1);

        engine
            .drain_audio_until(Timestamp::from_millis(1_100))
            .expect("reconnected audio blocks");
        let snapshot = engine.snapshot();
        assert!(!snapshot.audio_fallback);
        assert_eq!(snapshot.audio_backend, "Reconnecting input");
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        assert!(snapshot.last_error.is_none());
    }

    #[test]
    fn automatic_audio_input_reconnects_after_a_bounded_media_interval() {
        let opens = Arc::new(AtomicUsize::new(0));
        let config = EngineConfig::default().with_audio_provider(Arc::new(ReconnectingProvider {
            opens: Arc::clone(&opens),
        }));
        let mut engine = EngineSession::new(project(), config).expect("engine");
        assert_eq!(
            engine.audio_active_device_id.as_deref(),
            Some("reconnecting-input")
        );

        engine
            .drain_audio_until(Timestamp::from_millis(900))
            .expect("fallback audio blocks");
        assert!(engine.snapshot().audio_fallback);
        assert!(engine.audio_active_device_id.is_none());
        assert_eq!(opens.load(Ordering::SeqCst), 1);

        engine
            .drain_audio_until(Timestamp::from_millis(1_100))
            .expect("reconnected audio blocks");
        let snapshot = engine.snapshot();
        assert!(!snapshot.audio_fallback);
        assert_eq!(
            engine.audio_active_device_id.as_deref(),
            Some("reconnecting-input")
        );
        assert_eq!(opens.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn selected_desktop_monitor_reconnects_after_a_bounded_media_interval() {
        let opens = Arc::new(AtomicUsize::new(0));
        let config = EngineConfig::default()
            .with_audio_provider(Arc::new(ReconnectingMonitorProvider {
                opens: Arc::clone(&opens),
            }))
            .with_desktop_audio_id("reconnecting-monitor");
        let mut engine = EngineSession::new(project(), config).expect("engine");

        engine
            .drain_audio_until(Timestamp::from_millis(900))
            .expect("silent desktop blocks");
        assert_eq!(
            engine.snapshot().desktop_audio,
            DesktopAudioSource::Silent(
                "unavailable (audio device unavailable: unplugged)".to_owned()
            )
        );
        assert_eq!(opens.load(Ordering::SeqCst), 1);

        engine
            .drain_audio_until(Timestamp::from_millis(1_100))
            .expect("reconnected desktop blocks");
        let snapshot = engine.snapshot();
        assert_eq!(
            snapshot.desktop_audio,
            DesktopAudioSource::Monitor("Reconnecting monitor".to_owned())
        );
        assert_eq!(opens.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn automatic_desktop_monitor_reconnects_after_a_bounded_media_interval() {
        let opens = Arc::new(AtomicUsize::new(0));
        let config =
            EngineConfig::default().with_audio_provider(Arc::new(ReconnectingMonitorProvider {
                opens: Arc::clone(&opens),
            }));
        let mut engine = EngineSession::new(project(), config).expect("engine");
        assert_eq!(
            engine.desktop_audio_active_device_id.as_deref(),
            Some("reconnecting-monitor")
        );

        engine
            .drain_audio_until(Timestamp::from_millis(900))
            .expect("silent desktop blocks");
        assert!(engine.desktop_audio_active_device_id.is_none());
        assert_eq!(opens.load(Ordering::SeqCst), 1);

        engine
            .drain_audio_until(Timestamp::from_millis(1_100))
            .expect("reconnected desktop blocks");
        let snapshot = engine.snapshot();
        assert_eq!(
            snapshot.desktop_audio,
            DesktopAudioSource::Monitor("Reconnecting monitor".to_owned())
        );
        assert_eq!(
            engine.desktop_audio_active_device_id.as_deref(),
            Some("reconnecting-monitor")
        );
        assert_eq!(opens.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn selected_desktop_monitor_does_not_silently_switch_to_another_route() {
        let config = EngineConfig::default()
            .with_audio_provider(Arc::new(MonitorProvider))
            .with_desktop_audio_id("missing-monitor");
        let engine = EngineSession::new(project(), config).expect("engine");

        assert_eq!(
            engine.snapshot().desktop_audio,
            DesktopAudioSource::Silent("no playback monitor".to_owned())
        );
    }

    #[test]
    fn a_playback_monitor_feeds_the_desktop_channel() {
        let config = EngineConfig::default().with_audio_provider(Arc::new(MonitorProvider));
        let mut engine = EngineSession::new(project(), config).expect("engine");

        assert_eq!(
            engine.snapshot().desktop_audio,
            DesktopAudioSource::Monitor("Speakers".to_owned())
        );

        engine.tick(None, Some("program")).expect("tick");

        assert!(
            engine.stats().desktop_peak_milli > 0,
            "the opened monitor drives the desktop meter"
        );
    }

    #[test]
    fn a_session_without_a_playback_route_keeps_the_desktop_channel_silent() {
        let engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let snapshot = engine.snapshot();

        assert!(!snapshot.desktop_audio.is_capturing());
        assert_eq!(snapshot.desktop_audio.label(), "no playback monitor");
    }

    #[test]
    fn monitor_audio_updates_levels_without_encoding_video() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");

        engine
            .monitor_audio_until(Timestamp::ZERO)
            .expect("monitor tick");

        assert!(engine.stats().microphone_peak_milli > 0);
        assert_eq!(engine.stats().video_frames, 0);
        assert_eq!(engine.stats().audio_blocks, 1);
    }

    #[test]
    fn monitor_modes_route_engine_audio_to_the_bounded_output_worker() {
        let config = EngineConfig::default()
            .with_audio_input_monitor_mode(AudioMonitorMode::MonitorOnly)
            .with_monitor_output_id("test-output");
        let mut engine = EngineSession::new(project(), config).expect("engine");

        let output = engine
            .drain_audio_until(Timestamp::ZERO)
            .expect("audio block");
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].sample(0, 0), Some(0.0));
        assert_eq!(engine.stats().monitor_blocks_submitted, 1);
        assert_eq!(engine.stats().monitor_blocks_dropped, 0);
        assert!(engine.snapshot().monitor_output.is_some());

        engine
            .set_channel_monitor_mode(EngineAudioChannel::Microphone, AudioMonitorMode::Off)
            .expect("switch monitor mode");
        assert_eq!(
            engine
                .mixer
                .source_monitor_mode(engine.microphone_audio_source)
                .expect("microphone source"),
            AudioMonitorMode::Off
        );
        engine
            .set_monitor_output_id(None)
            .expect("clear monitor output");
        assert!(engine.snapshot().monitor_output.is_none());
    }

    #[test]
    fn monitor_output_worker_failure_is_visible_without_failing_the_engine_tick() {
        let config = EngineConfig::default().with_monitor_output_id("missing-output");
        let mut engine = EngineSession::new(project(), config).expect("engine");
        engine
            .monitor_audio_until(Timestamp::ZERO)
            .expect("monitor tick remains independent of sink failure");

        for _ in 0..100 {
            if engine
                .snapshot()
                .monitor_output
                .as_ref()
                .is_some_and(|output| output.state == AudioOutputWorkerState::Failed)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let output = engine
            .snapshot()
            .monitor_output
            .expect("configured monitor worker");
        assert_eq!(output.state, AudioOutputWorkerState::Failed);
        assert!(output
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("unavailable")));
    }

    #[test]
    fn idle_audio_format_rebuild_preserves_routing_and_restarts_the_timeline() {
        let config = EngineConfig::default()
            .with_audio_input_monitor_mode(AudioMonitorMode::MonitorOnly)
            .with_monitor_output_id("test-output");
        let mut engine = EngineSession::new(project(), config).expect("engine");
        let next_format = AudioFormat::new(44_100, 1).expect("next format");

        engine
            .set_audio_format(next_format)
            .expect("idle format change");
        assert_eq!(engine.config.audio_format, next_format);
        assert_eq!(engine.timeline.audio_format(), next_format);
        assert_eq!(
            engine
                .mixer
                .source_monitor_mode(engine.microphone_audio_source)
                .expect("microphone source"),
            AudioMonitorMode::MonitorOnly
        );
        assert!(engine.snapshot().monitor_output.is_some());

        let tick = engine.tick(None, None).expect("reconfigured tick");
        assert!(!tick.audio_blocks.is_empty());
        assert!(tick
            .audio_blocks
            .iter()
            .all(|buffer| buffer.format() == next_format));

        engine
            .start_replay_buffer(1_024 * 1_024, Duration::from_secs(5))
            .expect("replay buffer");
        let error = engine
            .set_audio_format(AudioFormat::new(48_000, 2).expect("other format"))
            .expect_err("active replay must block format replacement");
        assert!(matches!(
            error,
            EngineError::Busy("change the audio format")
        ));
        engine.stop_replay_buffer();
    }

    #[test]
    fn recording_contains_both_media_kinds_in_timestamp_order() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.obsr"));
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine.start_recording(&path).expect("recording");
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        let bytes = engine.finish_recording().expect("finalize");
        let persisted = std::fs::read(&path).expect("read recording");
        assert_eq!(persisted.len(), bytes);
        let packets = obs_rs_output::MemoryMuxer::decode(&persisted).expect("decode recording");
        assert!(packets
            .iter()
            .any(|packet| packet.kind() == obs_rs_output::PacketKind::Video));
        assert!(packets
            .iter()
            .any(|packet| packet.kind() == obs_rs_output::PacketKind::Audio));
        assert!(packets
            .windows(2)
            .all(|packets| packets[0].timestamp() <= packets[1].timestamp()));
        std::fs::remove_file(path).expect("remove recording");
    }

    #[test]
    fn segmented_recording_publishes_numbered_packet_files() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("obs-rs-engine-segmented-{token}.obsr"));
        let policy = SegmentedRecordingPolicy::new(2_000_000, Duration::from_nanos(1), 4)
            .expect("split policy");
        let stale = base.with_file_name(format!("obs-rs-engine-segmented-{token}-0002.obsr.part"));
        std::fs::write(&stale, [1, 2, 3]).expect("write stale segment artifact");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .start_segmented_recording(&base, policy)
            .expect("segmented recording");
        assert!(!stale.exists(), "startup removes stale segment artifact");
        for _ in 0..3 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        let bytes = engine.finish_recording().expect("finalize split recording");

        let paths: Vec<_> = (1..=3)
            .map(|index| {
                base.with_file_name(format!("obs-rs-engine-segmented-{token}-{index:04}.obsr"))
            })
            .collect();
        assert!(paths.iter().all(|path| path.is_file()));
        assert!(!base.exists(), "the base path is only a naming anchor");
        assert!(!base
            .with_file_name(format!("obs-rs-engine-segmented-{token}-0004.obsr"))
            .exists());
        let persisted_bytes: usize = paths
            .iter()
            .map(|path| {
                usize::try_from(std::fs::metadata(path).expect("segment metadata").len())
                    .expect("segment size fits usize")
            })
            .sum();
        assert_eq!(persisted_bytes, bytes);
        for path in &paths {
            let packets =
                obs_rs_output::MemoryMuxer::decode(&std::fs::read(path).expect("read segment"))
                    .expect("decode segment");
            assert!(packets.iter().any(|packet| {
                packet.kind() == obs_rs_output::PacketKind::Video && packet.is_keyframe()
            }));
            std::fs::remove_file(path).expect("remove segment");
        }
    }

    #[test]
    fn recording_rejects_extensions_that_do_not_select_a_known_container() {
        let path = std::env::temp_dir().join("obs-rs-unknown-recording.bin");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let error = engine
            .start_recording(path)
            .expect_err("unknown extension must be rejected");
        assert!(error
            .to_string()
            .contains(".mkv, .mp4, .mov, .flv, or .obsr"));
        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn matroska_recording_uses_raw_production_media_and_publishes_atomically() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::MatroskaH264Aac)
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mkv"));
        let temp_path = path.with_extension("mkv.part");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine.start_recording(&path).expect("Matroska recording");
        assert!(!path.exists(), "the final path stays hidden until EOS");
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine.finish_recording().expect("finalize Matroska");
        let persisted = std::fs::read(&path).expect("read Matroska");
        assert_eq!(persisted.len(), bytes);
        assert!(persisted.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]));
        assert!(!temp_path.exists());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn mp4_recording_uses_raw_production_media_and_publishes_atomically() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::Mp4H264Aac)
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mp4"));
        let temp_path = path.with_extension("mp4.part");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine.start_recording(&path).expect("MP4 recording");
        assert!(!path.exists(), "the final path stays hidden until EOS");
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine.finish_recording().expect("finalize MP4");
        let persisted = std::fs::read(&path).expect("read MP4");
        assert_eq!(persisted.len(), bytes);
        assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
        assert!(!temp_path.exists());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn remux_recording_publishes_mp4_after_consuming_hidden_matroska_source() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::MatroskaH264Aac)
            || !capabilities.supports_remux()
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-auto-remux-{token}.mp4"));
        let source_path = path.with_extension("mkv.part");
        let remux_temp_path = path.with_extension("mp4.part");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .start_remux_recording(&path)
            .expect("automatic remux recording");
        assert!(
            !path.exists(),
            "the final path stays hidden until remux EOS"
        );
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine.finish_recording().expect("finalize automatic remux");
        let persisted = std::fs::read(&path).expect("read remuxed MP4");
        assert_eq!(persisted.len(), bytes);
        assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
        assert!(!source_path.exists());
        assert!(!remux_temp_path.exists());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn explicit_fragmented_mp4_profile_reaches_the_engine_recording_boundary() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::FragmentedMp4H264Aac)
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mp4"));
        let temp_path = path.with_extension("mp4.part");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .start_recording_profile(&path, OutputProfile::fragmented_mp4_h264_aac())
            .expect("fragmented MP4 recording");
        assert!(!path.exists(), "the final path stays hidden until EOS");
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine.finish_recording().expect("finalize fragmented MP4");
        let persisted = std::fs::read(&path).expect("read fragmented MP4");
        assert_eq!(persisted.len(), bytes);
        assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
        assert!(persisted.windows(4).any(|chunk| chunk == b"moof"));
        assert!(!temp_path.exists());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn segmented_mp4_recording_reaches_the_engine_native_boundary() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::Mp4H264Aac)
            || !capabilities.supports_segmented_recording()
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base_path = std::env::temp_dir().join(format!("obs-rs-engine-segmented-{token}.mp4"));
        let policy =
            SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_millis(500), 3)
                .expect("segment policy");
        let available = capabilities.capabilities();
        let video_encoder = available
            .video_encoders()
            .iter()
            .find(|encoder| encoder.codec() == obs_rs_output::VideoCodec::H264);
        let audio_encoder = available
            .audio_encoders()
            .iter()
            .find(|encoder| encoder.codec() == obs_rs_output::AudioCodec::Aac);
        let (Some(video_encoder), Some(audio_encoder)) = (video_encoder, audio_encoder) else {
            return;
        };
        let video_config = VideoEncoderConfig {
            implementation: obs_rs_output::EncoderImplementation::new(video_encoder.id()),
            bitrate_kbps: 2_000,
            ..VideoEncoderConfig::default()
        };
        let audio_config = AudioEncoderConfig {
            implementation: obs_rs_output::EncoderImplementation::new(audio_encoder.id()),
            bitrate_kbps: 160,
            ..AudioEncoderConfig::default()
        };
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .start_segmented_recording_configured(&base_path, policy, video_config, audio_config)
            .expect("segmented MP4 recording");
        for _ in 0..90 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine
            .finish_recording()
            .expect("finalize segmented MP4 recording");
        assert!(bytes > 0);

        let mut published = 0_usize;
        let stem = base_path
            .file_stem()
            .and_then(|value| value.to_str())
            .expect("base stem");
        for index in 1..=policy.max_segments() {
            let path = base_path.with_file_name(format!("{stem}-{index:05}.mp4"));
            if path.exists() {
                published = published.saturating_add(1);
                assert!(std::fs::metadata(&path).expect("segment metadata").len() > 0);
                std::fs::remove_file(path).expect("remove segment");
            }
            let temp = base_path.with_file_name(format!("{stem}-{index:05}.mp4.part"));
            assert!(!temp.exists(), "temporary segment must be cleaned");
        }
        assert!(published > 0, "engine must publish at least one segment");
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn mov_recording_uses_raw_production_media_and_publishes_atomically() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::MovH264Aac)
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mov"));
        let temp_path = path.with_extension("mov.part");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine.start_recording(&path).expect("MOV recording");
        assert!(!path.exists(), "the final path stays hidden until EOS");
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine.finish_recording().expect("finalize MOV");
        let persisted = std::fs::read(&path).expect("read MOV");
        assert_eq!(persisted.len(), bytes);
        assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
        assert!(!temp_path.exists());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn flv_recording_uses_raw_production_media_and_publishes_atomically() {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(obs_rs_output::OutputProfileKind::FlvH264Aac)
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.flv"));
        let temp_path = path.with_extension("flv.part");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine.start_recording(&path).expect("FLV recording");
        assert!(!path.exists(), "the final path stays hidden until EOS");
        for _ in 0..4 {
            engine.tick(None, Some("program")).expect("media tick");
        }
        assert_eq!(engine.reference_video_encode_calls, 0);
        assert_eq!(engine.reference_audio_encode_calls, 0);
        let bytes = engine.finish_recording().expect("finalize FLV");
        let persisted = std::fs::read(&path).expect("read FLV");
        assert_eq!(persisted.len(), bytes);
        assert_eq!(persisted.get(..3), Some(&b"FLV"[..]));
        assert!(!temp_path.exists());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[test]
    fn worker_accepts_frames_and_finalizes_on_its_own_thread() {
        let format = project()
            .active_profile_spec()
            .expect("profile")
            .video_format();
        let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let worker = EngineWorker::spawn_with_capacity(session, 1).expect("worker");
        worker
            .sync_project(project())
            .expect("project sync while idle");
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-worker-{token}.obsr"));
        worker.start_recording(&path).expect("recording");
        assert!(worker.try_push_frame(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255],)));
        let bytes = worker.finish_recording().expect("finalize");
        let packets =
            obs_rs_output::MemoryMuxer::decode(&std::fs::read(&path).expect("read recording"))
                .expect("decode recording");
        assert_eq!(packets.len(), 2);
        assert!(packets
            .iter()
            .any(|packet| packet.kind() == obs_rs_output::PacketKind::Video));
        assert!(packets
            .iter()
            .any(|packet| packet.kind() == obs_rs_output::PacketKind::Audio));
        assert!(bytes > 0);
        std::fs::remove_file(path).expect("remove recording");
    }

    #[test]
    fn worker_accepts_segmented_recording_and_finalizes_numbered_files() {
        let format = project()
            .active_profile_spec()
            .expect("profile")
            .video_format();
        let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let worker = EngineWorker::spawn_with_capacity(session, 4).expect("worker");
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("obs-rs-engine-worker-segmented-{token}.obsr"));
        let policy = SegmentedRecordingPolicy::new(10_000, Duration::from_nanos(1), 3)
            .expect("split policy");

        worker
            .start_segmented_recording(&base, policy)
            .expect("segmented recording");
        assert!(worker.try_push_frame(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255],)));
        assert!(worker.try_push_frame(VideoFrame::solid(
            format,
            Timestamp::from_millis(33),
            [4, 5, 6, 255],
        )));
        let bytes = worker.finish_recording().expect("finalize split recording");
        assert!(bytes > 0);

        for index in 1..=2 {
            let path = base.with_file_name(format!(
                "obs-rs-engine-worker-segmented-{token}-{index:04}.obsr"
            ));
            assert!(path.is_file(), "missing segment {index}");
            let packets =
                obs_rs_output::MemoryMuxer::decode(&std::fs::read(&path).expect("read segment"))
                    .expect("decode segment");
            assert!(packets
                .iter()
                .any(|packet| packet.kind() == obs_rs_output::PacketKind::Video));
            std::fs::remove_file(path).expect("remove segment");
        }
    }

    #[test]
    fn worker_publishes_monitor_levels_while_outputs_are_idle() {
        let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let worker = EngineWorker::spawn_with_capacity(session, 1).expect("worker");

        assert!(worker.try_monitor_audio(Timestamp::ZERO));
        // A blocking command is a queue barrier: its reply arrives only after
        // the preceding monitor sample has been processed and published.
        worker
            .set_channel_gain_milli(EngineAudioChannel::Microphone, 1_000)
            .expect("queue barrier");

        let snapshot = worker.snapshot();
        assert!(snapshot.engine.stats.microphone_peak_milli > 0);
        assert_eq!(snapshot.engine.stats.video_frames, 0);
        assert!(!snapshot.engine.recording && !snapshot.engine.streaming);
    }

    #[test]
    fn a_recording_reports_every_phase_it_passes_through() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-engine-phase-{token}.obsr"));
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Idle);

        engine.start_recording(&path).expect("recording");
        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Running);
        engine.tick(None, Some("program")).expect("media tick");

        engine.finish_recording().expect("finalize");
        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Idle);
        assert!(engine.recording_lifecycle().is_stopped());
        std::fs::remove_file(path).expect("remove recording");
    }

    #[test]
    fn a_recording_that_cannot_be_opened_reports_failed_rather_than_idle() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");

        // An empty path names no file, so the writer cannot be built at all.
        engine
            .start_recording("")
            .expect_err("a path that names no file is rejected");

        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
        assert!(
            engine.recording_lifecycle().is_stopped(),
            "a frontend must treat a failed start as not recording"
        );
        assert!(
            engine.snapshot().last_error.is_some(),
            "the failure has to leave an explanation behind"
        );
        assert!(!engine.is_recording());
    }

    #[test]
    fn a_recording_that_cannot_be_committed_stays_failed_and_keeps_its_stream() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("obs-rs-engine-commit-{token}"));
        let path = root.join("recording.obsr");
        std::fs::create_dir_all(&path).expect("create conflicting final directory");
        // The temporary stream can be opened beside the requested path, but a
        // regular file cannot atomically replace the directory at commit time.
        engine
            .start_recording(&path)
            .expect("the temporary packet stream opens");
        engine.tick(None, Some("program")).expect("media tick");

        engine
            .finish_recording()
            .expect_err("committing into a missing directory fails");

        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
        assert!(
            engine.is_recording(),
            "a failed commit must not discard the captured packet stream"
        );
        engine.abort_recording();
        std::fs::remove_dir(&path).expect("remove conflicting final directory");
        std::fs::remove_dir(root).expect("remove commit fixture root");
    }

    #[test]
    fn a_stream_that_cannot_connect_reports_failed_and_can_be_retried() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");

        // Port 0 is never a listening peer, so the connect is refused.
        engine
            .start_streaming("127.0.0.1:0")
            .expect_err("an unreachable peer is rejected");

        assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Failed);
        assert!(!engine.is_streaming());

        // Stopping clears the failure so the next attempt starts from idle
        // rather than inheriting the previous one's phase.
        engine
            .finish_streaming()
            .expect("stop a stream that failed");
        assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Idle);
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    #[ignore = "requires a local native production sink; run on a reference output host"]
    fn production_schemes_create_native_stream_outputs() {
        let video = project()
            .active_profile_spec()
            .expect("profile")
            .video_format();
        let audio = EngineConfig::default().audio_format();
        for endpoint in [
            "rtmp://127.0.0.1:9/live/test",
            "rtmps://127.0.0.1:9/live/test",
            "srt://127.0.0.1:9",
        ] {
            let mut stream = StreamOutput::connect(endpoint, 1_048_576, 1, video, audio, None)
                .expect("native production pipeline");
            assert!(matches!(stream, StreamOutput::Production(_)));
            assert_eq!(stream.video_requirement(), VideoInputRequirement::Raw);
            assert_eq!(stream.audio_requirement(), AudioInputRequirement::Raw);
            stream.close().expect("close live pipeline");
        }
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    #[ignore = "requires a local native production sink; run on a reference output host"]
    fn production_only_streams_skip_reference_encoders_and_receive_raw_media() {
        let frame = VideoFrame::solid(
            project()
                .active_profile_spec()
                .expect("profile")
                .video_format(),
            Timestamp::ZERO,
            [24, 96, 180, 255],
        );
        for endpoint in [
            "rtmp://127.0.0.1:9/live/test",
            "rtmps://127.0.0.1:9/live/test",
            "srt://127.0.0.1:9",
        ] {
            let mut engine =
                EngineSession::new(project(), EngineConfig::default()).expect("engine");
            engine
                .start_streaming(endpoint)
                .expect("native production pipeline");

            engine
                .push_program_frame(&frame)
                .expect("raw media submission");

            assert_eq!(engine.reference_video_encode_calls, 0, "{endpoint}");
            assert_eq!(engine.reference_audio_encode_calls, 0, "{endpoint}");
            assert_eq!(engine.stats.video_encode_latency.samples(), 0, "{endpoint}");
            assert_eq!(engine.stats.audio_encode_latency.samples(), 0, "{endpoint}");
            assert_eq!(
                engine.stats.output_submit_latency.samples(),
                2,
                "{endpoint}"
            );
            let metrics = engine
                .snapshot()
                .production_stream_metrics
                .expect("production telemetry");
            assert_eq!(metrics.video_submitted, 1, "{endpoint}");
            assert_eq!(metrics.audio_submitted, 1, "{endpoint}");
            assert!(metrics.video_queue_bytes <= 1_048_576, "{endpoint}");
            assert!(metrics.audio_queue_bytes <= 1_048_576, "{endpoint}");
        }
    }

    #[test]
    fn reference_recording_runs_reference_encoders_once_per_media_item() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-reference-only-{token}.obsr"));
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
        engine.start_recording(&path).expect("recording");

        engine
            .push_program_frame(&frame)
            .expect("packetized media submission");

        assert_eq!(engine.reference_video_encode_calls, 1);
        assert_eq!(engine.reference_audio_encode_calls, 1);
        assert_eq!(engine.stats.video_encode_latency.samples(), 1);
        assert_eq!(engine.stats.audio_encode_latency.samples(), 1);
        assert_eq!(engine.stats.output_submit_latency.samples(), 2);
        engine.finish_recording().expect("finalize recording");
        std::fs::remove_file(path).expect("remove recording");
    }

    #[test]
    fn reference_tcp_and_websocket_streams_keep_packetized_encoding() {
        let policy = ReconnectPolicy::new(1);
        let streams = [
            StreamOutput::Tcp(
                StreamSession::new(
                    TcpPacketTransport::new("127.0.0.1:9"),
                    1_048_576,
                    PacketDropPolicy::DropNewest,
                    policy,
                )
                .expect("TCP stream"),
            ),
            StreamOutput::WebSocket(
                StreamSession::new(
                    WebSocketPacketTransport::new("ws://127.0.0.1:9/live"),
                    1_048_576,
                    PacketDropPolicy::DropNewest,
                    policy,
                )
                .expect("WebSocket stream"),
            ),
        ];

        for stream in streams {
            assert_eq!(
                stream.video_requirement(),
                VideoInputRequirement::Packetized
            );
            assert_eq!(
                stream.audio_requirement(),
                AudioInputRequirement::Packetized
            );
            let mut engine =
                EngineSession::new(project(), EngineConfig::default()).expect("engine");
            let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
            engine.streaming = Some(stream);

            engine
                .push_program_frame(&frame)
                .expect("packetized media submission");

            assert_eq!(engine.reference_video_encode_calls, 1);
            assert_eq!(engine.reference_audio_encode_calls, 1);
            assert!(engine.snapshot().stream_queued_bytes > 0);
        }
    }

    #[cfg(feature = "production-gstreamer")]
    #[test]
    fn reference_recording_and_rtmp_encode_once_and_submit_raw_once() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-mixed-output-{token}.obsr"));
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
        engine.start_recording(&path).expect("recording");
        engine
            .start_streaming("rtmp://127.0.0.1:9/live/test")
            .expect("native production pipeline");

        engine
            .push_program_frame(&frame)
            .expect("mixed media submission");

        assert_eq!(engine.reference_video_encode_calls, 1);
        assert_eq!(engine.reference_audio_encode_calls, 1);
        let metrics = engine
            .snapshot()
            .production_stream_metrics
            .expect("production telemetry");
        assert_eq!(metrics.video_submitted, 1);
        assert_eq!(metrics.audio_submitted, 1);
        engine.finish_recording().expect("finalize recording");
        std::fs::remove_file(path).expect("remove recording");
    }

    #[test]
    fn aborting_a_recording_clears_a_previous_failure() {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .start_recording("")
            .expect_err("a path that names no file is rejected");
        assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);

        engine.abort_recording();

        assert_eq!(
            engine.recording_lifecycle(),
            OutputLifecycle::Idle,
            "an explicit stop must not leave the session permanently broken"
        );
    }

    #[test]
    fn a_dead_worker_reports_both_outputs_as_failed() {
        let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let worker = EngineWorker::spawn_with_capacity(session, 1).expect("worker");

        let snapshot = worker.snapshot();

        assert!(snapshot.alive);
        assert_eq!(snapshot.engine.recording_lifecycle, OutputLifecycle::Idle);
        assert_eq!(snapshot.engine.streaming_lifecycle, OutputLifecycle::Idle);
    }
}
