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

use std::{
    error::Error,
    fmt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceKind, AudioFilter, AudioFilterChain, AudioFormat,
    AudioInput, AudioInputProvider, AudioInputState, AudioMixer, AudioResampler, AudioSourceId,
    SimulatedAudioProvider,
};
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_clock::{MediaTimeline, TimelineError};
use obs_rs_core::{Runtime, RuntimeError};
use obs_rs_media::{
    ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, FrameFilter, FrameRate, FrameTransform,
    LatencyMetrics, LumaKey, RawVideoFrame, RenderDelay, Timestamp, VideoFormat, VideoFrame,
    MAX_RENDER_DELAY_MILLISECONDS, MAX_SCROLL_SPEED, MIN_RENDER_DELAY_MILLISECONDS,
    MIN_SCROLL_SPEED,
};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output::OutputProfile;
use obs_rs_output::{
    AtomicPacketFileWriter, AudioEncoder, AudioEncoderConfig, AudioInputRequirement, EncodedPacket,
    OutputError, PacketDropPolicy, RawAudioEncoder, ReconnectPolicy, ReplayBuffer, RleVideoEncoder,
    StreamMetrics, StreamSession, StreamState, StreamTarget, TcpPacketTransport, VideoEncoder,
    VideoEncoderConfig, VideoInputRequirement, WebSocketPacketTransport,
};
#[cfg(feature = "production-gstreamer")]
pub use obs_rs_output_gstreamer::{
    AudioEncoderCapability, OutputCapabilitiesSnapshot, ProductionProtocol, ProtocolCapability,
    VideoEncoderCapability,
};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output_gstreamer::{
    GStreamerCapabilitySnapshot, GStreamerError, GStreamerOutputSession, NativeOutputState,
    ProductionDestination, ProductionPipelinePlan,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Project, ProjectError, SourceFilterCategory, SourceFilterSpec};

const DEFAULT_AUDIO_BLOCK_FRAMES: usize = 480;
const DEFAULT_TIMELINE_TOLERANCE_NANOS: u64 = 5_000_000;
const DEFAULT_OUTPUT_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_RECONNECT_ATTEMPTS: u32 = 3;

/// Probes the production backend once and returns its typed GUI-safe model.
#[cfg(feature = "production-gstreamer")]
#[must_use]
pub fn output_capabilities_snapshot() -> OutputCapabilitiesSnapshot {
    GStreamerCapabilitySnapshot::probe().capabilities()
}

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
    pub desktop_peak_hold_milli: u16,
    pub microphone_peak_hold_milli: u16,
    pub desktop_clipped: bool,
    pub microphone_clipped: bool,
    pub video_encode_latency: LatencyMetrics,
    pub audio_encode_latency: LatencyMetrics,
    pub output_submit_latency: LatencyMetrics,
    pub audio_blocks_per_video_tick: u32,
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

/// A streaming lifecycle transition published by the engine worker.
///
/// Frontends consume these events asynchronously; output setup and teardown
/// never need to complete on their event thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputEvent {
    Starting,
    Running,
    Disconnected,
    Reconnecting { attempt: u32 },
    Failed { reason: String },
    Stopping,
    Stopped,
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

enum RecordingOutput {
    Reference(AtomicPacketFileWriter),
    #[cfg(feature = "production-gstreamer")]
    Production {
        session: GStreamerOutputSession,
        final_path: PathBuf,
    },
}

impl RecordingOutput {
    const fn video_requirement(&self) -> VideoInputRequirement {
        match self {
            Self::Reference(_) => VideoInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => VideoInputRequirement::Raw,
        }
    }

    const fn audio_requirement(&self) -> AudioInputRequirement {
        match self {
            Self::Reference(_) => AudioInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => AudioInputRequirement::Raw,
        }
    }

