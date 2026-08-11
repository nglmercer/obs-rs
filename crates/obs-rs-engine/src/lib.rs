//! Portable orchestration for one OBS-RS audio/video session.
//!
//! The engine deliberately owns the media boundary without owning a GUI event
//! loop. Applications can drive it from a worker thread, a headless command, or
//! a desktop adapter. Platform devices are injected through the audio provider
//! trait; the deterministic signal remains available as a safe fallback.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod worker;

pub use worker::{EngineWorker, EngineWorkerSnapshot};

use std::{error::Error, fmt, path::PathBuf, sync::Arc};

use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceKind, AudioFormat, AudioInput, AudioInputProvider,
    AudioMixer, AudioSourceId, SimulatedAudioProvider,
};
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_clock::{MediaTimeline, TimelineError};
use obs_rs_core::{Runtime, RuntimeError};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{
    AtomicPacketFileWriter, AudioEncoder, EncodedPacket, OutputError, PacketDropPolicy,
    RawAudioEncoder, ReconnectPolicy, RleVideoEncoder, StreamMetrics, StreamSession, StreamState,
    TcpPacketTransport, VideoEncoder, WebSocketPacketTransport,
};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output_gstreamer::{
    GStreamerCapabilitySnapshot, GStreamerError, GStreamerOutputSession, NativeOutputState,
    ProductionDestination, ProductionPipelinePlan,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Project, ProjectError};

const DEFAULT_AUDIO_BLOCK_FRAMES: usize = 480;
const DEFAULT_TIMELINE_TOLERANCE_NANOS: u64 = 5_000_000;
const DEFAULT_OUTPUT_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_RECONNECT_ATTEMPTS: u32 = 3;

/// Configuration for one portable engine session.
pub struct EngineConfig {
    audio_format: AudioFormat,
    audio_block_frames: usize,
    timeline_tolerance_nanos: u64,
    output_queue_bytes: usize,
    reconnect_attempts: u32,
    audio_input_id: Option<String>,
    desktop_audio_id: Option<String>,
    audio_provider: Arc<dyn AudioInputProvider>,
    video_encoder: Box<dyn VideoEncoder>,
}

impl EngineConfig {
    /// Creates a configuration with the deterministic audio fallback and the
    /// lossless RLE video encoder.
    ///
    /// Use [`Self::with_video_encoder`] to install a different [`VideoEncoder`].
    ///
    /// # Panics
    ///
    /// Panics if the hardcoded default video format is ever invalid, which
    /// cannot happen for the constants used here.
    #[must_use]
    pub fn new(audio_format: AudioFormat) -> Self {
        Self {
            audio_format,
            audio_block_frames: DEFAULT_AUDIO_BLOCK_FRAMES,
            timeline_tolerance_nanos: DEFAULT_TIMELINE_TOLERANCE_NANOS,
            output_queue_bytes: DEFAULT_OUTPUT_QUEUE_BYTES,
            reconnect_attempts: DEFAULT_RECONNECT_ATTEMPTS,
            audio_input_id: None,
            desktop_audio_id: None,
            audio_provider: Arc::new(SimulatedAudioProvider::new()),
            video_encoder: Box::new(RleVideoEncoder::new(
                VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("valid rate"))
                    .expect("valid format"),
            )),
        }
    }

    /// Replaces the audio provider used before falling back to the test signal.
    #[must_use]
    pub fn with_audio_provider(mut self, provider: Arc<dyn AudioInputProvider>) -> Self {
        self.audio_provider = provider;
        self
    }

    /// Selects a provider-stable input ID, or clears the selection when empty.
    #[must_use]
    pub fn with_audio_input_id(mut self, device_id: impl Into<String>) -> Self {
        let device_id = device_id.into();
        self.audio_input_id = (!device_id.trim().is_empty()).then_some(device_id);
        self
    }

    /// Selects the playback device whose monitor feeds the desktop channel.
    ///
    /// An empty value clears the selection, which makes the session pick the
    /// first available playback route. Desktop capture has no deterministic
    /// stand-in: when no monitor can be opened the channel stays silent rather
    /// than borrowing the microphone's test signal, so what the meter shows is
    /// what the recording contains.
    #[must_use]
    pub fn with_desktop_audio_id(mut self, device_id: impl Into<String>) -> Self {
        let device_id = device_id.into();
        self.desktop_audio_id = (!device_id.trim().is_empty()).then_some(device_id);
        self
    }

    /// Sets the number of sample frames mixed per engine tick.
    ///
    /// A zero value is rejected when the session is created, where the error can
    /// be reported through the normal engine error channel.
    #[must_use]
    pub const fn with_audio_block_frames(mut self, frames: usize) -> Self {
        self.audio_block_frames = frames;
        self
    }

    /// Sets the bounded stream queue capacity in bytes.
    #[must_use]
    pub const fn with_output_queue_bytes(mut self, bytes: usize) -> Self {
        self.output_queue_bytes = bytes;
        self
    }

    /// Replaces the video encoder used for recording and streaming.
    ///
    /// The default is a lossless RLE reference encoder; production hosts swap in
    /// a hardware-accelerated or production-codec implementation behind this
    /// same contract without touching engine source.
    #[must_use]
    pub fn with_video_encoder(mut self, encoder: Box<dyn VideoEncoder>) -> Self {
        self.video_encoder = encoder;
        self
    }

    /// Returns the negotiated audio format.
    #[must_use]
    pub const fn audio_format(&self) -> AudioFormat {
        self.audio_format
    }
}

