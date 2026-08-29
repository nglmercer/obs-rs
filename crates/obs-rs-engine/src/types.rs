use std::{error::Error, fmt};

use obs_rs_audio::{AudioBuffer, AudioDeviceError, AudioOutputWorkerSnapshot, AvSyncMetrics};
use obs_rs_clock::TimelineError;
use obs_rs_core::RuntimeError;
use obs_rs_media::{LatencyMetrics, Timestamp, VideoFrame};
use obs_rs_output::{OutputError, StreamMetrics, StreamState};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output_gstreamer::GStreamerError;
use obs_rs_project::{ProjectError, SourceFilterCategory, SourceFilterSpec};

/// Why a persisted filter could not be installed in the current runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterCompileFailure {
    /// The filter belongs to a category handled by another runtime boundary.
    UnsupportedCategory,
    /// No registered implementation knows this filter kind.
    UnsupportedKind,
    /// The implementation exists, but the persisted settings are invalid.
    InvalidSettings,
}

impl FilterCompileFailure {
    /// Returns the stable diagnostic label for this failure.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UnsupportedCategory => "unsupported category",
            Self::UnsupportedKind => "unsupported kind",
            Self::InvalidSettings => "invalid settings",
        }
    }
}

/// Bounded metadata explaining why one persisted filter was unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilterDiagnostic {
    kind: String,
    category: SourceFilterCategory,
    pub(super) failure: FilterCompileFailure,
}

impl FilterDiagnostic {
    pub(super) fn new(spec: &SourceFilterSpec, failure: FilterCompileFailure) -> Self {
        Self {
            kind: spec.kind().as_str().to_owned(),
            category: spec.category(),
            failure,
        }
    }

    /// Returns the persisted filter kind.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns the persisted filter category.
    #[must_use]
    pub const fn category(&self) -> SourceFilterCategory {
        self.category
    }

    /// Returns the reason the filter was not installed.
    #[must_use]
    pub const fn failure(&self) -> FilterCompileFailure {
        self.failure
    }
}

impl fmt::Display for FilterDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "filter '{}' ({}) unavailable: {}",
            self.kind,
            self.category.id(),
            self.failure.label()
        )
    }
}

/// Result of translating a project filter into one runtime filter operation.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterCompilation<T> {
    /// The filter was validated and can be installed.
    Applied(T),
    /// The filter is disabled and therefore intentionally has no operation.
    Ignored,
    /// The filter remains in project data but is unavailable in this runtime.
    Unavailable(FilterDiagnostic),
}

/// A single coordinated media tick.
pub struct EngineTick {
    pub preview_frame: Option<VideoFrame>,
    pub program_frame: Option<VideoFrame>,
    pub audio_blocks: Vec<AudioBuffer>,
    pub timestamp: Timestamp,
    pub audio_peak_milli: u16,
}

/// Monotonic counters and device diagnostics exposed to hosts and GUIs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineStats {
    pub ticks: u64,
    pub video_frames: u64,
    pub audio_blocks: u64,
    pub audio_fallback_blocks: u64,
    pub monitor_blocks_submitted: u64,
    pub monitor_blocks_dropped: u64,
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
    pub av_sync: AvSyncMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineAudioChannel {
    Desktop,
    Microphone,
}

/// Lifecycle of one output, recording or streaming.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputLifecycle {
    #[default]
    Idle,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// A streaming lifecycle transition published by the engine worker.
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
    #[must_use]
    pub const fn is_stopped(self) -> bool {
        matches!(self, Self::Idle | Self::Failed)
    }

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesktopAudioSource {
    Monitor(String),
    Silent(String),
}

impl DesktopAudioSource {
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Monitor(label) | Self::Silent(label) => label,
        }
    }

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
    pub recording_lifecycle: OutputLifecycle,
    pub streaming_lifecycle: OutputLifecycle,
    pub replay_lifecycle: OutputLifecycle,
    pub replay_save_status: ReplaySaveStatus,
    pub replay_buffer_packets: usize,
    pub stream_state: Option<StreamState>,
    pub audio_backend: String,
    pub audio_fallback: bool,
    /// Runtime identity of the device currently feeding the microphone.
    pub audio_active_device_id: Option<String>,
    pub desktop_audio: DesktopAudioSource,
    /// Runtime identity of the playback route currently feeding desktop audio.
    pub desktop_audio_active_device_id: Option<String>,
    pub monitor_output: Option<AudioOutputWorkerSnapshot>,
    pub filter_diagnostics: Vec<String>,
    pub stream_metrics: Option<StreamMetrics>,
    pub production_stream_metrics: Option<ProductionStreamMetrics>,
    pub stream_queued_bytes: usize,
    pub last_error: Option<String>,
    pub stats: EngineStats,
}

/// Status of the latest replay-buffer save request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplaySaveStatus {
    Idle,
    Saving,
    Saved { bytes: usize },
    Failed { reason: String },
}

/// Protocol-independent telemetry copied from a native production adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProductionStreamMetrics {
    pub video_submitted: u64,
    pub audio_submitted: u64,
    pub dropped: u64,
    pub reconnects: u64,
    pub video_queue_bytes: u64,
    pub audio_queue_bytes: u64,
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