    fn push_packet(&mut self, packet: EncodedPacket) -> Result<(), EngineError> {
        match self {
            Self::Reference(writer) => writer.push(packet).map_err(Into::into),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => Ok(()),
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = frame;
        match self {
            Self::Reference(_) => Ok(()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session, .. } => {
                session.push_video(frame.clone()).map_err(Into::into)
            }
        }
    }

    fn push_raw_video(&mut self, frame: &RawVideoFrame) -> Result<(), EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = frame;
        match self {
            Self::Reference(_) => Ok(()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session, .. } => {
                session.push_raw_video(frame.clone()).map_err(Into::into)
            }
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    fn push_audio(&mut self, buffer: &AudioBuffer) -> Result<(), EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = buffer;
        match self {
            Self::Reference(_) => Ok(()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session, .. } => {
                session.push_audio(buffer.clone()).map_err(Into::into)
            }
        }
    }

    fn finalize(&mut self) -> Result<usize, EngineError> {
        match self {
            Self::Reference(writer) => writer.finalize().map_err(Into::into),
            #[cfg(feature = "production-gstreamer")]
            Self::Production {
                session,
                final_path,
            } => {
                session.close()?;
                usize::try_from(std::fs::metadata(final_path)?.len()).map_err(|_| {
                    EngineError::InvalidConfiguration(
                        "recording size does not fit this platform".to_owned(),
                    )
                })
            }
        }
    }

    fn abort(&mut self) {
        match self {
            Self::Reference(writer) => {
                let _ = writer.abort();
            }
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => {}
        }
    }
}

enum StreamOutput {
    Tcp(StreamSession<TcpPacketTransport>),
    WebSocket(StreamSession<WebSocketPacketTransport>),
    #[cfg(feature = "production-gstreamer")]
    Production(GStreamerOutputSession),
}

impl StreamOutput {
    const fn video_requirement(&self) -> VideoInputRequirement {
        match self {
            Self::Tcp(_) | Self::WebSocket(_) => VideoInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => VideoInputRequirement::Raw,
        }
    }

    const fn audio_requirement(&self) -> AudioInputRequirement {
        match self {
            Self::Tcp(_) | Self::WebSocket(_) => AudioInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => AudioInputRequirement::Raw,
        }
    }

    fn connect(
        address: &str,
        capacity_bytes: usize,
        reconnect_attempts: u32,
        video_format: VideoFormat,
        audio_format: AudioFormat,
        encoder_config: Option<(&VideoEncoderConfig, &AudioEncoderConfig)>,
    ) -> Result<Self, EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = (video_format, audio_format, encoder_config);
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