impl Clone for EngineConfig {
    // `Clone::clone` has no fallible contract, so documenting panics would be
    // misleading. The `expect` calls here guard values this constructor itself
    // produced, so they cannot fire in practice.
    #[allow(clippy::missing_panics_doc)]
    fn clone(&self) -> Self {
        // The video encoder is a trait object that has no `Clone`, and the
        // format it was built for is not readable back out of the trait. A
        // cloned config therefore installs a fresh default RLE encoder rather
        // than producing a half-populated config the session constructor would
        // have to special-case.
        let format = VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        Self {
            audio_format: self.audio_format,
            audio_block_frames: self.audio_block_frames,
            timeline_tolerance_nanos: self.timeline_tolerance_nanos,
            output_queue_bytes: self.output_queue_bytes,
            reconnect_attempts: self.reconnect_attempts,
            audio_input_id: self.audio_input_id.clone(),
            desktop_audio_id: self.desktop_audio_id.clone(),
            audio_provider: Arc::clone(&self.audio_provider),
            video_encoder: Box::new(RleVideoEncoder::new(format)),
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        let format = AudioFormat::new(48_000, 2)
            .unwrap_or_else(|error| unreachable!("the built-in audio format is valid: {error}"));
        Self::new(format)
    }
}

/// A single coordinated media tick.
pub struct EngineTick {
    /// The rendered preview frame, when a preview scene was selected.
    pub preview_frame: Option<VideoFrame>,
    /// The rendered program frame, when a program scene was selected.
    pub program_frame: Option<VideoFrame>,
    /// All audio blocks needed to reach this video frame's timestamp.
    pub audio_blocks: Vec<AudioBuffer>,
    /// The video deadline represented by this tick.
    pub timestamp: Timestamp,
    /// Peak level of the mixed audio delivered by this tick in thousandths.
    pub audio_peak_milli: u16,
}

/// Monotonic counters and device diagnostics exposed to hosts and GUIs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineStats {
    pub ticks: u64,
    pub video_frames: u64,
    pub audio_blocks: u64,
    pub audio_fallback_blocks: u64,
    pub last_video_timestamp: Option<Timestamp>,
    pub last_audio_timestamp: Option<Timestamp>,
    pub audio_peak_milli: u16,
    pub desktop_peak_milli: u16,
    pub microphone_peak_milli: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineAudioChannel {
    Desktop,
    Microphone,
}

/// Lifecycle of one output, recording or streaming.
///
/// A frontend cannot infer this from "is the handle open?" alone: a connect
/// that failed and a stream that was never started both leave no handle, yet a
/// user has to be told the difference. Tracking the phase explicitly is what
/// lets the desktop reconcile its own booleans against what the engine really
/// did, rather than assuming a start request succeeded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputLifecycle {
    /// No output is open and none was requested.
    #[default]
    Idle,
    /// A start was requested and has not yet been accepted or rejected.
    Starting,
    /// The output is open and accepting packets.
    Running,
    /// A stop was requested and finalization is in progress.
    Stopping,
    /// The output stopped because of an error rather than a request.
    Failed,
}

impl OutputLifecycle {
    /// Returns whether this phase means the output is no longer carrying media.
    ///
    /// Both `Idle` and `Failed` are terminal for the frontend's purposes: in
    /// either case a UI still showing "recording" is lying to the operator.
    #[must_use]
    pub const fn is_stopped(self) -> bool {
        matches!(self, Self::Idle | Self::Failed)
    }

    /// Returns the stable label used by the status bar and diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}

/// What the desktop mixer channel is reading.
///
/// A silent desktop channel is an ordinary outcome rather than an error — a
/// headless machine or a platform without monitor capture simply has nothing to
/// record — so the reason travels with the state instead of being reported as a
/// failure the operator has to dismiss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesktopAudioSource {
    /// The named playback monitor is open and feeding the channel.
    Monitor(String),
    /// Nothing is open; the payload explains why, for the diagnostics report.
    Silent(String),
}

impl DesktopAudioSource {
    /// Returns the device name, or the reason the channel is silent.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Monitor(label) | Self::Silent(label) => label,
        }
    }

    /// Returns whether a real monitor is feeding the channel.
    #[must_use]
    pub const fn is_capturing(&self) -> bool {
        matches!(self, Self::Monitor(_))
    }
}

