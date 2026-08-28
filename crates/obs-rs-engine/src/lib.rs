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
mod session_output;
mod session_runtime;
mod types;
mod worker;

use audio::{audio_peak_milli, audio_reconnect_deadline, open_audio_input, open_desktop_audio};
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

use std::sync::Arc;

use audio_routes::AudioRouteWorker;

#[cfg(test)]
use obs_rs_audio::AudioDeviceError;
#[cfg(test)]
use obs_rs_audio::AudioDeviceKind;
#[cfg(test)]
use obs_rs_audio::AudioInputProvider;
use obs_rs_audio::{
    AudioBuffer, AudioDelayLine, AudioFilterChain, AudioInput, AudioMixer, AudioOutputWorker,
    AudioOutputWorkerHandle, AudioSourceId, AvSyncMetrics, SimulatedAudioProvider,
};
#[cfg(test)]
use obs_rs_audio::{AudioFilter, AudioFormat};
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_clock::MediaTimeline;
use obs_rs_core::Runtime;
#[cfg(test)]
use obs_rs_media::{ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, LumaKey, RenderDelay};
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
#[cfg(all(test, feature = "production-gstreamer"))]
use obs_rs_output::{AudioEncoderConfig, OutputProfile, VideoEncoderConfig};
use obs_rs_output::{
    AudioInputRequirement, EncodedPacket, RawAudioEncoder, ReplayBuffer, RleVideoEncoder,
    StreamState, VideoEncoder, VideoInputRequirement,
};
#[cfg(test)]
use obs_rs_output::{
    PacketDropPolicy, ReconnectPolicy, SegmentedRecordingPolicy, StreamSession, TcpPacketTransport,
    WebSocketPacketTransport,
};
use obs_rs_output_gstreamer::GStreamerCapabilitySnapshot;
#[cfg(feature = "production-gstreamer")]
pub use obs_rs_output_gstreamer::{
    stinger_decode_capabilities, write_interrupted_remux_manifest, GStreamerStingerLoader,
    RemuxRecovery, StingerDecodeCapabilities,
};
pub use obs_rs_output_gstreamer::{
    AudioEncoderCapability, OutputCapabilitiesSnapshot, ProductionProtocol, ProtocolCapability,
    VideoEncoderCapability,
};
#[cfg(not(feature = "production-gstreamer"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemuxRecovery {
    /// No native production-output runtime is provisioned in this build.
    NoCandidate,
    /// Kept shape-compatible with the provisioned native runtime.
    Recovered { bytes: usize },
}
#[cfg(not(feature = "production-gstreamer"))]
/// Reports that interrupted-remux recovery is unavailable in the portable
/// build.
/// # Errors
///
/// Always returns an explicit error because the optional native production
/// output runtime is not included in the portable build.
pub fn write_interrupted_remux_manifest(
    _path: impl AsRef<std::path::Path>,
) -> Result<(), EngineError> {
    Err(EngineError::InvalidConfiguration(
        "automatic remux requires the optional production-gstreamer feature".to_owned(),
    ))
}
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

    #[path = "engine_audio_tests.rs"]
    mod audio_tests;
    #[path = "engine_output_tests.rs"]
    mod output_tests;
    #[path = "engine_project_tests.rs"]
    mod project_tests;
}