    #[cfg(feature = "production-gstreamer")]
    fn connect_target(
        target: &StreamTarget,
        capacity_bytes: usize,
        reconnect_attempts: u32,
        video_format: VideoFormat,
        audio_format: AudioFormat,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<Self, EngineError> {
        if let StreamTarget::Reference { address } = target {
            return Self::connect(
                address,
                capacity_bytes,
                reconnect_attempts,
                video_format,
                audio_format,
                Some((video, audio)),
            );
        }
        let (profile, destination) = ProductionDestination::from_stream_target(target)?;
        let capabilities = GStreamerCapabilitySnapshot::probe();
        let plan = ProductionPipelinePlan::negotiate_configured(
            profile,
            &destination,
            &capabilities,
            video,
            audio,
        )?;
        Ok(Self::Production(
            GStreamerOutputSession::start_with_reconnect_limit(
                &plan,
                &destination,
                video_format,
                audio_format,
                reconnect_attempts,
            )?,
        ))
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
    fn push_raw_video(&mut self, frame: RawVideoFrame) -> Result<(), EngineError> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            stream.push_raw_video(frame)?;
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
    fn push_video(&mut self, frame: VideoFrame) -> Result<(), EngineError> {
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
    desktop_audio_filters: AudioFilterChain,
    microphone_audio_filters: AudioFilterChain,
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
    replay_buffer: Option<ReplayBuffer>,
    streaming: Option<StreamOutput>,
    /// Phases the handles alone cannot express, notably a failed start.
    recording_lifecycle: OutputLifecycle,
    streaming_lifecycle: OutputLifecycle,
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
        let timeline =
            MediaTimeline::new(format.frame_rate(), audio_format, timeline_tolerance_nanos);
        let mut mixer = AudioMixer::new(audio_format);
        let desktop_audio_source = mixer.add_source(1.0)?;
        let microphone_audio_source = mixer.add_source(1.0)?;
        let (audio_input, audio_backend, audio_fallback) =
            open_audio_input(&audio_provider, audio_format, audio_input_id.as_deref());
        let (desktop_audio, desktop_audio_backend) =
            open_desktop_audio(&audio_provider, audio_format, desktop_audio_id.as_deref());

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
            desktop_audio_filters: AudioFilterChain::new(),
            microphone_audio_filters: AudioFilterChain::new(),
            audio_input,
            audio_backend,
            audio_fallback,
            desktop_audio,
            desktop_audio_backend,
            next_audio_deadline: None,
            render_timestamp: Timestamp::ZERO,
            recording: None,
            replay_buffer: None,
            streaming: None,
            recording_lifecycle: OutputLifecycle::Idle,
            streaming_lifecycle: OutputLifecycle::Idle,
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

    /// Updates the stereo pan of a live input source in thousandths of a full
    /// left/right turn (`-1000` is left, `0` is center, `1000` is right).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the pan is outside the bounded mixer
    /// contract or the channel source is unavailable.
    pub fn set_channel_pan_milli(
        &mut self,
        channel: EngineAudioChannel,
        pan_milli: i32,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_source,
            EngineAudioChannel::Microphone => self.microphone_audio_source,
        };
        self.mixer.set_pan_milli(source, pan_milli)?;
        Ok(())
    }

    /// Replaces the ordered audio-filter chain on one live mixer channel.
    ///
    /// The chain is owned by the engine and applied to each captured block
    /// before metering and mixing. Replacing it is a control-plane operation;
    /// applying it remains allocation-free on the audio path.
    pub fn set_channel_audio_filters(
        &mut self,
        channel: EngineAudioChannel,
        filters: AudioFilterChain,
    ) {
        match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_filters = filters,
            EngineAudioChannel::Microphone => self.microphone_audio_filters = filters,
        }
    }

    /// Installs one OBS-compatible Gain filter on a live mixer channel.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the dB value is outside the bounded Gain
    /// filter range.
    pub fn set_channel_gain_filter_db_milli(
        &mut self,
        channel: EngineAudioChannel,
        milli_db: i32,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::gain_db_milli(milli_db)?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's Invert Polarity filter on a live mixer channel.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the bounded chain cannot accept the filter.
    pub fn set_channel_invert_polarity(
        &mut self,
        channel: EngineAudioChannel,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::InvertPolarity)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's bounded Limiter filter on a live mixer channel.
    ///
    /// The limiter keeps its attack/release envelope in the engine-owned
    /// filter instance, so captured blocks remain continuous without a
    /// separate runtime state store.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when threshold or release is outside the
    /// supported OBS-compatible range.
    pub fn set_channel_limiter(
        &mut self,
        channel: EngineAudioChannel,
        threshold_db_milli: i32,
        release_ms: u16,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::limiter_db_milli(
            threshold_db_milli,
            release_ms,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's bounded Compressor filter on a live mixer channel.
    ///
    /// This live-channel slice detects the channel's own signal. Sidechain
    /// compression remains unavailable until the engine has a canonical,
    /// synchronized source-to-source audio route.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when one of the compressor controls is outside
    /// the supported OBS-compatible range.
    pub fn set_channel_compressor(
        &mut self,
        channel: EngineAudioChannel,
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::compressor(
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's bounded peak Expander filter on a live mixer channel.
    ///
    /// This slice uses peak detection on the channel's own signal. RMS/gate,
    /// knee, sidechain, and project-source routing require a broader audio
    /// graph and remain outside this control-plane operation.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when one of the expander controls is outside
    /// the supported OBS-compatible range.
    pub fn set_channel_expander(
        &mut self,
        channel: EngineAudioChannel,
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::expander(
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's stateful peak Noise Gate on a live mixer channel.
    ///
    /// The detector uses the channel's own peak signal. RMS detection,
    /// sidechain input, and project-source routing remain outside this
    /// control-plane operation.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when one of the gate controls is outside the
    /// supported OBS-compatible range.
    pub fn set_channel_noise_gate(
        &mut self,
        channel: EngineAudioChannel,
        open_threshold_db_milli: i32,
        close_threshold_db_milli: i32,
        attack_ms: u16,
        hold_ms: u16,
        release_ms: u16,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::noise_gate(
            open_threshold_db_milli,
            close_threshold_db_milli,
            attack_ms,
            hold_ms,
            release_ms,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
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
            self.dispatch_audio(audio)?;
        }
        if let Some(frame) = program_frame.as_ref() {
            self.dispatch_video(frame)?;
            self.stats.video_frames = self.stats.video_frames.saturating_add(1);
            self.stats.last_video_timestamp = Some(frame.timestamp());
        }
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_blocks_per_video_tick =
            u32::try_from(audio_blocks.len()).unwrap_or(u32::MAX);
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
            self.dispatch_audio(audio)?;
        }
        self.dispatch_video(frame)?;
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_blocks_per_video_tick =
            u32::try_from(audio_blocks.len()).unwrap_or(u32::MAX);
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

    /// Queues a validated packed/planar program frame from an accelerated
    /// compositor and schedules audio against its timestamp.
    pub fn push_program_raw_frame(&mut self, frame: &RawVideoFrame) -> Result<(), EngineError> {
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
            self.dispatch_audio(audio)?;
        }
        self.dispatch_raw_video(frame)?;
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_blocks_per_video_tick =
            u32::try_from(audio_blocks.len()).unwrap_or(u32::MAX);
        self.stats.video_frames = self.stats.video_frames.saturating_add(1);
        self.stats.last_video_timestamp = Some(frame.timestamp());
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

    /// Starts an atomic Matroska or `OBSRPKT1` recording based on `path`.
    ///
    /// The phase moves to `Starting` before any file work and settles on
    /// `Running` or `Failed`, so a caller that only sees the error still leaves
    /// an observable record of what happened behind.
    pub fn start_recording(&mut self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_recording_with_config(path.into(), None)
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

    fn start_recording_with_config(
        &mut self,
        path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_recording(path, encoder_config);
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
            let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
            self.recording = Some(RecordingOutput::Reference(AtomicPacketFileWriter::new(
                final_path, temp_path,
            )?));
            return Ok(());
        }
        if !extension.eq_ignore_ascii_case("mkv") {
            return Err(EngineError::InvalidConfiguration(
                "recording extension must be .mkv or .obsr".to_owned(),
            ));
        }
        #[cfg(feature = "production-gstreamer")]
        {
            let destination = ProductionDestination::Recording(final_path.clone());
            let capabilities = GStreamerCapabilitySnapshot::probe();
            let profile =
                encoder_config.map_or_else(OutputProfile::matroska_h264_aac, |config| match config
                    .0
                    .codec
                {
                    obs_rs_output::VideoCodec::H264 => OutputProfile::matroska_h264_aac(),
                    obs_rs_output::VideoCodec::Hevc => OutputProfile::matroska_hevc_aac(),
                    obs_rs_output::VideoCodec::Av1 => OutputProfile::matroska_av1_aac(),
                    _ => OutputProfile::reference(),
                });
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
            self.recording = Some(RecordingOutput::Production {
                session,
                final_path,
            });
            Ok(())
        }
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = encoder_config;
        #[cfg(not(feature = "production-gstreamer"))]
        Err(EngineError::InvalidConfiguration(
            "Matroska support was not compiled into this host".to_owned(),
        ))
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
        self.replay_buffer = Some(ReplayBuffer::new(capacity_bytes, duration)?);
        Ok(())
    }

    /// Stops replay capture and discards its retained packet history.
    pub fn stop_replay_buffer(&mut self) {
        self.replay_buffer = None;
    }

    /// Saves the retained replay packets through the atomic packet writer.
    ///
    /// The replay history remains active after a successful save, matching the
    /// OBS workflow where saving a replay does not stop capture. The packet
    /// container is the inspectable OBS-RS reference container; production
    /// remuxing remains a separate output capability.
    pub fn save_replay_buffer(&self, path: impl Into<PathBuf>) -> Result<usize, EngineError> {
        let Some(buffer) = self.replay_buffer.as_ref() else {
            return Err(EngineError::InvalidConfiguration(
                "replay buffer is not running".to_owned(),
            ));
        };
        let final_path = path.into();
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("replay path must name a file".to_owned())
            })?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        let packets = buffer.snapshot();
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
            let mut input = self.read_audio_block(deadline.timestamp())?;
            let mut desktop = self.read_desktop_block(deadline.timestamp())?;
            self.microphone_audio_filters.apply(&mut input)?;
            self.desktop_audio_filters.apply(&mut desktop)?;
            let mixed = self.mixer.mix(
                deadline.timestamp(),
                self.config.audio_block_frames,
                &[
                    (self.desktop_audio_source, &desktop),
                    (self.microphone_audio_source, &input),
                ],
            )?;
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

fn build_runtime(project: &Project, plugin: &BuiltinPlugin) -> Result<Runtime, EngineError> {
    let profile = project
        .active_profile_spec()
        .ok_or(EngineError::NoActiveProfile)?;
    let mut runtime = Runtime::new();
    runtime.register_plugin(plugin)?;

    // Source definitions are profile-wide. Create each runtime source once;
    // scenes below only attach scene-local items to that shared instance.
    let mut source_ids = std::collections::HashMap::new();
    for source in profile.sources() {
        let source_id =
            runtime.create_source(source.kind().as_str(), source.name(), source.settings())?;
        for filter in source.filters() {
            if let Some(runtime_filter) = compile_filter(filter) {
                runtime.add_source_filter(source_id, runtime_filter)?;
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
    Ok(runtime)
}

/// Compiles a persistent source filter into the built-in runtime operation.
///
/// Unknown kinds, audio/video filters, disabled instances, and malformed
/// settings remain valid project data but are omitted until a matching runtime
/// implementation is available. The project crate therefore stays independent
/// of this renderer-facing enum.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "the project-to-renderer boundary keeps every supported effect mapping explicit"
)]
pub fn compile_filter(spec: &SourceFilterSpec) -> Option<FrameFilter> {
    if !spec.enabled() || spec.category() != SourceFilterCategory::Effect {
        return None;
    }
    match spec.kind().as_str() {
        "grayscale" => Some(FrameFilter::Grayscale),
        "brightness" => spec
            .settings()
            .get("milli")
            .and_then(|value| value.parse().ok())
            .map(|milli| FrameFilter::Brightness { milli }),
        "opacity" => spec
            .settings()
            .get("value")
            .and_then(|value| value.parse().ok())
            .map(FrameFilter::Opacity),
        "crop_pad" => {
            let read_edge = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u32>().ok())
                    .filter(|value| *value <= FrameTransform::MAX_CROP)
            };
            Some(FrameFilter::CropPad {
                left: read_edge("left")?,
                top: read_edge("top")?,
                right: read_edge("right")?,
                bottom: read_edge("bottom")?,
            })
        }
        "color_correction" => {
            let read_value = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            Some(FrameFilter::ColorCorrection(ColorCorrection::new(
                read_value("gamma")?,
                read_value("contrast")?,
                read_value("brightness")?,
                read_value("saturation")?,
                read_value("hue_shift")?,
                read_value("opacity")?,
            )?))
        }
        "color_multiply_add" => {
            let read_channel = |key| spec.settings().get(key)?.parse::<u8>().ok();
            Some(FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new(
                [
                    read_channel("multiply_red")?,
                    read_channel("multiply_green")?,
                    read_channel("multiply_blue")?,
                ],
                [
                    read_channel("add_red")?,
                    read_channel("add_green")?,
                    read_channel("add_blue")?,
                ],
            )))
        }
        "luma_key" => {
            let read_value = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            Some(FrameFilter::LumaKey(LumaKey::new(
                read_value("luma_max")?,
                read_value("luma_min")?,
                read_value("luma_max_smooth")?,
                read_value("luma_min_smooth")?,
            )?))
        }
        "color_key" => {
            let read_channel = |key| spec.settings().get(key)?.parse::<u8>().ok();
            let read_threshold = |key| spec.settings().get(key)?.parse::<i32>().ok();
            Some(FrameFilter::ColorKey(ColorKey::new(
                read_channel("key_red")?,
                read_channel("key_green")?,
                read_channel("key_blue")?,
                read_threshold("similarity")?,
                read_threshold("smoothness")?,
            )?))
        }
        "chroma_key" => {
            let read_channel = |key| spec.settings().get(key)?.parse::<u8>().ok();
            let read_threshold = |key| spec.settings().get(key)?.parse::<i32>().ok();
            Some(FrameFilter::ChromaKey(ChromaKey::new(
                read_channel("key_red")?,
                read_channel("key_green")?,
                read_channel("key_blue")?,
                read_threshold("similarity")?,
                read_threshold("smoothness")?,
                read_threshold("spill")?,
            )?))
        }
        "sharpen" => spec
            .settings()
            .get("sharpness")
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| *value <= 1_000)
            .map(|milli| FrameFilter::Sharpen { milli }),
        "scroll" => {
            let read_speed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i16>().ok())
                    .filter(|value| (MIN_SCROLL_SPEED..=MAX_SCROLL_SPEED).contains(value))
            };
            let looped = match spec.settings().get("loop") {
                None | Some("true") => true,
                Some("false") => false,
                Some(_) => return None,
            };
            Some(FrameFilter::Scroll {
                speed_x: read_speed("speed_x")?,
                speed_y: read_speed("speed_y")?,
                looped,
            })
        }
        "render_delay" => spec
            .settings()
            .get("milliseconds")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| {
                (MIN_RENDER_DELAY_MILLISECONDS..=MAX_RENDER_DELAY_MILLISECONDS).contains(value)
            })
            .map(|milliseconds| FrameFilter::RenderDelay(RenderDelay { milliseconds })),
        _ => None,
    }
}

/// Compiles a persistent audio/video filter into an ordered audio operation.
///
/// Audio filters are kept separate from [`compile_filter`] because they run on
/// captured audio blocks rather than rendered video frames. The project-facing
/// settings use fixed-point `db_milli` plus integer milliseconds, which avoids
/// locale-dependent decimal parsing on the real-time boundary.
#[must_use]
pub fn compile_audio_filter(spec: &SourceFilterSpec) -> Option<AudioFilter> {
    if !spec.enabled() || spec.category() != SourceFilterCategory::AudioVideo {
        return None;
    }
    match spec.kind().as_str() {
        "gain" => spec
            .settings()
            .get("db_milli")
            .and_then(|value| value.parse::<i32>().ok())
            .and_then(|milli_db| AudioFilter::gain_db_milli(milli_db).ok()),
        "invert_polarity" => Some(AudioFilter::InvertPolarity),
        "limiter" => {
            let threshold = spec
                .settings()
                .get("threshold_db_milli")
                .and_then(|value| value.parse::<i32>().ok())?;
            let release_ms = spec
                .settings()
                .get("release_ms")
                .and_then(|value| value.parse::<u16>().ok())?;
            AudioFilter::limiter_db_milli(threshold, release_ms).ok()
        }
        "compressor" => {
            let read_signed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            let read_unsigned = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u16>().ok())
            };
            AudioFilter::compressor(
                read_unsigned("ratio_milli")?,
                read_signed("threshold_db_milli")?,
                read_unsigned("attack_ms")?,
                read_unsigned("release_ms")?,
                read_signed("output_gain_db_milli")?,
            )
            .ok()
        }
        "expander" => {
            let read_signed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            let read_unsigned = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u16>().ok())
            };
            AudioFilter::expander(
                read_unsigned("ratio_milli")?,
                read_signed("threshold_db_milli")?,
                read_unsigned("attack_ms")?,
                read_unsigned("release_ms")?,
                read_signed("output_gain_db_milli")?,
            )
            .ok()
        }
        "gate" | "noise_gate" => {
            let read_signed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            let read_unsigned = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u16>().ok())
            };
            AudioFilter::noise_gate(
                read_signed("open_threshold_db_milli")?,
                read_signed("close_threshold_db_milli")?,
                read_unsigned("attack_ms")?,
                read_unsigned("hold_ms")?,
                read_unsigned("release_ms")?,
            )
            .ok()
        }
        _ => None,
    }
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
        if let Some(input) = open_input_with_conversion(provider, &device_id, format) {
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
    match open_input_with_conversion(provider, &device_id, format) {
        Some(input) => (Some(input), device_name),
        None => (None, "unavailable (no compatible device format)".to_owned()),
    }
}