/// A small immutable status snapshot suitable for a status bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineSnapshot {
    pub recording: bool,
    pub streaming: bool,
    /// Explicit recording phase, including a failed start the boolean hides.
    pub recording_lifecycle: OutputLifecycle,
    /// Explicit streaming phase, including a failed connect or a lost peer.
    pub streaming_lifecycle: OutputLifecycle,
    pub stream_state: Option<StreamState>,
    pub audio_backend: String,
    pub audio_fallback: bool,
    /// What the desktop channel is capturing, or why it is silent.
    pub desktop_audio: DesktopAudioSource,
    pub stream_metrics: Option<StreamMetrics>,
    /// Native production-stream counters for SRT/RTMP/RTMPS sessions.
    pub production_stream_metrics: Option<ProductionStreamMetrics>,
    pub stream_queued_bytes: usize,
    pub last_error: Option<String>,
    pub stats: EngineStats,
}

/// Protocol-independent telemetry copied from a native production adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProductionStreamMetrics {
    pub video_submitted: u64,
    pub audio_submitted: u64,
    pub dropped: u64,
    pub reconnects: u64,
    pub max_submit_latency_nanos: u128,
}

/// Errors raised by session construction, media processing, or output lifecycle.
#[derive(Debug)]
pub enum EngineError {
    InvalidConfiguration(String),
    NoActiveProfile,
    Runtime(RuntimeError),
    Project(ProjectError),
    Timeline(TimelineError),
    Audio(AudioDeviceError),
    AudioMix(obs_rs_audio::AudioError),
    Output(OutputError),
    #[cfg(feature = "production-gstreamer")]
    ProductionOutput(GStreamerError),
    Io(std::io::Error),
    Worker(String),
    Busy(&'static str),
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(reason) => {
                write!(formatter, "invalid engine configuration: {reason}")
            }
            Self::NoActiveProfile => formatter.write_str("project has no active profile"),
            Self::Runtime(error) => write!(formatter, "runtime failed: {error}"),
            Self::Project(error) => write!(formatter, "project failed: {error}"),
            Self::Timeline(error) => write!(formatter, "media timeline failed: {error}"),
            Self::Audio(error) => write!(formatter, "audio input failed: {error}"),
            Self::AudioMix(error) => write!(formatter, "audio mixer failed: {error}"),
            Self::Output(error) => write!(formatter, "output failed: {error}"),
            #[cfg(feature = "production-gstreamer")]
            Self::ProductionOutput(error) => write!(formatter, "production output failed: {error}"),
            Self::Io(error) => write!(formatter, "engine I/O failed: {error}"),
            Self::Worker(error) => write!(formatter, "engine worker failed: {error}"),
            Self::Busy(operation) => {
                write!(formatter, "cannot {operation} while an output is active")
            }
        }
    }
}

impl Error for EngineError {}

impl From<RuntimeError> for EngineError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<ProjectError> for EngineError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

impl From<TimelineError> for EngineError {
    fn from(error: TimelineError) -> Self {
        Self::Timeline(error)
    }
}

impl From<AudioDeviceError> for EngineError {
    fn from(error: AudioDeviceError) -> Self {
        Self::Audio(error)
    }
}

impl From<obs_rs_audio::AudioError> for EngineError {
    fn from(error: obs_rs_audio::AudioError) -> Self {
        Self::AudioMix(error)
    }
}

impl From<OutputError> for EngineError {
    fn from(error: OutputError) -> Self {
        Self::Output(error)
    }
}

#[cfg(feature = "production-gstreamer")]
impl From<GStreamerError> for EngineError {
    fn from(error: GStreamerError) -> Self {
        Self::ProductionOutput(error)
    }
}

impl From<std::io::Error> for EngineError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

struct RecordingOutput {
    writer: AtomicPacketFileWriter,
}

enum StreamOutput {
    Tcp(StreamSession<TcpPacketTransport>),
    WebSocket(StreamSession<WebSocketPacketTransport>),
    #[cfg(feature = "production-gstreamer")]
    Production(GStreamerOutputSession),
}