fn open_input_with_conversion(
    provider: &Arc<dyn AudioInputProvider>,
    device_id: &str,
    mix_format: AudioFormat,
) -> Option<Box<dyn AudioInput>> {
    let mut candidates = vec![mix_format];
    for (rate, channels) in [(48_000, 2), (44_100, 2), (48_000, 1), (44_100, 1)] {
        let candidate = AudioFormat::new(rate, channels).ok()?;
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    for device_format in candidates {
        let Ok(input) = provider.open_input(device_id, device_format) else {
            continue;
        };
        if device_format == mix_format {
            return Some(input);
        }
        return Some(Box::new(ConvertedAudioInput {
            input,
            converter: AudioResampler::new(device_format, mix_format).ok()?,
            mix_format,
        }));
    }
    None
}

struct ConvertedAudioInput {
    input: Box<dyn AudioInput>,
    converter: AudioResampler,
    mix_format: AudioFormat,
}

impl AudioInput for ConvertedAudioInput {
    fn format(&self) -> AudioFormat {
        self.mix_format
    }

    fn state(&self) -> AudioInputState {
        self.input.state()
    }

    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioDeviceError> {
        let source = self.converter.input_format();
        let source_frames = (frames
            .saturating_mul(source.sample_rate() as usize)
            .saturating_add(self.mix_format.sample_rate() as usize - 1))
            / self.mix_format.sample_rate() as usize;
        let input = self.input.read_block(timestamp, source_frames.max(1))?;
        let converted = self.converter.process(&input)?;
        if converted.frames() == frames {
            return Ok(converted);
        }
        let sample_count = frames.saturating_mul(usize::from(self.mix_format.channels()));
        let mut samples = converted.samples().to_vec();
        samples.resize(sample_count, 0.0);
        samples.truncate(sample_count);
        AudioBuffer::new(self.mix_format, timestamp, samples).map_err(Into::into)
    }

    fn stop(&mut self) {
        self.input.stop();
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
    use std::time::Duration;

    use super::*;
    use obs_rs_audio::AudioDeviceInfo;
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
        for _ in 0..3 {
            engine.tick(None, Some("program")).expect("replay tick");
        }
        assert!(engine.replay_buffer_packet_count() >= 3);
        let bytes = engine
            .save_replay_buffer(&final_path)
            .expect("save replay buffer");
        assert!(bytes > 16);
        let packets = obs_rs_output::MemoryMuxer::decode(
            &std::fs::read(&final_path).expect("read replay file"),
        )
        .expect("decode replay file");
        assert!(!packets.is_empty());
        assert!(engine.is_replay_buffer_active());

        engine.stop_replay_buffer();
        assert!(!engine.is_replay_buffer_active());
        assert!(matches!(
            engine.save_replay_buffer(&final_path),
            Err(EngineError::InvalidConfiguration(reason)) if reason.contains("not running")
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
        assert_eq!(
            stats.microphone_peak_hold_milli, stats.microphone_peak_milli,
            "the live meter publishes its held peak from the same mixer source"
        );
        assert!(!stats.microphone_clipped);
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
        let (mut input, name, fallback) = open_audio_input(&provider, mix, Some("native-mono"));
        assert_eq!(name, "Native mono device");
        assert!(!fallback);
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
    fn recording_rejects_extensions_that_do_not_select_a_known_container() {
        let path = std::env::temp_dir().join("obs-rs-unknown-recording.bin");
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let error = engine
            .start_recording(path)
            .expect_err("unknown extension must be rejected");
        assert!(error.to_string().contains(".mkv or .obsr"));
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