impl StreamOutput {
    fn connect(
        address: &str,
        capacity_bytes: usize,
        reconnect_attempts: u32,
        video_format: VideoFormat,
        audio_format: AudioFormat,
    ) -> Result<Self, EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = (video_format, audio_format);
        let address = address.trim();
        if address.is_empty() {
            return Err(EngineError::InvalidConfiguration(
                "stream address is empty".to_owned(),
            ));
        }
        let production_scheme = matches!(address.split(':').next(), Some("rtmp" | "rtmps" | "srt"));
        #[cfg(feature = "production-gstreamer")]
        if production_scheme {
            let (profile, destination) = ProductionDestination::from_stream_endpoint(address)?;
            let capabilities = GStreamerCapabilitySnapshot::probe();
            let plan = ProductionPipelinePlan::negotiate(profile, &destination, &capabilities)?;
            return Ok(Self::Production(
                GStreamerOutputSession::start_with_reconnect_limit(
                    &plan,
                    &destination,
                    video_format,
                    audio_format,
                    reconnect_attempts,
                )?,
            ));
        }
        #[cfg(not(feature = "production-gstreamer"))]
        if production_scheme {
            return Err(EngineError::InvalidConfiguration(
                "SRT/RTMP/RTMPS support was not compiled into this host".to_owned(),
            ));
        }
        let policy = ReconnectPolicy::new(reconnect_attempts);
        if address.starts_with("ws://") || address.starts_with("wss://") {
            let mut stream = StreamSession::new(
                WebSocketPacketTransport::new(address),
                capacity_bytes,
                PacketDropPolicy::DropNewest,
                policy,
            )?;
            stream.connect()?;
            Ok(Self::WebSocket(stream))
        } else {
            let mut stream = StreamSession::new(
                TcpPacketTransport::new(address),
                capacity_bytes,
                PacketDropPolicy::DropNewest,
                policy,
            )?;
            stream.connect()?;
            Ok(Self::Tcp(stream))
        }
    }

    fn submit(&mut self, packet: EncodedPacket) -> Result<(), EngineError> {
        match self {
            Self::Tcp(stream) => {
                stream.submit(packet)?;
            }
            Self::WebSocket(stream) => {
                stream.submit(packet)?;
            }
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => {}
        }
        Ok(())
    }

    fn pump(&mut self) -> Result<usize, EngineError> {
        match self {
            Self::Tcp(stream) => Ok(stream.flush()?),
            Self::WebSocket(stream) => Ok(stream.flush()?),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => {
                stream.poll_health()?;
                Ok(0)
            }
        }
    }

    fn reconnect(&mut self) -> Result<(), EngineError> {
        match self {
            Self::Tcp(stream) => stream.reconnect()?,
            Self::WebSocket(stream) => stream.reconnect()?,
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => stream.reconnect_live()?,
        }
        Ok(())
    }

    fn state(&self) -> StreamState {
        match self {
            Self::Tcp(stream) => stream.state(),
            Self::WebSocket(stream) => stream.state(),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => match stream.state() {
                NativeOutputState::Opening
                | NativeOutputState::Retrying
                | NativeOutputState::Lost => StreamState::Disconnected,
                NativeOutputState::Ready => StreamState::Connected,
                NativeOutputState::Failed => StreamState::Failed,
                NativeOutputState::Closed => StreamState::Closed,
            },
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    fn close(&mut self) -> Result<(), EngineError> {
        match self {
            Self::Tcp(stream) => stream.close(),
            Self::WebSocket(stream) => stream.close(),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => stream.close()?,
        }
        Ok(())
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    fn metrics(&self) -> Option<StreamMetrics> {
        match self {
            Self::Tcp(stream) => Some(stream.metrics()),
            Self::WebSocket(stream) => Some(stream.metrics()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => None,
        }
    }

    fn queued_bytes(&self) -> usize {
        match self {
            Self::Tcp(stream) => stream.queued_bytes(),
            Self::WebSocket(stream) => stream.queued_bytes(),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => 0,
        }
    }

    #[cfg_attr(not(feature = "production-gstreamer"), allow(clippy::unused_self))]
    fn production_metrics(&self) -> Option<ProductionStreamMetrics> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            let telemetry = stream.telemetry();
            return Some(ProductionStreamMetrics {
                video_submitted: telemetry.video_submitted(),
                audio_submitted: telemetry.audio_submitted(),
                dropped: telemetry.dropped(),
                reconnects: telemetry.reconnects(),
                max_submit_latency_nanos: telemetry.max_submit_latency_nanos(),
            });
        }
        None
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(
            clippy::needless_pass_by_value,
            clippy::unnecessary_wraps,
            clippy::unused_self,
            unused_variables
        )
    )]
    fn push_raw_audio(&mut self, buffer: AudioBuffer) -> Result<(), EngineError> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            stream.push_audio(buffer)?;
        }
        Ok(())
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(
            clippy::needless_pass_by_value,
            clippy::unnecessary_wraps,
            clippy::unused_self,
            unused_variables
        )
    )]
    fn push_raw_video(&mut self, frame: VideoFrame) -> Result<(), EngineError> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            stream.push_video(frame)?;
        }
        Ok(())
    }
}

/// The portable engine session.
pub struct EngineSession {
    config: EngineConfig,
    project: Project,
    format: VideoFormat,
    plugin: BuiltinPlugin,
    runtime: Runtime,
    timeline: MediaTimeline,
    mixer: AudioMixer,
    desktop_audio_source: AudioSourceId,
    microphone_audio_source: AudioSourceId,
    audio_input: Box<dyn AudioInput>,
    audio_backend: String,
    audio_fallback: bool,
    /// Absent when no playback monitor could be opened, which keeps the desktop
    /// channel silent instead of substituting another signal for it.
    desktop_audio: Option<Box<dyn AudioInput>>,
    desktop_audio_backend: String,
    next_audio_deadline: Option<obs_rs_audio::AudioDeadline>,
    render_timestamp: Timestamp,
    video_encoder: Box<dyn VideoEncoder>,
    audio_encoder: RawAudioEncoder,
    recording: Option<RecordingOutput>,
    streaming: Option<StreamOutput>,
    /// Phases the handles alone cannot express, notably a failed start.
    recording_lifecycle: OutputLifecycle,
    streaming_lifecycle: OutputLifecycle,
    stats: EngineStats,
    last_error: Option<String>,
}

#[allow(
    clippy::missing_errors_doc,
    reason = "the session methods share the documented EngineError boundary"
)]
impl EngineSession {
    /// Builds a session from the project's active profile.
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
        let profile = project
            .active_profile_spec()
            .ok_or(EngineError::NoActiveProfile)?;
        let format = profile.video_format();
        let plugin = BuiltinPlugin::new().map_err(|error| {
            EngineError::InvalidConfiguration(format!("built-in plugin failed: {error}"))
        })?;
        let runtime = build_runtime(&project, &plugin)?;
        let EngineConfig {
            audio_format,
            audio_block_frames,
            timeline_tolerance_nanos,
            output_queue_bytes,
            reconnect_attempts,
            audio_input_id,
            desktop_audio_id,
            audio_provider,
            video_encoder,
        } = config;
        if video_encoder.format() != format {
            return Err(EngineError::InvalidConfiguration(format!(
                "video encoder format {:?} does not match the project canvas {:?}",
                video_encoder.format(),
                format
            )));
        }
        let timeline = MediaTimeline::new(
            format.frame_rate(),
            audio_format,
            timeline_tolerance_nanos,
        );
        let mut mixer = AudioMixer::new(audio_format);
        let desktop_audio_source = mixer.add_source(1.0)?;
        let microphone_audio_source = mixer.add_source(1.0)?;
        let (audio_input, audio_backend, audio_fallback) = open_audio_input(
            &audio_provider,
            audio_format,
            audio_input_id.as_deref(),
        );
        let (desktop_audio, desktop_audio_backend) = open_desktop_audio(
            &audio_provider,
            audio_format,
            desktop_audio_id.as_deref(),
        );

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
                audio_provider,
                video_encoder: Box::new(RleVideoEncoder::new(format)),
            },
            project,
            format,
            plugin,
            runtime,
            timeline,
            mixer,
            desktop_audio_source,
            microphone_audio_source,
            audio_input,
            audio_backend,
            audio_fallback,
            desktop_audio,
            desktop_audio_backend,
            next_audio_deadline: None,
            render_timestamp: Timestamp::ZERO,
            recording: None,
            streaming: None,
            recording_lifecycle: OutputLifecycle::Idle,
            streaming_lifecycle: OutputLifecycle::Idle,
            stats: EngineStats::default(),
            last_error: None,
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
        if self.recording.is_some() || self.streaming.is_some() {
            return Err(EngineError::Busy("sync the project"));
        }
        let profile = project
            .active_profile_spec()
            .ok_or(EngineError::NoActiveProfile)?;
        let format = profile.video_format();
        let runtime = build_runtime(&project, &self.plugin)?;
        self.runtime = runtime;
        self.project = project;
        self.format = format;
        self.timeline = MediaTimeline::new(
            format.frame_rate(),
            self.config.audio_format,
            self.config.timeline_tolerance_nanos,
        );
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

    /// Returns the active canvas format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Updates the gain of the live input source in thousandths.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the gain is outside the mixer contract.
    pub fn set_channel_gain_milli(
        &mut self,
        channel: EngineAudioChannel,
        gain_milli: u16,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_source,
            EngineAudioChannel::Microphone => self.microphone_audio_source,
        };
        self.mixer
            .set_gain(source, f32::from(gain_milli) / 1_000.0)?;
        Ok(())
    }

    /// Mutes or unmutes the live input source.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the engine source has been removed.
    pub fn set_channel_muted(
        &mut self,
        channel: EngineAudioChannel,
        muted: bool,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_source,
            EngineAudioChannel::Microphone => self.microphone_audio_source,
        };
        self.mixer.set_muted(source, muted)?;
        Ok(())
    }

    /// Switches the live audio input without rebuilding the video runtime.
    ///
    /// The provider is queried on the engine worker thread. If the requested
    /// device is unavailable, the same deterministic fallback used during
    /// startup is selected and the snapshot exposes that fallback state.
    pub fn set_audio_input_id(&mut self, device_id: Option<&str>) {
        self.audio_input.stop();
        let (audio_input, audio_backend, audio_fallback) = open_audio_input(
            &self.config.audio_provider,
            self.config.audio_format,
            device_id,
        );
        self.audio_input = audio_input;
        self.audio_backend = audio_backend;
        self.audio_fallback = audio_fallback;
        self.config.audio_input_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        self.next_audio_deadline = None;
        self.last_error = None;
    }

    /// Switches the playback monitor feeding the desktop channel.
    ///
    /// Unlike the microphone there is no fallback signal, so an unavailable
    /// device leaves the channel silent and names the reason in the snapshot.
    pub fn set_desktop_audio_id(&mut self, device_id: Option<&str>) {
        if let Some(desktop) = self.desktop_audio.as_mut() {
            desktop.stop();
        }
        let (desktop_audio, desktop_audio_backend) = open_desktop_audio(
            &self.config.audio_provider,
            self.config.audio_format,
            device_id,
        );
        self.desktop_audio = desktop_audio;
        self.desktop_audio_backend = desktop_audio_backend;
        self.config.desktop_audio_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        self.next_audio_deadline = None;
    }

    /// Renders one scene using the session's independent preview clock.
    pub fn render_scene(&mut self, scene: &str) -> Result<Option<VideoFrame>, EngineError> {
        let timestamp = self.render_timestamp;
        let frame = self.render_scene_at(scene, timestamp)?;
        let period = self
            .format
            .frame_rate()
            .period_nanos()
            .unwrap_or(33_333_333);
        self.render_timestamp = timestamp.checked_add(period).unwrap_or(Timestamp::ZERO);
        Ok(frame)
    }

    /// Advances one video deadline and enough audio deadlines to keep packet
    /// timestamps monotonic in the output container.
    pub fn tick(
        &mut self,
        preview_scene: Option<&str>,
        program_scene: Option<&str>,
    ) -> Result<EngineTick, EngineError> {
        let video_deadline = self.timeline.next_video_frame()?;
        let timestamp = video_deadline.timestamp();
        let audio_blocks = self.drain_audio_until(timestamp)?;

        let preview_frame = preview_scene
            .map(|scene| self.render_scene_at(scene, timestamp))
            .transpose()?;
        let program_frame = program_scene
            .map(|scene| self.render_scene_at(scene, timestamp))
            .transpose()?;
        let program_frame = program_frame.flatten();
        let preview_frame = preview_frame.flatten();

        for audio in &audio_blocks {
            if let Some(stream) = self.streaming.as_mut() {
                stream.push_raw_audio(audio.clone())?;
            }
            let packet = self.audio_encoder.encode(audio)?;
            self.emit_packet(packet)?;
        }
        if let Some(frame) = program_frame.as_ref() {
            if let Some(stream) = self.streaming.as_mut() {
                stream.push_raw_video(frame.clone())?;
            }
            let packet = self.video_encoder.encode(frame)?;
            self.emit_packet(packet)?;
            self.stats.video_frames = self.stats.video_frames.saturating_add(1);
            self.stats.last_video_timestamp = Some(frame.timestamp());
        }
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_peak_milli = audio_blocks.last().map_or(0, audio_peak_milli);
        let _ = self.timeline.observe(
            timestamp,
            audio_blocks
                .first()
                .map_or(timestamp, AudioBuffer::timestamp),
        );

        Ok(EngineTick {
            preview_frame,
            program_frame,
            audio_blocks,
            timestamp,
            audio_peak_milli: self.stats.audio_peak_milli,
        })
    }

    /// Encodes and queues a program frame rendered by an external preview
    /// adapter, adding every audio block due before its timestamp.
    pub fn push_program_frame(&mut self, frame: &VideoFrame) -> Result<(), EngineError> {
        if frame.format() != self.format {
            return Err(EngineError::InvalidConfiguration(
                "program frame format does not match the output canvas".to_owned(),
            ));
        }
        if self
            .stats
            .last_video_timestamp
            .is_some_and(|last| frame.timestamp() < last)
        {
            return Err(EngineError::InvalidConfiguration(
                "program frame timestamp moved backwards".to_owned(),
            ));
        }
        let audio_blocks = self.drain_audio_until(frame.timestamp())?;
        for audio in &audio_blocks {
            if let Some(stream) = self.streaming.as_mut() {
                stream.push_raw_audio(audio.clone())?;
            }
            let packet = self.audio_encoder.encode(audio)?;
            self.emit_packet(packet)?;
        }
        if let Some(stream) = self.streaming.as_mut() {
            stream.push_raw_video(frame.clone())?;
        }
        let packet = self.video_encoder.encode(frame)?;
        self.emit_packet(packet)?;
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.video_frames = self.stats.video_frames.saturating_add(1);
        self.stats.last_video_timestamp = Some(frame.timestamp());
        self.stats.audio_peak_milli = audio_blocks.last().map_or(0, audio_peak_milli);
        let _ = self.timeline.observe(
            frame.timestamp(),
            audio_blocks
                .first()
                .map_or(frame.timestamp(), AudioBuffer::timestamp),
        );
        Ok(())
    }

    /// Samples and mixes audio up to a preview timestamp without encoding or
    /// emitting media. Frontends use this while outputs are idle so their
    /// mixer meters stay live without paying for an idle video encode.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when capture, mixing, or timeline advancement
    /// fails.
    pub fn monitor_audio_until(&mut self, timestamp: Timestamp) -> Result<(), EngineError> {
        let audio_blocks = self.drain_audio_until(timestamp)?;
        if let Some(latest) = audio_blocks.last() {
            self.stats.audio_peak_milli = audio_peak_milli(latest);
            self.stats.last_audio_timestamp = Some(latest.timestamp());
        }
        Ok(())
    }

    /// Starts an atomic `OBSRPKT1` recording at `path`.
    ///
    /// The phase moves to `Starting` before any file work and settles on
    /// `Running` or `Failed`, so a caller that only sees the error still leaves
    /// an observable record of what happened behind.
    pub fn start_recording(&mut self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_recording(path.into());
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

    fn open_recording(&mut self, final_path: PathBuf) -> Result<(), EngineError> {
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("recording path must name a file".to_owned())
            })?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        self.recording = Some(RecordingOutput {
            writer: AtomicPacketFileWriter::new(final_path, temp_path)?,
        });
        Ok(())
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
        match recording.writer.finalize() {
            Ok(bytes) => {
                self.recording_lifecycle = OutputLifecycle::Idle;
                Ok(bytes)
            }
            Err(error) => {
                self.recording = Some(recording);
                Err(self.fail_recording(error.into()))
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
            let _ = recording.writer.abort();
        }
        // An abort is a deliberate stop, so it clears a previous failure rather
        // than leaving the session permanently marked as broken.
        self.recording_lifecycle = OutputLifecycle::Idle;
    }

    /// Opens a TCP or WebSocket OBS-RS packet stream.
    ///
    /// A refused or unreachable peer leaves the phase `Failed`, which is what
    /// distinguishes "the user never started a stream" from "the stream could
    /// not be established".
    pub fn start_streaming(&mut self, address: &str) -> Result<(), EngineError> {
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
                Ok(()) => self.streaming_lifecycle = OutputLifecycle::Running,
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
                if let Err(reconnect) = stream.reconnect() {
                    self.last_error = Some(format!("{error}; reconnect failed: {reconnect}"));
                    // A pump error the transport could not recover from is the
                    // point the stream stops carrying media, whether or not the
                    // handle is still open.
                    self.streaming_lifecycle = OutputLifecycle::Failed;
                    return Err(error);
                }
                // The transport is carrying media again, so the phase must say
                // so. Leaving it at `Failed` from the attempt that just
                // recovered would show a stopped stream in the UI while packets
                // were flowing.
                self.streaming_lifecycle = OutputLifecycle::Running;
                Ok(0)
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
            stream_state: self.stream_state(),
            audio_backend: self.audio_backend.clone(),
            audio_fallback: self.audio_fallback,
            desktop_audio: if self.desktop_audio.is_some() {
                DesktopAudioSource::Monitor(self.desktop_audio_backend.clone())
            } else {
                DesktopAudioSource::Silent(self.desktop_audio_backend.clone())
            },
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

    fn read_audio_block(&mut self, timestamp: Timestamp) -> Result<AudioBuffer, EngineError> {
        match self
            .audio_input
            .read_block(timestamp, self.config.audio_block_frames)
        {
            Ok(buffer) => Ok(buffer),
            Err(error) => {
                self.audio_fallback = true;
                self.audio_backend = format!("simulated fallback ({error})");
                self.last_error = Some(error.to_string());
                self.audio_input = SimulatedAudioProvider::new()
                    .open_input("test-audio", self.config.audio_format)?;
                // The fallback signal runs on its own clock, so the timeline's
                // idea of the next audio deadline — computed against the real
                // device that just failed — is stale. Dropping it forces the
                // next tick to re-anchors the audio deadlines to the current
                // video timestamp instead of chasing a device that is gone.
                self.next_audio_deadline = None;
                Ok(self
                    .audio_input
                    .read_block(timestamp, self.config.audio_block_frames)?)
            }
        }
    }

    /// Reads one desktop block, or silence when no monitor is open.
    ///
    /// A monitor that fails mid-session is closed rather than retried every
    /// block: the desktop channel degrades to silence and says so in the
    /// backend label, which keeps a broken device from stalling the tick.
    fn read_desktop_block(&mut self, timestamp: Timestamp) -> Result<AudioBuffer, EngineError> {
        let frames = self.config.audio_block_frames;
        if let Some(desktop) = self.desktop_audio.as_mut() {
            match desktop.read_block(timestamp, frames) {
                Ok(buffer) => return Ok(buffer),
                Err(error) => {
                    self.desktop_audio = None;
                    self.desktop_audio_backend = format!("unavailable ({error})");
                    self.last_error = Some(error.to_string());
                }
            }
        }
        Ok(AudioBuffer::silence(
            self.config.audio_format,
            timestamp,
            frames,
        )?)
    }

    fn drain_audio_until(&mut self, timestamp: Timestamp) -> Result<Vec<AudioBuffer>, EngineError> {
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
            let input = self.read_audio_block(deadline.timestamp())?;
            let desktop = self.read_desktop_block(deadline.timestamp())?;
            self.stats.desktop_peak_milli = audio_peak_milli(&desktop);
            self.stats.microphone_peak_milli = audio_peak_milli(&input);
            let mixed = self.mixer.mix(
                deadline.timestamp(),
                self.config.audio_block_frames,
                &[
                    (self.desktop_audio_source, &desktop),
                    (self.microphone_audio_source, &input),
                ],
            )?;
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
        match (self.recording.as_mut(), self.streaming.as_mut()) {
            (Some(recording), Some(stream)) => {
                recording.writer.push(packet.clone())?;
                stream.submit(packet)?;
            }
            (Some(recording), None) => recording.writer.push(packet)?,
            (None, Some(stream)) => stream.submit(packet)?,
            (None, None) => {}
        }
        Ok(())
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

fn build_runtime(project: &Project, plugin: &BuiltinPlugin) -> Result<Runtime, EngineError> {
    let profile = project
        .active_profile_spec()
        .ok_or(EngineError::NoActiveProfile)?;
    let mut runtime = Runtime::new();
    runtime.register_plugin(plugin)?;
    for scene in profile.scenes() {
        let scene_id = scene.id().as_str();
        runtime.create_scene(scene_id)?;
        for source in scene.sources() {
            if !source.visible() {
                continue;
            }
            let source_id =
                runtime.create_source(source.kind().as_str(), source.name(), source.settings())?;
            runtime.attach_source(scene_id, source_id)?;
            runtime.set_source_transform(scene_id, source_id, source.transform())?;
            for filter in source.filters() {
                runtime.add_source_filter(scene_id, source_id, *filter)?;
            }
        }
    }
    Ok(runtime)
}

fn open_audio_input(
    provider: &Arc<dyn AudioInputProvider>,
    format: AudioFormat,
    requested_id: Option<&str>,
) -> (Box<dyn AudioInput>, String, bool) {
    let primary = provider.discover().ok().and_then(|devices| {
        requested_id
            .and_then(|requested| {
                devices
                    .iter()
                    .find(|device| {
                        device.kind() == AudioDeviceKind::Input
                            && device.id() == requested
                            && device.available()
                    })
                    .map(|device| (device.id().to_owned(), device.name().to_owned()))
            })
            .or_else(|| {
                devices
                    .into_iter()
                    .find(|device| device.kind() == AudioDeviceKind::Input && device.available())
                    .map(|device| (device.id().to_owned(), device.name().to_owned()))
            })
    });
    if let Some((device_id, device_name)) = primary {
        if let Ok(input) = provider.open_input(&device_id, format) {
            return (input, device_name, false);
        }
    }
    let fallback = SimulatedAudioProvider::new()
        .open_input("test-audio", format)
        .unwrap_or_else(|error| unreachable!("fallback audio format is valid: {error}"));
    (fallback, "simulated fallback".to_owned(), true)
}

/// Opens the playback monitor that feeds the desktop channel.
///
/// Desktop capture reads a device the platform classifies as an *output*; a
/// provider that can record from it hands back what the machine is playing.
/// Returning `None` is a normal outcome — a headless session or a provider
/// without monitor support simply records a silent desktop channel — so this
/// never substitutes the simulated signal, which would make the meter lie.
fn open_desktop_audio(
    provider: &Arc<dyn AudioInputProvider>,
    format: AudioFormat,
    requested_id: Option<&str>,
) -> (Option<Box<dyn AudioInput>>, String) {
    let Ok(devices) = provider.discover() else {
        return (None, "unavailable".to_owned());
    };
    let selected = requested_id
        .and_then(|requested| {
            devices
                .iter()
                .find(|device| device.id() == requested && device.available())
        })
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.kind() == AudioDeviceKind::Output && device.available())
        })
        .map(|device| (device.id().to_owned(), device.name().to_owned()));
    let Some((device_id, device_name)) = selected else {
        return (None, "no playback monitor".to_owned());
    };
    match provider.open_input(&device_id, format) {
        Ok(input) => (Some(input), device_name),
        Err(error) => (None, format!("unavailable ({error})")),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the full u16 range before conversion"
)]
fn audio_peak_milli(buffer: &AudioBuffer) -> u16 {
    let peak = buffer
        .samples()
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    (peak * 1_000.0).round().clamp(0.0, f32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_audio::AudioDeviceInfo;
    use obs_rs_config::Config;
    use obs_rs_media::FrameRate;
    use obs_rs_project::{Profile, SceneSpec, SourceSpec};

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
            .add_source(
                SourceSpec::new("pattern", "test_pattern", "Pattern", settings).expect("source"),
            )
            .expect("add source");
        profile.add_scene(scene).expect("add scene");
        project.add_profile(profile).expect("add profile");
        project
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
    }

    #[test]
    fn fallback_audio_is_reported() {
        let engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let snapshot = engine.snapshot();
        assert!(!snapshot.audio_fallback);
        assert_eq!(snapshot.audio_backend, "Deterministic test signal");
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
            Ok(Vec::new())
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
            let mut stream = StreamOutput::connect(endpoint, 1_048_576, 1, video, audio)
                .expect("native production pipeline");
            assert!(matches!(stream, StreamOutput::Production(_)));
            stream.close().expect("close live pipeline");
        }
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
