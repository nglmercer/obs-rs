//! Persistent application settings behind the OBS-style settings window.
//!
//! Everything the settings window edits is stored in one validated
//! [`obs_rs_config::Config`] document, so persistence is the same flat TOML
//! format the rest of OBS-RS uses and a malformed file degrades to defaults
//! rather than failing startup.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use obs_rs_audio::{AudioChannelLayout, AudioMonitorMode, MAX_AUDIO_SYNC_OFFSET_MILLISECONDS};
use obs_rs_config::Config;
use obs_rs_media::{ScaleFilter, VideoFormat};
use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, EncoderImplementation, EncoderPreset, HlsConfig, RateControl,
    RistConfig, RtmpConfig, SecretString, SrtConfig, SrtKeyLength, SrtMode, StreamProtocol,
    StreamTarget, VideoCodec, VideoEncoderConfig, WhipConfig,
};
use obs_rs_ui::{ProjectSceneSelection, Shortcut, UiAction, UiError, UiLocale};
use slint::{Brush, Color, Model, ModelRc, VecModel};

use crate::dock_tree::{DockNode, DOCK_IDS};
use crate::settings_model::{
    metrics, FpsMode, OutputMode, RecordingQuality, UiDensity, UiStyle, VideoSettings,
    DEFAULT_FONT_SIZE, FONT_SIZE_RANGE, MAX_DIMENSION,
};
use crate::{ThemeTokens, UiMetrics};

/// File name the settings document is read from and written to.
const SETTINGS_FILE: &str = "obs-rs-settings.toml";
/// Default file names inside the per-user directory.
const PROJECT_FILE: &str = "obs-rs-project.json";
const DIAGNOSTICS_FILE: &str = "obs-rs-diagnostics.obsrdg";
const PROJECT_SCENE_SELECTIONS_KEY: &str = "project_scene_selections";
const MAX_PERSISTED_PROJECT_SCENE_SELECTIONS: usize = 16;
const MAX_PERSISTED_SELECTION_KEY_BYTES: usize = 384;

/// Whether the first-run setup should be shown at startup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SetupState {
    /// The user has not completed or explicitly skipped setup.
    #[default]
    Pending,
    /// Setup was applied successfully.
    Completed,
    /// The user chose to continue without setup.
    Skipped,
}

impl SetupState {
    const fn id(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Skipped => "skipped",
        }
    }

    fn from_id(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

/// Result of loading settings with the startup-only first-run signal.
pub(crate) struct SettingsLoad {
    pub(crate) settings: AppSettings,
    pub(crate) show_setup: bool,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RecordingFormat {
    #[default]
    Matroska,
    Mp4,
    FragmentedMp4,
    Mov,
    Flv,
    ReferencePacket,
}

impl RecordingFormat {
    pub(crate) const ALL: [Self; 6] = [
        Self::Matroska,
        Self::Mp4,
        Self::FragmentedMp4,
        Self::Mov,
        Self::Flv,
        Self::ReferencePacket,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::Mp4 => "mp4",
            Self::FragmentedMp4 => "fragmented-mp4",
            Self::Mov => "mov",
            Self::Flv => "flv",
            Self::ReferencePacket => "obsr-packet",
        }
    }

    fn from_id(value: &str) -> Option<Self> {
        match value {
            "matroska" | "mkv" => Some(Self::Matroska),
            "mp4" => Some(Self::Mp4),
            "fragmented-mp4" => Some(Self::FragmentedMp4),
            "mov" => Some(Self::Mov),
            "flv" => Some(Self::Flv),
            "obsr-packet" | "obsr" => Some(Self::ReferencePacket),
            _ => None,
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Matroska => "mkv",
            Self::Mp4 | Self::FragmentedMp4 => "mp4",
            Self::Mov => "mov",
            Self::Flv => "flv",
            Self::ReferencePacket => "obsr",
        }
    }

    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Matroska => "Matroska (.mkv)",
            Self::Mp4 => "MPEG-4 (.mp4)",
            Self::FragmentedMp4 => "Fragmented MPEG-4 (.mp4)",
            Self::Mov => "QuickTime Movie (.mov)",
            Self::Flv => "Flash Video (.flv)",
            Self::ReferencePacket => "OBS-RS Packet (.obsr)",
        }
    }
}

/// Returns the per-user directory OBS-RS keeps its documents in.
///
/// Storing them beside the working directory meant a session launched from
/// another directory looked like it had lost every setting, so the default is
/// the XDG config directory. A file that already exists in the working
/// directory still wins, which keeps existing installs working unchanged.
pub(crate) fn user_directory() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let directory = base.join("obs-rs");
    std::fs::create_dir_all(&directory).ok()?;
    Some(directory)
}

/// Returns the path the settings document lives at.
pub(crate) fn settings_path() -> PathBuf {
    PathBuf::from(user_file(SETTINGS_FILE))
}

/// Resolves `name` to the legacy working-directory file when one exists, and
/// to the per-user directory otherwise.
pub(crate) fn user_file(name: &str) -> String {
    if Path::new(name).exists() {
        return name.to_owned();
    }
    user_directory().map_or_else(
        || name.to_owned(),
        |directory| directory.join(name).to_string_lossy().into_owned(),
    )
}

/// Canvas sizes offered on the Video page, in OBS's descending order.
pub(crate) const RESOLUTIONS: [(u32, u32); 6] = [
    (1920, 1080),
    (1600, 900),
    (1280, 720),
    (1024, 576),
    (854, 480),
    (640, 360),
];

/// Frame rates offered on the Video page as `(numerator, denominator)`.
pub(crate) const FRAME_RATES: [(u32, u32); 6] = [
    (60, 1),
    (60_000, 1_001),
    (50, 1),
    (30, 1),
    (30_000, 1_001),
    (24, 1),
];

/// Sample rates offered on the Audio page.
pub(crate) const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];

/// Channel layouts offered on the Audio page.
pub(crate) const CHANNEL_LAYOUTS: [AudioChannelLayout; 6] = [
    AudioChannelLayout::Stereo,
    AudioChannelLayout::Mono,
    AudioChannelLayout::TwoPointOne,
    AudioChannelLayout::Quad,
    AudioChannelLayout::FivePointOne,
    AudioChannelLayout::SevenPointOne,
];

/// Per-channel positive audio sync offsets accepted by the Audio page.
pub(crate) const AUDIO_SYNC_OFFSET_RANGE: std::ops::RangeInclusive<u32> =
    0..=MAX_AUDIO_SYNC_OFFSET_MILLISECONDS;

/// Default per-channel audio sync offset.
pub(crate) const AUDIO_SYNC_OFFSET_DEFAULT: u32 = 0;

/// Monitor routing choices exposed by the Audio settings page.
pub(crate) const AUDIO_MONITOR_MODES: [AudioMonitorMode; 3] = [
    AudioMonitorMode::Off,
    AudioMonitorMode::MonitorOnly,
    AudioMonitorMode::MonitorAndOutput,
];

/// Stable settings identifier for one monitor-routing choice.
pub(crate) const fn audio_monitor_mode_id(mode: AudioMonitorMode) -> &'static str {
    match mode {
        AudioMonitorMode::Off => "off",
        AudioMonitorMode::MonitorOnly => "monitor_only",
        AudioMonitorMode::MonitorAndOutput => "monitor_and_output",
    }
}

/// Parses a persisted monitor-routing choice, falling back per field when it
/// is unknown or from a future version.
pub(crate) fn audio_monitor_mode_from_id(value: &str) -> Option<AudioMonitorMode> {
    match value {
        "off" => Some(AudioMonitorMode::Off),
        "monitor_only" => Some(AudioMonitorMode::MonitorOnly),
        "monitor_and_output" => Some(AudioMonitorMode::MonitorAndOutput),
        _ => None,
    }
}

/// The smallest and largest canvas-space distance accepted by source snapping.
///
/// OBS stores this as a pixel distance in its `BasicWindow` configuration. The
/// Rust settings document keeps the same user-facing unit while bounding the
/// value before it reaches the interactive geometry path.
pub(crate) const CANVAS_SNAP_DISTANCE_RANGE: std::ops::RangeInclusive<u16> = 1..=100;

/// The default source-snapping distance used by the canvas and settings page.
pub(crate) const CANVAS_SNAP_DISTANCE_DEFAULT: u16 = 10;

/// The replay history duration accepted by the Output page, in seconds.
pub(crate) const REPLAY_BUFFER_DURATION_RANGE: std::ops::RangeInclusive<u32> = 1..=3_600;

/// The default replay history duration shown by the Output page.
pub(crate) const REPLAY_BUFFER_DURATION_DEFAULT: u32 = 20;

/// The replay history byte budget accepted by the Output page, in MiB.
pub(crate) const REPLAY_BUFFER_CAPACITY_MIB_RANGE: std::ops::RangeInclusive<u32> = 1..=256;

/// The default replay history byte budget shown by the Output page, in MiB.
pub(crate) const REPLAY_BUFFER_CAPACITY_MIB_DEFAULT: u32 = 64;

/// The split-recording target duration accepted by the Output page, in minutes.
pub(crate) const RECORDING_SPLIT_DURATION_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 1..=1_440;

/// The default split-recording target duration shown by the Output page.
pub(crate) const RECORDING_SPLIT_DURATION_MINUTES_DEFAULT: u32 = 60;

/// The split-recording target size accepted by the Output page, in MiB.
pub(crate) const RECORDING_SPLIT_SIZE_MIB_RANGE: std::ops::RangeInclusive<u32> = 1..=256;

/// The default split-recording target size shown by the Output page, in MiB.
pub(crate) const RECORDING_SPLIT_SIZE_MIB_DEFAULT: u32 = 64;

/// The maximum split-recording segment count accepted by the Output page.
pub(crate) const RECORDING_SPLIT_SEGMENTS_RANGE: std::ops::RangeInclusive<u32> = 1..=1_024;

/// The default maximum split-recording segment count shown by the Output page.
pub(crate) const RECORDING_SPLIT_SEGMENTS_DEFAULT: u32 = 64;

/// An sRGB colour as `[red, green, blue]`.
type Rgb = [u8; 3];

/// One named colour scheme for the whole application.
pub(crate) struct ThemePreset {
    pub(crate) key: &'static str,
    window_bg: Rgb,
    panel_bg: Rgb,
    header_bg: Rgb,
    header_active_bg: Rgb,
    border: Rgb,
    border_strong: Rgb,
    row_bg: Rgb,
    row_selected_bg: Rgb,
    control_bg: Rgb,
    text: Rgb,
    text_strong: Rgb,
    text_muted: Rgb,
    accent: Rgb,
    canvas_bg: Rgb,
}

/// The themes offered on the Appearance page, in display order.
pub(crate) const THEMES: [ThemePreset; 4] = [
    ThemePreset {
        key: "dark",
        window_bg: [0x18, 0x1A, 0x1F],
        panel_bg: [0x1F, 0x22, 0x29],
        header_bg: [0x2B, 0x2F, 0x38],
        header_active_bg: [0x3A, 0x3F, 0x4A],
        border: [0x37, 0x3C, 0x45],
        border_strong: [0x4B, 0x55, 0x63],
        row_bg: [0x27, 0x2B, 0x32],
        row_selected_bg: [0x33, 0x49, 0x69],
        control_bg: [0x27, 0x34, 0x49],
        text: [0xE2, 0xE8, 0xF0],
        text_strong: [0xF9, 0xFA, 0xFB],
        text_muted: [0x94, 0xA3, 0xB8],
        accent: [0x3B, 0x82, 0xF6],
        canvas_bg: [0x00, 0x00, 0x00],
    },
    ThemePreset {
        key: "darker",
        window_bg: [0x0D, 0x0E, 0x11],
        panel_bg: [0x14, 0x16, 0x1A],
        header_bg: [0x1D, 0x20, 0x25],
        header_active_bg: [0x2A, 0x2E, 0x35],
        border: [0x25, 0x29, 0x30],
        border_strong: [0x3A, 0x40, 0x4A],
        row_bg: [0x1A, 0x1D, 0x22],
        row_selected_bg: [0x25, 0x3A, 0x57],
        control_bg: [0x1B, 0x25, 0x36],
        text: [0xD8, 0xDE, 0xE8],
        text_strong: [0xF4, 0xF6, 0xF9],
        text_muted: [0x7C, 0x8A, 0x9E],
        accent: [0x2F, 0x6F, 0xE0],
        canvas_bg: [0x00, 0x00, 0x00],
    },
    ThemePreset {
        key: "midnight",
        window_bg: [0x10, 0x14, 0x22],
        panel_bg: [0x17, 0x1D, 0x30],
        header_bg: [0x20, 0x28, 0x40],
        header_active_bg: [0x2C, 0x36, 0x53],
        border: [0x2A, 0x33, 0x4D],
        border_strong: [0x41, 0x4E, 0x74],
        row_bg: [0x1C, 0x23, 0x39],
        row_selected_bg: [0x2E, 0x42, 0x74],
        control_bg: [0x22, 0x2D, 0x4C],
        text: [0xDF, 0xE5, 0xF5],
        text_strong: [0xFA, 0xFB, 0xFF],
        text_muted: [0x8D, 0x98, 0xBC],
        accent: [0x5B, 0x74, 0xF0],
        canvas_bg: [0x00, 0x00, 0x00],
    },
    ThemePreset {
        key: "slate",
        window_bg: [0x22, 0x26, 0x2B],
        panel_bg: [0x2C, 0x31, 0x38],
        header_bg: [0x39, 0x3F, 0x48],
        header_active_bg: [0x48, 0x50, 0x5B],
        border: [0x45, 0x4C, 0x56],
        border_strong: [0x5C, 0x66, 0x74],
        row_bg: [0x33, 0x38, 0x40],
        row_selected_bg: [0x3F, 0x57, 0x7A],
        control_bg: [0x34, 0x42, 0x58],
        text: [0xE7, 0xEB, 0xF1],
        text_strong: [0xFF, 0xFF, 0xFF],
        text_muted: [0xA3, 0xAF, 0xC0],
        accent: [0x4C, 0x8D, 0xF0],
        canvas_bg: [0x00, 0x00, 0x00],
    },
];

/// Everything the settings window persists between sessions.
///
/// The independent booleans mirror the independent checkboxes on the General
/// page; grouping them into a flags type would only add a translation layer.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AppSettings {
    pub(crate) locale: String,
    pub(crate) theme: usize,
    pub(crate) style: UiStyle,
    pub(crate) font_size: u8,
    pub(crate) density: UiDensity,
    pub(crate) confirm_start_stream: bool,
    pub(crate) confirm_stop_stream: bool,
    pub(crate) confirm_stop_recording: bool,
    pub(crate) auto_record_when_streaming: bool,
    /// Canvas pixels within which a moving or resizing item snaps to a guide.
    pub(crate) canvas_snap_distance: u16,
    /// Draw the OBS-style action, graphics, and 4:3 safe-area guides in preview.
    pub(crate) show_safe_areas: bool,
    pub(crate) sample_rate: usize,
    pub(crate) channels: usize,
    pub(crate) hotkey_swap: String,
    pub(crate) hotkey_start_recording: String,
    pub(crate) hotkey_stop_recording: String,
    pub(crate) hotkey_start_streaming: String,
    pub(crate) hotkey_stop_streaming: String,
    pub(crate) hotkey_undo: String,
    pub(crate) hotkey_redo: String,
    pub(crate) hotkey_save_project: String,
    pub(crate) hotkey_fade_transition: String,
    pub(crate) hotkey_save_replay: String,
    pub(crate) hotkey_start_replay: String,
    pub(crate) hotkey_stop_replay: String,
    pub(crate) preview_border_color: String,
    pub(crate) program_border_color: String,
    pub(crate) project_path: String,
    pub(crate) diagnostics_path: String,
    pub(crate) recording_path: String,
    /// Directory new recordings are written into, as the Output page shows it.
    pub(crate) recording_directory: String,
    /// Generate recording file names with `-` instead of spaces.
    pub(crate) recording_filename_without_spaces: bool,
    pub(crate) recording_quality: RecordingQuality,
    pub(crate) recording_format: RecordingFormat,
    /// Publish Matroska recordings as MP4 after the native bounded remux step.
    pub(crate) recording_auto_remux: bool,
    pub(crate) recording_codec: VideoCodec,
    pub(crate) recording_audio_encoder: EncoderImplementation,
    /// Maximum wall-clock history retained by the replay buffer, in seconds.
    pub(crate) replay_buffer_duration_seconds: u32,
    /// Maximum encoded replay history retained, in mebibytes.
    pub(crate) replay_buffer_capacity_mib: u32,
    /// Publish bounded numbered segments instead of one recording file when
    /// the effective format has a supported split-muxer boundary.
    pub(crate) recording_split_enabled: bool,
    /// Target wall-clock duration for one split segment, in minutes.
    pub(crate) recording_split_duration_minutes: u32,
    /// Target encoded size for one split segment, in mebibytes.
    pub(crate) recording_split_size_mib: u32,
    /// Maximum number of numbered split segments kept in one recording.
    pub(crate) recording_split_max_segments: u32,
    pub(crate) output_mode: OutputMode,
    /// Show the detailed encoder controls inside Simple output mode.
    pub(crate) stream_custom_encoder: bool,
    pub(crate) video: VideoSettings,
    pub(crate) stream_protocol: StreamProtocol,
    pub(crate) rtmp: RtmpConfig,
    pub(crate) srt: SrtConfig,
    pub(crate) whip_endpoint: String,
    pub(crate) whip_bearer_token: Option<SecretString>,
    pub(crate) hls: HlsConfig,
    pub(crate) rist: RistConfig,
    pub(crate) reference_address: String,
    /// Provider-stable audio input ID; empty selects the provider-declared
    /// default input and keeps the deterministic fallback as a safe last
    /// resort.
    pub(crate) audio_input_id: String,
    /// Provider-stable local monitor-output ID; empty disables local playback.
    pub(crate) audio_monitor_output_id: String,
    /// Monitor destination policy for the microphone channel.
    pub(crate) microphone_monitor_mode: AudioMonitorMode,
    /// Monitor destination policy for the desktop-audio channel.
    pub(crate) desktop_audio_monitor_mode: AudioMonitorMode,
    /// Positive, sample-quantized delay applied to the microphone channel.
    pub(crate) audio_input_sync_offset_millis: u32,
    /// Positive, sample-quantized delay applied to the desktop-audio channel.
    pub(crate) desktop_audio_sync_offset_millis: u32,
    /// Last Preview scene selected in the desktop session; empty means use the
    /// first scene after a project is restored.
    pub(crate) last_preview_scene: String,
    /// Last Program scene selected in the desktop session; empty means use the
    /// first scene after a project is restored.
    pub(crate) last_program_scene: String,
    /// Bounded per-document Preview/Program choices restored across sessions.
    pub(crate) project_scene_selections: Vec<ProjectSceneSelection>,
    /// Reopen the project file from [`AppSettings::project_path`] at startup.
    pub(crate) restore_project: bool,
    /// Write the project back to the same file when the window closes.
    pub(crate) save_project_on_exit: bool,
    /// Controls the blocking setup wizard shown for a new installation.
    pub(crate) setup_state: SetupState,
    /// Bounded summary of the last local setup benchmark for diagnostics.
    pub(crate) setup_benchmark_summary: String,
    /// The dock layout the window was last left in.
    pub(crate) layout: LayoutSettings,
}

/// Window layout state, restored so a session reopens where it was left.
///
/// This is the desktop's own state rather than project data, which is why it
/// belongs to the settings document instead of the project file. Width shares
/// are floats, so the type compares by value rather than deriving `Eq`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutSettings {
    /// Legacy projection of the tree's dock IDs: 0 scenes, 1 sources, 2 mixer,
    /// 3 transitions, 4 controls. New layout code must mutate `dock_tree` and
    /// refresh this projection at the toolkit boundary.
    pub(crate) panel_order: Vec<i32>,
    pub(crate) show_scenes: bool,
    pub(crate) show_sources: bool,
    pub(crate) show_mixer: bool,
    pub(crate) show_transitions: bool,
    pub(crate) show_controls: bool,
    /// 0 is studio mode, 1 the single-canvas default, and 2 is multiview.
    pub(crate) view_mode: i32,
    /// Height of the dock row in logical pixels.
    pub(crate) dock_height: u32,
    /// Legacy width shares per dock kind, as adjusted by the row splitter
    /// adapter. Tree-native layouts retain this for old settings readers.
    pub(crate) panel_weights: Vec<f32>,
    /// Dock kinds that were left detached in their own windows.
    pub(crate) floating_panels: Vec<i32>,
    /// Last known physical desktop geometry for detached docks. The scale is
    /// retained so a window can keep its logical size when it is restored on
    /// a display with a different DPI.
    pub(crate) floating_geometry: Vec<FloatingGeometry>,
    /// Last known physical desktop geometry for windowed projector feeds.
    /// Fullscreen projectors deliberately keep no geometry of their own.
    pub(crate) projector_geometry: Vec<ProjectorGeometry>,
    /// Versioned tree representation of the dock arrangement.
    pub(crate) dock_tree: DockNode,
}

/// Bounded geometry for one detached dock window.
///
/// Positions and dimensions use the windowing backend's physical pixel space,
/// which is the only space that is stable across multi-monitor desktops. A
/// saved scale factor lets restore adjust the dimensions without silently
/// turning a 320 logical-pixel dock into a tiny window after a DPI change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FloatingGeometry {
    pub(crate) panel: i32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_milli: u32,
}

impl FloatingGeometry {
    pub(crate) const MIN_POSITION: i32 = -2_000_000;
    pub(crate) const MAX_POSITION: i32 = 2_000_000;
    pub(crate) const MIN_WIDTH: u32 = 240;
    pub(crate) const MAX_WIDTH: u32 = 8_192;
    pub(crate) const MIN_HEIGHT: u32 = 160;
    pub(crate) const MAX_HEIGHT: u32 = 8_192;
    pub(crate) const MIN_SCALE_MILLI: u32 = 500;
    pub(crate) const MAX_SCALE_MILLI: u32 = 4_000;

    /// Creates a geometry record only when every value is safe to restore.
    pub(crate) fn new(
        panel: i32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale_milli: u32,
    ) -> Option<Self> {
        if !DEFAULT_PANEL_ORDER.contains(&panel)
            || !(Self::MIN_POSITION..=Self::MAX_POSITION).contains(&x)
            || !(Self::MIN_POSITION..=Self::MAX_POSITION).contains(&y)
            || !(Self::MIN_WIDTH..=Self::MAX_WIDTH).contains(&width)
            || !(Self::MIN_HEIGHT..=Self::MAX_HEIGHT).contains(&height)
            || !(Self::MIN_SCALE_MILLI..=Self::MAX_SCALE_MILLI).contains(&scale_milli)
        {
            return None;
        }
        Some(Self {
            panel,
            x,
            y,
            width,
            height,
            scale_milli,
        })
    }
}

/// The stable IDs used by the projector settings record.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ProjectorKind {
    Program,
    Preview,
    Multiview,
    Source,
    Scene,
}

impl ProjectorKind {
    pub(crate) const ALL: [Self; 5] = [
        Self::Program,
        Self::Preview,
        Self::Multiview,
        Self::Source,
        Self::Scene,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Preview => "preview",
            Self::Multiview => "multiview",
            Self::Source => "source",
            Self::Scene => "scene",
        }
    }

    fn from_id(value: &str) -> Option<Self> {
        match value {
            "program" => Some(Self::Program),
            "preview" => Some(Self::Preview),
            "multiview" => Some(Self::Multiview),
            "source" => Some(Self::Source),
            "scene" => Some(Self::Scene),
            _ => None,
        }
    }
}

/// Bounded geometry and display state for one projector feed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProjectorGeometry {
    pub(crate) projector: ProjectorKind,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_milli: u32,
    pub(crate) fullscreen: bool,
}

impl ProjectorGeometry {
    /// Creates a geometry record only when every value is safe to restore.
    pub(crate) fn new(
        projector: ProjectorKind,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale_milli: u32,
    ) -> Option<Self> {
        if !(FloatingGeometry::MIN_POSITION..=FloatingGeometry::MAX_POSITION).contains(&x)
            || !(FloatingGeometry::MIN_POSITION..=FloatingGeometry::MAX_POSITION).contains(&y)
            || !(FloatingGeometry::MIN_WIDTH..=FloatingGeometry::MAX_WIDTH).contains(&width)
            || !(FloatingGeometry::MIN_HEIGHT..=FloatingGeometry::MAX_HEIGHT).contains(&height)
            || !(FloatingGeometry::MIN_SCALE_MILLI..=FloatingGeometry::MAX_SCALE_MILLI)
                .contains(&scale_milli)
        {
            return None;
        }
        Some(Self {
            projector,
            x,
            y,
            width,
            height,
            scale_milli,
            fullscreen: false,
        })
    }

    pub(crate) const fn with_fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }
}

/// Scales a bounded physical dimension when a window is restored on another
/// DPI, keeping the result inside the same safe window-size range.
pub(crate) fn scale_window_dimension(value: u32, ratio: f32, minimum: u32, maximum: u32) -> u32 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "dimensions are bounded by the geometry record before scaling"
    )]
    let scaled = (value as f32 * ratio).round() as u32;
    scaled.clamp(minimum, maximum)
}

/// The dock IDs a layout must contain, in the order OBS ships them.
const DEFAULT_PANEL_ORDER: [i32; 5] = [1, 0, 2, 3, 4];

/// Relative dock widths OBS ships with: the mixer is the widest strip and the
/// controls column the narrowest.
const DEFAULT_PANEL_WEIGHTS: [f32; 5] = [1.0, 1.0, 1.85, 1.0, 1.4];

/// Bounds a stored width share must lie inside to be used.
const WEIGHT_RANGE: std::ops::RangeInclusive<f32> = 0.2..=8.0;

impl Default for LayoutSettings {
    fn default() -> Self {
        let panel_order = DEFAULT_PANEL_ORDER.to_vec();
        let panel_weights = DEFAULT_PANEL_WEIGHTS.to_vec();
        let dock_tree = DockNode::from_legacy(&panel_order, &panel_weights)
            .expect("the built-in dock layout must be valid");
        Self {
            panel_order,
            show_scenes: true,
            show_sources: true,
            show_mixer: true,
            show_transitions: true,
            show_controls: true,
            view_mode: 1,
            dock_height: 248,
            panel_weights,
            floating_panels: Vec::new(),
            floating_geometry: Vec::new(),
            projector_geometry: Vec::new(),
            dock_tree,
        }
    }
}

impl LayoutSettings {
    /// Parses `1,0,2,3,4` into a complete dock order.
    ///
    /// A document that names a dock twice, omits one, or contains an unknown ID
    /// is rejected wholesale: a partial layout would hide docks with no way for
    /// the user to tell why.
    fn parse_panel_order(value: &str) -> Option<Vec<i32>> {
        let order = value
            .split(',')
            .map(|entry| entry.trim().parse::<i32>().ok())
            .collect::<Option<Vec<_>>>()?;
        let mut sorted = order.clone();
        sorted.sort_unstable();
        (sorted == [0, 1, 2, 3, 4]).then_some(order)
    }

    /// Parses `1.0,1.0,1.85,1.0,1.4` into one share per dock.
    ///
    /// A document with the wrong count, or a share outside the range a splitter
    /// can produce, falls back wholesale rather than leaving a dock unusable.
    fn parse_panel_weights(value: &str) -> Option<Vec<f32>> {
        let weights = value
            .split(',')
            .map(|entry| entry.trim().parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        (weights.len() == DEFAULT_PANEL_WEIGHTS.len()
            && weights
                .iter()
                .all(|weight| weight.is_finite() && WEIGHT_RANGE.contains(weight)))
        .then_some(weights)
    }

    /// Parses the comma-separated list of detached dock IDs.
    fn parse_floating(value: &str) -> Vec<i32> {
        let mut panels = value
            .split(',')
            .filter_map(|entry| entry.trim().parse::<i32>().ok())
            .filter(|panel| DEFAULT_PANEL_ORDER.contains(panel))
            .collect::<Vec<_>>();
        panels.sort_unstable();
        panels.dedup();
        panels
    }

    fn panel_weights_text(&self) -> String {
        self.panel_weights
            .iter()
            .map(|weight| format!("{weight:.3}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn floating_text(&self) -> String {
        self.floating_panels
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Parses the versioned `v1:panel:x:y:width:height:scale;...` geometry
    /// list. Individual bad records are discarded so one unplugged monitor
    /// cannot destroy the positions of every other detached dock.
    fn parse_floating_geometry(value: &str) -> Vec<FloatingGeometry> {
        let Some(records) = value.strip_prefix("v1:") else {
            return Vec::new();
        };
        let mut geometry: Vec<FloatingGeometry> = Vec::new();
        for record in records.split(';').filter(|record| !record.is_empty()) {
            let fields = record.split(':').collect::<Vec<_>>();
            if fields.len() != 6 {
                continue;
            }
            let [panel, x, y, width, height, scale] = [
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
            ];
            let (Some(panel), Some(x), Some(y), Some(width), Some(height), Some(scale)) = (
                panel.parse().ok(),
                x.parse().ok(),
                y.parse().ok(),
                width.parse().ok(),
                height.parse().ok(),
                scale.parse().ok(),
            ) else {
                continue;
            };
            let Some(entry) = FloatingGeometry::new(panel, x, y, width, height, scale) else {
                continue;
            };
            if geometry.iter().all(|other| other.panel != entry.panel)
                && geometry.len() < DEFAULT_PANEL_ORDER.len()
            {
                geometry.push(entry);
            }
        }
        geometry.sort_unstable_by_key(|entry| entry.panel);
        geometry
    }

    /// Parses the versioned projector geometry list. Version one records did
    /// not carry fullscreen state and therefore restore as windowed; version
    /// two adds one bounded `0`/`1` field without invalidating old settings.
    fn parse_projector_geometry(value: &str) -> Vec<ProjectorGeometry> {
        let (version, records) = if let Some(records) = value.strip_prefix("v2:") {
            (2_u8, records)
        } else if let Some(records) = value.strip_prefix("v1:") {
            (1_u8, records)
        } else {
            return Vec::new();
        };
        let mut geometry: Vec<ProjectorGeometry> = Vec::new();
        for record in records.split(';').filter(|record| !record.is_empty()) {
            let fields = record.split(':').collect::<Vec<_>>();
            if (version == 1 && fields.len() != 6) || (version == 2 && fields.len() != 7) {
                continue;
            }
            let [projector, x, y, width, height, scale] = [
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
            ];
            let (Some(projector), Some(x), Some(y), Some(width), Some(height), Some(scale)) = (
                ProjectorKind::from_id(projector),
                x.parse().ok(),
                y.parse().ok(),
                width.parse().ok(),
                height.parse().ok(),
                scale.parse().ok(),
            ) else {
                continue;
            };
            let fullscreen = match version {
                1 => false,
                2 => match fields[6] {
                    "0" => false,
                    "1" => true,
                    _ => continue,
                },
                _ => continue,
            };
            let Some(entry) = ProjectorGeometry::new(projector, x, y, width, height, scale)
                .map(|entry| entry.with_fullscreen(fullscreen))
            else {
                continue;
            };
            if geometry
                .iter()
                .all(|other| other.projector != entry.projector)
                && geometry.len() < ProjectorKind::ALL.len()
            {
                geometry.push(entry);
            }
        }
        geometry.sort_unstable_by_key(|entry| entry.projector);
        geometry
    }

    fn floating_geometry_text(&self) -> String {
        let mut geometry = self.floating_geometry.clone();
        geometry.sort_unstable_by_key(|entry| entry.panel);
        let records = geometry
            .into_iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}:{}:{}",
                    entry.panel, entry.x, entry.y, entry.width, entry.height, entry.scale_milli
                )
            })
            .collect::<Vec<_>>();
        format!("v1:{}", records.join(";"))
    }

    fn projector_geometry_text(&self) -> String {
        let mut geometry = self.projector_geometry.clone();
        geometry.sort_unstable_by_key(|entry| entry.projector);
        let records = geometry
            .into_iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}:{}:{}:{}",
                    entry.projector.id(),
                    entry.x,
                    entry.y,
                    entry.width,
                    entry.height,
                    entry.scale_milli,
                    u8::from(entry.fullscreen),
                )
            })
            .collect::<Vec<_>>();
        format!("v2:{}", records.join(";"))
    }

    fn panel_order_text(&self) -> String {
        self.dock_tree
            .leaf_order()
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            locale: "en".to_owned(),
            theme: 0,
            style: UiStyle::default(),
            font_size: DEFAULT_FONT_SIZE,
            density: UiDensity::default(),
            confirm_start_stream: false,
            confirm_stop_stream: true,
            confirm_stop_recording: true,
            auto_record_when_streaming: false,
            canvas_snap_distance: CANVAS_SNAP_DISTANCE_DEFAULT,
            show_safe_areas: false,
            // 48 kHz stereo, matching the mixer the desktop state starts with.
            sample_rate: 1,
            channels: 0,
            hotkey_swap: "Space".to_owned(),
            hotkey_start_recording: "Ctrl+R".to_owned(),
            hotkey_stop_recording: "Ctrl+Shift+R".to_owned(),
            hotkey_start_streaming: "Ctrl+B".to_owned(),
            hotkey_stop_streaming: "Ctrl+Shift+B".to_owned(),
            hotkey_undo: "Ctrl+Z".to_owned(),
            hotkey_redo: "Ctrl+Y".to_owned(),
            hotkey_save_project: "Ctrl+S".to_owned(),
            hotkey_fade_transition: "Ctrl+Shift+F".to_owned(),
            hotkey_save_replay: "F8".to_owned(),
            hotkey_start_replay: "Ctrl+Shift+F8".to_owned(),
            hotkey_stop_replay: "Ctrl+Alt+F8".to_owned(),
            preview_border_color: "#60A5FA".to_owned(),
            program_border_color: "#F87171".to_owned(),
            project_path: user_file(PROJECT_FILE),
            diagnostics_path: user_file(DIAGNOSTICS_FILE),
            recording_path: user_file("obs-rs-recording.mkv"),
            recording_directory: default_recording_directory(),
            recording_filename_without_spaces: false,
            recording_quality: RecordingQuality::default(),
            recording_format: RecordingFormat::Matroska,
            recording_auto_remux: false,
            recording_codec: VideoCodec::H264,
            recording_audio_encoder: EncoderImplementation::default(),
            replay_buffer_duration_seconds: REPLAY_BUFFER_DURATION_DEFAULT,
            replay_buffer_capacity_mib: REPLAY_BUFFER_CAPACITY_MIB_DEFAULT,
            recording_split_enabled: false,
            recording_split_duration_minutes: RECORDING_SPLIT_DURATION_MINUTES_DEFAULT,
            recording_split_size_mib: RECORDING_SPLIT_SIZE_MIB_DEFAULT,
            recording_split_max_segments: RECORDING_SPLIT_SEGMENTS_DEFAULT,
            output_mode: OutputMode::default(),
            stream_custom_encoder: false,
            video: VideoSettings::default(),
            stream_protocol: StreamProtocol::Rtmp,
            rtmp: RtmpConfig::default(),
            srt: SrtConfig::default(),
            whip_endpoint: "https://127.0.0.1/whip".to_owned(),
            whip_bearer_token: None,
            hls: HlsConfig::default(),
            rist: RistConfig::default(),
            reference_address: "127.0.0.1:9000".to_owned(),
            audio_input_id: String::new(),
            audio_monitor_output_id: String::new(),
            microphone_monitor_mode: AudioMonitorMode::Off,
            desktop_audio_monitor_mode: AudioMonitorMode::Off,
            audio_input_sync_offset_millis: AUDIO_SYNC_OFFSET_DEFAULT,
            desktop_audio_sync_offset_millis: AUDIO_SYNC_OFFSET_DEFAULT,
            last_preview_scene: String::new(),
            last_program_scene: String::new(),
            project_scene_selections: Vec::new(),
            restore_project: true,
            save_project_on_exit: true,
            setup_state: SetupState::Pending,
            setup_benchmark_summary: String::new(),
            layout: LayoutSettings::default(),
        }
    }
}

impl LayoutSettings {
    /// Reads the layout keys, falling back per key so one unreadable value
    /// cannot discard the rest of the stored layout.
    /// Reads every key the settings window owns.
    ///
    /// The document is a flat list of independent keys and this is its
    /// per-key fallback table, so splitting it would only scatter one
    /// mapping across several functions.
    #[allow(clippy::too_many_lines, reason = "one fallback arm per stored key")]
    fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        let legacy_order = config
            .get("layout_panel_order")
            .and_then(LayoutSettings::parse_panel_order)
            .unwrap_or_else(|| defaults.panel_order.clone());
        let legacy_weights = config
            .get("layout_panel_weights")
            .and_then(Self::parse_panel_weights)
            .unwrap_or_else(|| defaults.panel_weights.clone());
        let dock_tree = config
            .get("layout_dock_tree")
            .and_then(DockNode::decode)
            .filter(|tree| tree.leaf_order().len() == DOCK_IDS.len())
            .or_else(|| DockNode::from_legacy(&legacy_order, &legacy_weights))
            .unwrap_or_else(|| defaults.dock_tree.clone());
        Self {
            panel_order: dock_tree.leaf_order(),
            show_scenes: flag(config, "layout_show_scenes", defaults.show_scenes),
            show_sources: flag(config, "layout_show_sources", defaults.show_sources),
            show_mixer: flag(config, "layout_show_mixer", defaults.show_mixer),
            show_transitions: flag(config, "layout_show_transitions", defaults.show_transitions),
            show_controls: flag(config, "layout_show_controls", defaults.show_controls),
            view_mode: config
                .get("layout_view_mode")
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|mode| (0..=2).contains(mode))
                .unwrap_or(defaults.view_mode),
            dock_height: config
                .get("layout_dock_height")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|height| (120..=1_200).contains(height))
                .unwrap_or(defaults.dock_height),
            panel_weights: legacy_weights,
            floating_panels: config
                .get("layout_floating_panels")
                .map(Self::parse_floating)
                .unwrap_or(defaults.floating_panels),
            floating_geometry: config
                .get("layout_floating_geometry")
                .map(Self::parse_floating_geometry)
                .unwrap_or(defaults.floating_geometry),
            projector_geometry: config
                .get("layout_projector_geometry")
                .map(Self::parse_projector_geometry)
                .unwrap_or(defaults.projector_geometry),
            dock_tree,
        }
    }
}

impl AppSettings {
    #[cfg(test)]
    pub(crate) fn stream_endpoint(&self) -> Option<String> {
        self.stream_target().endpoint()
    }

    pub(crate) fn stream_target(&self) -> StreamTarget {
        match self.stream_protocol {
            StreamProtocol::Rtmp => StreamTarget::Rtmp(self.rtmp.clone()),
            StreamProtocol::Rtmps => StreamTarget::Rtmps(self.rtmp.clone()),
            StreamProtocol::Srt => StreamTarget::Srt(self.srt.clone()),
            StreamProtocol::Whip => StreamTarget::Whip(WhipConfig {
                endpoint: self.whip_endpoint.clone(),
                bearer_token: self.whip_bearer_token.clone(),
            }),
            StreamProtocol::Hls => StreamTarget::Hls(self.hls.clone()),
            StreamProtocol::Rist => StreamTarget::Rist(self.rist.clone()),
            StreamProtocol::Reference => StreamTarget::Reference {
                address: self.reference_address.clone(),
            },
        }
    }

    /// Reads settings from `path`, falling back to defaults for anything the
    /// document does not contain or cannot express.
    #[allow(dead_code)]
    pub(crate) fn load(path: &Path) -> Self {
        Self::load_with_status(path).settings
    }

    /// Reads settings and determines whether the blocking setup wizard belongs
    /// on the first startup.
    pub(crate) fn load_with_status(path: &Path) -> SettingsLoad {
        let Ok(document) = std::fs::read_to_string(path) else {
            return SettingsLoad {
                settings: Self::default(),
                show_setup: !path.exists(),
            };
        };
        let Ok(config) = Config::parse(&document) else {
            // An existing but malformed document should not trap a user in the
            // wizard. The regular settings loader still falls back safely.
            let settings = Self {
                setup_state: SetupState::Completed,
                ..Self::default()
            };
            return SettingsLoad {
                settings,
                show_setup: false,
            };
        };
        let mut settings = Self::from_config(&config);
        // Files from before first-run setup existed are already a configured
        // installation. Only a newly created file or an explicit pending state
        // can open the wizard.
        if config.get("setup_state").is_none() {
            settings.setup_state = SetupState::Completed;
        }
        let show_setup = settings.setup_state == SetupState::Pending;
        SettingsLoad {
            settings,
            show_setup,
        }
    }

    /// Writes the settings document, creating the file when it is missing.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the document cannot be written.
    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temporary, self.to_config().serialize())?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    /// Reads every key the settings window owns.
    ///
    /// The document is a flat list of independent keys and this is its per-key
    /// fallback table, so splitting it would only scatter one mapping across
    /// several functions.
    #[allow(clippy::too_many_lines, reason = "one fallback arm per stored key")]
    fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        let (hls, rist) = extended_stream_config(config, &defaults);
        Self {
            locale: config
                .get("locale")
                .filter(|value| UiLocale::from_code(value).is_some())
                .map_or_else(|| defaults.locale.clone(), str::to_owned),
            theme: config
                .get("theme")
                .and_then(|value| THEMES.iter().position(|theme| theme.key == value))
                .unwrap_or(defaults.theme),
            style: config
                .get("appearance_style")
                .and_then(UiStyle::from_id)
                .unwrap_or(defaults.style),
            font_size: config
                .get("appearance_font_size")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|size| FONT_SIZE_RANGE.contains(size))
                .unwrap_or(defaults.font_size),
            density: config
                .get("appearance_density")
                .and_then(UiDensity::from_id)
                .unwrap_or(defaults.density),
            confirm_start_stream: flag(
                config,
                "confirm_start_stream",
                defaults.confirm_start_stream,
            ),
            confirm_stop_stream: flag(config, "confirm_stop_stream", defaults.confirm_stop_stream),
            confirm_stop_recording: flag(
                config,
                "confirm_stop_recording",
                defaults.confirm_stop_recording,
            ),
            auto_record_when_streaming: flag(
                config,
                "auto_record_when_streaming",
                defaults.auto_record_when_streaming,
            ),
            canvas_snap_distance: config
                .get("canvas_snap_distance")
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|distance| CANVAS_SNAP_DISTANCE_RANGE.contains(distance))
                .unwrap_or(defaults.canvas_snap_distance),
            show_safe_areas: flag(config, "show_safe_areas", defaults.show_safe_areas),
            sample_rate: config
                .get("audio_sample_rate")
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(|rate| SAMPLE_RATES.iter().position(|value| *value == rate))
                .unwrap_or(defaults.sample_rate),
            channels: config
                .get("audio_channels")
                .and_then(|value| value.parse::<u16>().ok())
                .and_then(|count| {
                    CHANNEL_LAYOUTS
                        .iter()
                        .position(|layout| layout.channels() == count)
                })
                .unwrap_or(defaults.channels),
            audio_input_sync_offset_millis: config
                .get("audio_input_sync_offset_millis")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|offset| AUDIO_SYNC_OFFSET_RANGE.contains(offset))
                .unwrap_or(defaults.audio_input_sync_offset_millis),
            desktop_audio_sync_offset_millis: config
                .get("desktop_audio_sync_offset_millis")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|offset| AUDIO_SYNC_OFFSET_RANGE.contains(offset))
                .unwrap_or(defaults.desktop_audio_sync_offset_millis),
            hotkey_swap: hotkey(config, "hotkey_swap", &defaults.hotkey_swap),
            hotkey_start_recording: hotkey(
                config,
                "hotkey_start_recording",
                &defaults.hotkey_start_recording,
            ),
            hotkey_stop_recording: hotkey(
                config,
                "hotkey_stop_recording",
                &defaults.hotkey_stop_recording,
            ),
            hotkey_start_streaming: hotkey(
                config,
                "hotkey_start_streaming",
                &defaults.hotkey_start_streaming,
            ),
            hotkey_stop_streaming: hotkey(
                config,
                "hotkey_stop_streaming",
                &defaults.hotkey_stop_streaming,
            ),
            hotkey_undo: hotkey(config, "hotkey_undo", &defaults.hotkey_undo),
            hotkey_redo: hotkey(config, "hotkey_redo", &defaults.hotkey_redo),
            hotkey_save_project: hotkey(
                config,
                "hotkey_save_project",
                &defaults.hotkey_save_project,
            ),
            hotkey_fade_transition: hotkey(
                config,
                "hotkey_fade_transition",
                &defaults.hotkey_fade_transition,
            ),
            hotkey_save_replay: hotkey(config, "hotkey_save_replay", &defaults.hotkey_save_replay),
            hotkey_start_replay: hotkey(
                config,
                "hotkey_start_replay",
                &defaults.hotkey_start_replay,
            ),
            hotkey_stop_replay: hotkey(config, "hotkey_stop_replay", &defaults.hotkey_stop_replay),
            preview_border_color: colour_text(
                config,
                "preview_border_color",
                &defaults.preview_border_color,
            ),
            program_border_color: colour_text(
                config,
                "program_border_color",
                &defaults.program_border_color,
            ),
            project_path: text(config, "project_path", &defaults.project_path),
            diagnostics_path: text(config, "diagnostics_path", &defaults.diagnostics_path),
            recording_path: text(config, "recording_path", &defaults.recording_path),
            recording_directory: text(config, "recording_directory", &defaults.recording_directory),
            recording_filename_without_spaces: flag(
                config,
                "recording_filename_without_spaces",
                defaults.recording_filename_without_spaces,
            ),
            recording_quality: config
                .get("recording_quality")
                .and_then(RecordingQuality::from_id)
                .unwrap_or(defaults.recording_quality),
            recording_auto_remux: flag(
                config,
                "recording_auto_remux",
                defaults.recording_auto_remux,
            ),
            recording_audio_encoder: EncoderImplementation::new(text(
                config,
                "recording_audio_encoder",
                defaults.recording_audio_encoder.id(),
            )),
            replay_buffer_duration_seconds: config
                .get("replay_buffer_duration_seconds")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|duration| REPLAY_BUFFER_DURATION_RANGE.contains(duration))
                .unwrap_or(defaults.replay_buffer_duration_seconds),
            replay_buffer_capacity_mib: config
                .get("replay_buffer_capacity_mib")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|capacity| REPLAY_BUFFER_CAPACITY_MIB_RANGE.contains(capacity))
                .unwrap_or(defaults.replay_buffer_capacity_mib),
            recording_split_enabled: flag(
                config,
                "recording_split_enabled",
                defaults.recording_split_enabled,
            ),
            recording_split_duration_minutes: config
                .get("recording_split_duration_minutes")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|duration| RECORDING_SPLIT_DURATION_MINUTES_RANGE.contains(duration))
                .unwrap_or(defaults.recording_split_duration_minutes),
            recording_split_size_mib: config
                .get("recording_split_size_mib")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|capacity| RECORDING_SPLIT_SIZE_MIB_RANGE.contains(capacity))
                .unwrap_or(defaults.recording_split_size_mib),
            recording_split_max_segments: config
                .get("recording_split_max_segments")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|segments| RECORDING_SPLIT_SEGMENTS_RANGE.contains(segments))
                .unwrap_or(defaults.recording_split_max_segments),
            output_mode: config
                .get("output_mode")
                .and_then(OutputMode::from_id)
                .unwrap_or(defaults.output_mode),
            stream_custom_encoder: flag(
                config,
                "stream_custom_encoder",
                defaults.stream_custom_encoder,
            ),
            video: video_from_config(config, defaults.video),
            recording_format: config
                .get("recording_format")
                .and_then(RecordingFormat::from_id)
                .unwrap_or(defaults.recording_format),
            recording_codec: config
                .get("recording_codec")
                .and_then(VideoCodec::from_id)
                .unwrap_or(defaults.recording_codec),
            stream_protocol: config
                .get("stream_protocol")
                .and_then(StreamProtocol::from_id)
                .unwrap_or(defaults.stream_protocol),
            rtmp: rtmp_from_config(config, &defaults.rtmp),
            srt: srt_from_config(config, &defaults.srt),
            whip_endpoint: text(config, "whip_endpoint", &defaults.whip_endpoint),
            whip_bearer_token: optional_text(config, "whip_bearer_token").map(SecretString::new),
            hls,
            rist,
            reference_address: text(config, "reference_address", &defaults.reference_address),
            audio_input_id: text(config, "audio_input_id", &defaults.audio_input_id),
            audio_monitor_output_id: text(
                config,
                "audio_monitor_output_id",
                &defaults.audio_monitor_output_id,
            ),
            microphone_monitor_mode: config
                .get("microphone_monitor_mode")
                .and_then(audio_monitor_mode_from_id)
                .unwrap_or(defaults.microphone_monitor_mode),
            desktop_audio_monitor_mode: config
                .get("desktop_audio_monitor_mode")
                .and_then(audio_monitor_mode_from_id)
                .unwrap_or(defaults.desktop_audio_monitor_mode),
            last_preview_scene: text(config, "last_preview_scene", &defaults.last_preview_scene),
            last_program_scene: text(config, "last_program_scene", &defaults.last_program_scene),
            project_scene_selections: config
                .get(PROJECT_SCENE_SELECTIONS_KEY)
                .map(parse_project_scene_selections)
                .unwrap_or_default(),
            restore_project: flag(config, "restore_project", defaults.restore_project),
            save_project_on_exit: flag(
                config,
                "save_project_on_exit",
                defaults.save_project_on_exit,
            ),
            setup_state: config
                .get("setup_state")
                .and_then(SetupState::from_id)
                .unwrap_or(defaults.setup_state),
            setup_benchmark_summary: bounded_text(
                config,
                "setup_benchmark_summary",
                &defaults.setup_benchmark_summary,
                4_096,
            ),
            layout: LayoutSettings::from_config(config),
        }
    }

    /// Writes every key the settings window owns.
    ///
    /// The inverse of [`AppSettings::from_config`], and a flat list for the
    /// same reason.
    #[allow(clippy::too_many_lines, reason = "one entry per stored key")]
    fn to_config(&self) -> Config {
        let mut config = Config::new();
        let entries = [
            ("locale", self.locale.clone()),
            (
                "theme",
                THEMES[self.theme.min(THEMES.len() - 1)].key.to_owned(),
            ),
            (
                "confirm_start_stream",
                self.confirm_start_stream.to_string(),
            ),
            ("confirm_stop_stream", self.confirm_stop_stream.to_string()),
            (
                "confirm_stop_recording",
                self.confirm_stop_recording.to_string(),
            ),
            (
                "auto_record_when_streaming",
                self.auto_record_when_streaming.to_string(),
            ),
            (
                "canvas_snap_distance",
                self.canvas_snap_distance.to_string(),
            ),
            ("show_safe_areas", self.show_safe_areas.to_string()),
            ("audio_sample_rate", self.sample_rate_hz().to_string()),
            ("audio_channels", self.channel_count().to_string()),
            (
                "audio_input_sync_offset_millis",
                self.audio_input_sync_offset_millis.to_string(),
            ),
            (
                "desktop_audio_sync_offset_millis",
                self.desktop_audio_sync_offset_millis.to_string(),
            ),
            ("hotkey_swap", self.hotkey_swap.clone()),
            (
                "hotkey_start_recording",
                self.hotkey_start_recording.clone(),
            ),
            ("hotkey_stop_recording", self.hotkey_stop_recording.clone()),
            (
                "hotkey_start_streaming",
                self.hotkey_start_streaming.clone(),
            ),
            ("hotkey_stop_streaming", self.hotkey_stop_streaming.clone()),
            ("hotkey_undo", self.hotkey_undo.clone()),
            ("hotkey_redo", self.hotkey_redo.clone()),
            ("hotkey_save_project", self.hotkey_save_project.clone()),
            (
                "hotkey_fade_transition",
                self.hotkey_fade_transition.clone(),
            ),
            ("hotkey_save_replay", self.hotkey_save_replay.clone()),
            ("hotkey_start_replay", self.hotkey_start_replay.clone()),
            ("hotkey_stop_replay", self.hotkey_stop_replay.clone()),
            ("preview_border_color", self.preview_border_color.clone()),
            ("program_border_color", self.program_border_color.clone()),
            ("project_path", self.project_path.clone()),
            ("diagnostics_path", self.diagnostics_path.clone()),
            ("recording_path", self.recording_path.clone()),
            ("recording_directory", self.recording_directory.clone()),
            (
                "recording_filename_without_spaces",
                self.recording_filename_without_spaces.to_string(),
            ),
            ("recording_quality", self.recording_quality.id().to_owned()),
            (
                "recording_auto_remux",
                self.recording_auto_remux.to_string(),
            ),
            (
                "recording_audio_encoder",
                self.recording_audio_encoder.id().to_owned(),
            ),
            (
                "replay_buffer_duration_seconds",
                self.replay_buffer_duration_seconds.to_string(),
            ),
            (
                "replay_buffer_capacity_mib",
                self.replay_buffer_capacity_mib.to_string(),
            ),
            (
                "recording_split_enabled",
                self.recording_split_enabled.to_string(),
            ),
            (
                "recording_split_duration_minutes",
                self.recording_split_duration_minutes.to_string(),
            ),
            (
                "recording_split_size_mib",
                self.recording_split_size_mib.to_string(),
            ),
            (
                "recording_split_max_segments",
                self.recording_split_max_segments.to_string(),
            ),
            ("output_mode", self.output_mode.id().to_owned()),
            (
                "stream_custom_encoder",
                self.stream_custom_encoder.to_string(),
            ),
            ("appearance_style", self.style.id().to_owned()),
            ("appearance_font_size", self.font_size.to_string()),
            ("appearance_density", self.density.id().to_owned()),
            ("video_base_width", self.video.base_width.to_string()),
            ("video_base_height", self.video.base_height.to_string()),
            ("video_output_width", self.video.output_width.to_string()),
            ("video_output_height", self.video.output_height.to_string()),
            (
                "video_scale_filter",
                self.video.scale_filter.id().to_owned(),
            ),
            ("video_fps_mode", self.video.fps_mode.id().to_owned()),
            ("video_fps_numerator", self.video.fps_numerator.to_string()),
            (
                "video_fps_denominator",
                self.video.fps_denominator.to_string(),
            ),
            ("recording_format", self.recording_format.id().to_owned()),
            ("recording_codec", self.recording_codec.id().to_owned()),
            ("audio_input_id", self.audio_input_id.clone()),
            (
                "audio_monitor_output_id",
                self.audio_monitor_output_id.clone(),
            ),
            (
                "microphone_monitor_mode",
                audio_monitor_mode_id(self.microphone_monitor_mode).to_owned(),
            ),
            (
                "desktop_audio_monitor_mode",
                audio_monitor_mode_id(self.desktop_audio_monitor_mode).to_owned(),
            ),
            ("last_preview_scene", self.last_preview_scene.clone()),
            ("last_program_scene", self.last_program_scene.clone()),
            (
                PROJECT_SCENE_SELECTIONS_KEY,
                serialize_project_scene_selections(
                    &self.project_scene_selections,
                    Some(&self.project_path),
                ),
            ),
            ("restore_project", self.restore_project.to_string()),
            (
                "save_project_on_exit",
                self.save_project_on_exit.to_string(),
            ),
            ("setup_state", self.setup_state.id().to_owned()),
            (
                "setup_benchmark_summary",
                self.setup_benchmark_summary.chars().take(4_096).collect(),
            ),
            ("layout_panel_order", self.layout.panel_order_text()),
            ("layout_show_scenes", self.layout.show_scenes.to_string()),
            ("layout_show_sources", self.layout.show_sources.to_string()),
            ("layout_show_mixer", self.layout.show_mixer.to_string()),
            (
                "layout_show_transitions",
                self.layout.show_transitions.to_string(),
            ),
            (
                "layout_show_controls",
                self.layout.show_controls.to_string(),
            ),
            ("layout_view_mode", self.layout.view_mode.to_string()),
            ("layout_dock_height", self.layout.dock_height.to_string()),
            ("layout_panel_weights", self.layout.panel_weights_text()),
            ("layout_floating_panels", self.layout.floating_text()),
            (
                "layout_floating_geometry",
                self.layout.floating_geometry_text(),
            ),
            (
                "layout_projector_geometry",
                self.layout.projector_geometry_text(),
            ),
            (
                "layout_dock_tree",
                self.layout
                    .dock_tree
                    .encode()
                    .unwrap_or_else(|| LayoutSettings::default().dock_tree.encode().unwrap()),
            ),
        ];
        for (key, value) in entries {
            // Every key here is a literal identifier and every value is bounded
            // UI text, so a rejection can only mean a programming error.
            debug_assert!(config.set(key, &value).is_ok(), "settings key {key}");
            let _ = config.set(key, &value);
        }
        self.write_stream_config(&mut config);
        config
    }

    fn write_stream_config(&self, config: &mut Config) {
        for (key, value) in self.stream_config_entries() {
            debug_assert!(config.set(key, &value).is_ok(), "settings key {key}");
            let _ = config.set(key, &value);
        }
    }

    fn stream_config_entries(&self) -> Vec<(&'static str, String)> {
        let mut entries = vec![
            ("stream_protocol", self.stream_protocol.id().to_owned()),
            ("rtmp_service", self.rtmp.service.clone()),
            ("rtmp_server", self.rtmp.server.clone()),
            (
                "rtmp_stream_key",
                self.rtmp.stream_key.expose_secret().to_owned(),
            ),
            (
                "stream_video_encoder",
                self.rtmp.video.implementation.id().to_owned(),
            ),
            (
                "stream_audio_encoder",
                self.rtmp.audio.implementation.id().to_owned(),
            ),
            (
                "stream_video_bitrate_kbps",
                self.rtmp.video.bitrate_kbps.to_string(),
            ),
            (
                "stream_audio_bitrate_kbps",
                self.rtmp.audio.bitrate_kbps.to_string(),
            ),
            (
                "stream_audio_sample_rate",
                self.rtmp.audio.sample_rate.to_string(),
            ),
            (
                "stream_audio_channels",
                self.rtmp.audio.channels.to_string(),
            ),
            (
                "stream_audio_complexity",
                self.rtmp
                    .audio
                    .complexity
                    .map_or_else(String::new, |value| value.to_string()),
            ),
            (
                "stream_rate_control",
                self.rtmp.video.rate_control.id().to_owned(),
            ),
            (
                "stream_keyframe_interval_secs",
                self.rtmp.video.keyframe_interval_secs.to_string(),
            ),
            ("stream_preset", self.rtmp.video.preset.id().to_owned()),
            (
                "stream_profile",
                self.rtmp.video.profile.clone().unwrap_or_default(),
            ),
            ("stream_b_frames", self.rtmp.video.b_frames.to_string()),
            ("stream_reconnect", self.rtmp.reconnect.to_string()),
            (
                "stream_maximum_retries",
                self.rtmp.maximum_retries.to_string(),
            ),
            (
                "stream_network_buffer_ms",
                self.rtmp.network_buffer_ms.to_string(),
            ),
            ("srt_host", self.srt.host.clone()),
            ("srt_port", self.srt.port.to_string()),
            ("srt_mode", self.srt.mode.id().to_owned()),
            ("srt_latency_ms", self.srt.latency_ms.to_string()),
            (
                "srt_passphrase",
                self.srt
                    .passphrase
                    .as_ref()
                    .map_or_else(String::new, |value| value.expose_secret().to_owned()),
            ),
            (
                "srt_pbkeylen",
                self.srt
                    .pbkeylen
                    .map_or_else(String::new, |value| value.bytes().to_string()),
            ),
            (
                "srt_stream_id",
                self.srt.stream_id.clone().unwrap_or_default(),
            ),
            (
                "srt_connect_timeout_ms",
                self.srt.connect_timeout_ms.to_string(),
            ),
            ("whip_endpoint", self.whip_endpoint.clone()),
            (
                "whip_bearer_token",
                self.whip_bearer_token
                    .as_ref()
                    .map_or_else(String::new, |value| value.expose_secret().to_owned()),
            ),
            ("reference_address", self.reference_address.clone()),
        ];
        entries.extend(extended_stream_entries(self));
        entries
    }

    /// Restores the stored dock layout into a freshly built window.
    pub(crate) fn apply_layout(&self, ui: &crate::MainWindow) {
        self.layout.apply(ui);
    }
}

/// Reads the Video page's keys, falling back per key.
///
/// A document written before the canvas and the output were separate values
/// contains neither key, so both fall back to the shipped defaults rather than
/// leaving the page blank. A stored resolution outside the renderer's budget
/// is treated the same way as an unparsable one.
fn video_from_config(config: &Config, defaults: VideoSettings) -> VideoSettings {
    let dimension = |key: &str, fallback: u32| {
        config
            .get(key)
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| (1..=MAX_DIMENSION).contains(value))
            .unwrap_or(fallback)
    };
    let video = VideoSettings {
        base_width: dimension("video_base_width", defaults.base_width),
        base_height: dimension("video_base_height", defaults.base_height),
        output_width: dimension("video_output_width", defaults.output_width),
        output_height: dimension("video_output_height", defaults.output_height),
        scale_filter: config
            .get("video_scale_filter")
            .and_then(ScaleFilter::from_id)
            .unwrap_or(defaults.scale_filter),
        fps_mode: config
            .get("video_fps_mode")
            .and_then(FpsMode::from_id)
            .unwrap_or(defaults.fps_mode),
        fps_numerator: number(config, "video_fps_numerator", defaults.fps_numerator),
        fps_denominator: number(config, "video_fps_denominator", defaults.fps_denominator),
    };
    // A pair that cannot become a format would break the renderer on the next
    // sync, so the whole video block falls back rather than half of it.
    if video.base_format().is_err() || video.output_format().is_err() {
        return defaults;
    }
    video
}

/// Returns the directory new recordings are written into by default.
///
/// `XDG_VIDEOS_DIR` is not read from `user-dirs.dirs` here — that file is a
/// shell fragment, not a config document — so the home directory's `Videos`
/// folder is used when it already exists and the per-user directory otherwise.
fn default_recording_directory() -> String {
    let videos = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Videos"))
        .filter(|path| path.is_dir());
    videos.or_else(user_directory).map_or_else(
        || ".".to_owned(),
        |path| path.to_string_lossy().into_owned(),
    )
}

fn extended_stream_config(config: &Config, defaults: &AppSettings) -> (HlsConfig, RistConfig) {
    let hls = HlsConfig {
        directory: PathBuf::from(text(
            config,
            "hls_directory",
            defaults.hls.directory.to_string_lossy().as_ref(),
        )),
        segment_duration_secs: number(
            config,
            "hls_segment_duration_secs",
            defaults.hls.segment_duration_secs,
        ),
        playlist_size: number(config, "hls_playlist_size", defaults.hls.playlist_size),
        low_latency: flag(config, "hls_low_latency", defaults.hls.low_latency),
    };
    let rist = RistConfig {
        host: text(config, "rist_host", &defaults.rist.host),
        port: number(config, "rist_port", defaults.rist.port),
        sender_buffer_ms: number(
            config,
            "rist_sender_buffer_ms",
            defaults.rist.sender_buffer_ms,
        ),
        shared_secret: optional_text(config, "rist_shared_secret").map(SecretString::new),
    };
    (hls, rist)
}

fn extended_stream_entries(settings: &AppSettings) -> Vec<(&'static str, String)> {
    vec![
        (
            "hls_directory",
            settings.hls.directory.to_string_lossy().into_owned(),
        ),
        (
            "hls_segment_duration_secs",
            settings.hls.segment_duration_secs.to_string(),
        ),
        ("hls_playlist_size", settings.hls.playlist_size.to_string()),
        ("hls_low_latency", settings.hls.low_latency.to_string()),
        ("rist_host", settings.rist.host.clone()),
        ("rist_port", settings.rist.port.to_string()),
        (
            "rist_sender_buffer_ms",
            settings.rist.sender_buffer_ms.to_string(),
        ),
        (
            "rist_shared_secret",
            settings
                .rist
                .shared_secret
                .as_ref()
                .map_or_else(String::new, |value| value.expose_secret().to_owned()),
        ),
    ]
}

/// Restores the layout OBS-RS ships with, discarding the session's arrangement.
///
/// This is the Docks menu's reset: it writes the defaults straight onto the
/// window, and the ordinary shutdown capture then persists them, so a reset
/// survives the session without a second save path.
pub(crate) fn apply_default_layout(ui: &crate::MainWindow) {
    LayoutSettings::default().apply(ui);
}

impl LayoutSettings {
    fn apply(&self, ui: &crate::MainWindow) {
        let layout = self;
        ui.set_panel_order(ModelRc::new(VecModel::from(layout.dock_tree.leaf_order())));
        ui.set_show_scenes(layout.show_scenes);
        ui.set_show_sources(layout.show_sources);
        ui.set_show_mixer(layout.show_mixer);
        ui.set_show_transitions(layout.show_transitions);
        ui.set_show_controls(layout.show_controls);
        ui.set_panel_weights(ModelRc::new(VecModel::from(layout.panel_weights.clone())));
        // Docks are restored to the row; reopening their windows is left to the
        // user, so a session never starts with windows they cannot see.
        let floating = (0..DEFAULT_PANEL_ORDER.len())
            .map(|kind| {
                i32::try_from(kind).is_ok_and(|kind| layout.floating_panels.contains(&kind))
            })
            .collect::<Vec<_>>();
        ui.set_panel_floating(ModelRc::new(VecModel::from(floating)));
        ui.set_view_mode(layout.view_mode);
        #[allow(
            clippy::cast_precision_loss,
            reason = "dock heights are bounded to 1200 logical pixels"
        )]
        ui.set_dock_height(layout.dock_height as f32);
    }
}

impl AppSettings {
    /// Reads the window's current dock layout back into this document.
    pub(crate) fn capture_layout(&mut self, ui: &crate::MainWindow) {
        let order = read_model(&ui.get_panel_order());
        // A window that somehow lost a dock keeps the stored order rather than
        // persisting a layout that could never be restored.
        if LayoutSettings::parse_panel_order(
            &order
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .is_some()
        {
            self.layout.panel_order = order;
        }
        self.layout.show_scenes = ui.get_show_scenes();
        self.layout.show_sources = ui.get_show_sources();
        self.layout.show_mixer = ui.get_show_mixer();
        self.layout.show_transitions = ui.get_show_transitions();
        self.layout.show_controls = ui.get_show_controls();
        let weights = read_model(&ui.get_panel_weights());
        if weights.len() == DEFAULT_PANEL_WEIGHTS.len() {
            self.layout.panel_weights = weights;
        }
        self.layout.dock_tree =
            DockNode::from_legacy(&self.layout.panel_order, &self.layout.panel_weights)
                .unwrap_or_else(|| LayoutSettings::default().dock_tree);
        self.layout.floating_panels = read_model(&ui.get_panel_floating())
            .into_iter()
            .enumerate()
            .filter(|(_, floating)| *floating)
            .filter_map(|(kind, _)| i32::try_from(kind).ok())
            .collect();
        self.layout.view_mode = ui.get_view_mode().clamp(0, 2);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the height is clamped into the persisted range first"
        )]
        {
            self.layout.dock_height = ui.get_dock_height().clamp(120.0, 1_200.0) as u32;
        }
    }

    /// Returns the stored locale, falling back to English for an unknown code.
    pub(crate) fn ui_locale(&self) -> UiLocale {
        UiLocale::from_code(&self.locale).unwrap_or(UiLocale::English)
    }

    /// Returns the recording format the selected quality actually writes.
    ///
    /// The lossless preset is the reference RLE pipeline, which only the
    /// OBS-RS packet container carries, so choosing it changes the format
    /// rather than silently producing a lossy file.
    pub(crate) const fn effective_recording_format(&self) -> RecordingFormat {
        if self.recording_quality.is_lossless() {
            RecordingFormat::ReferencePacket
        } else {
            self.recording_format
        }
    }

    /// Returns whether the next generated path is the final MP4 published by
    /// the bounded native remux boundary.
    pub(crate) fn recording_uses_auto_remux(&self) -> bool {
        self.recording_auto_remux
            && !self.recording_split_enabled
            && self.effective_recording_format() == RecordingFormat::Matroska
    }

    /// Returns the video codec the selected quality actually encodes with.
    pub(crate) const fn effective_recording_codec(&self) -> VideoCodec {
        if self.recording_quality.is_lossless() {
            VideoCodec::ReferenceRle
        } else {
            self.recording_codec
        }
    }

    /// Returns the file the next recording is written to.
    ///
    /// OBS names recordings from the clock, so two recordings in one session
    /// never collide and the file says when it was made. The extension always
    /// comes from the effective format, so the container and the name agree.
    pub(crate) fn recording_file_path(&self, stamp: &str) -> String {
        let name = if self.recording_filename_without_spaces {
            stamp.replace(' ', "-")
        } else {
            stamp.to_owned()
        };
        let mut path = PathBuf::from(self.recording_directory.trim());
        let extension = if self.recording_uses_auto_remux() {
            "mp4"
        } else {
            self.effective_recording_format().extension()
        };
        path.push(format!("{name}.{extension}"));
        path.to_string_lossy().into_owned()
    }

    /// Builds the encoder configuration the recording quality asks for.
    ///
    /// `format` is the encoded output geometry, not the canvas: a quality
    /// preset is a bitrate target, and the same target means something
    /// different at 720p than at 1080p.
    pub(crate) fn recording_video_encoder(&self, format: VideoFormat) -> VideoEncoderConfig {
        let codec = self.effective_recording_codec();
        let mut encoder = VideoEncoderConfig {
            codec,
            implementation: EncoderImplementation::default(),
            profile: (codec == VideoCodec::H264).then(|| "high".to_owned()),
            ..self.rtmp.video.clone()
        };
        if let Some(bitrate) = self.recording_quality.video_bitrate_kbps(format) {
            encoder.bitrate_kbps = bitrate;
        }
        encoder
    }

    /// Builds the audio encoder configuration recordings use.
    pub(crate) fn recording_audio_encoder_config(&self) -> AudioEncoderConfig {
        AudioEncoderConfig {
            codec: AudioCodec::Aac,
            implementation: self.recording_audio_encoder.clone(),
            ..self.rtmp.audio.clone()
        }
    }

    /// Returns the selected sample rate in hertz.
    pub(crate) fn sample_rate_hz(&self) -> u32 {
        SAMPLE_RATES[self.sample_rate.min(SAMPLE_RATES.len() - 1)]
    }

    /// Returns the selected channel count.
    pub(crate) fn channel_count(&self) -> u16 {
        CHANNEL_LAYOUTS[self.channels.min(CHANNEL_LAYOUTS.len() - 1)].channels()
    }

    /// Builds the complete token set for the selected theme and style, with
    /// the accessibility colour overrides applied on top.
    pub(crate) fn tokens(&self) -> ThemeTokens {
        self.tokens_for(self.theme, self.style)
    }

    /// Returns the geometry every settings page lays out against.
    pub(crate) fn metrics(&self) -> UiMetrics {
        metrics(self.density, self.font_size)
    }

    /// Returns the geometry for a density and font size that have not been
    /// committed yet, which is what makes the Appearance page previewable.
    pub(crate) fn metrics_for(density: UiDensity, font_size: u8) -> UiMetrics {
        metrics(density, font_size)
    }

    /// Builds a palette preview for `theme` and `style` while retaining this
    /// settings value's accessibility colour overrides.
    pub(crate) fn tokens_for(&self, theme: usize, style: UiStyle) -> ThemeTokens {
        let preset = styled(&THEMES[theme.min(THEMES.len() - 1)], style);
        ThemeTokens {
            window_bg: brush(preset.window_bg),
            panel_bg: brush(preset.panel_bg),
            header_bg: brush(preset.header_bg),
            header_active_bg: brush(preset.header_active_bg),
            border: brush(preset.border),
            border_strong: brush(preset.border_strong),
            row_bg: brush(preset.row_bg),
            row_selected_bg: brush(preset.row_selected_bg),
            control_bg: brush(preset.control_bg),
            text: brush(preset.text),
            text_strong: brush(preset.text_strong),
            text_muted: brush(preset.text_muted),
            accent: brush(preset.accent),
            canvas_bg: brush(preset.canvas_bg),
            preview_border: parse_colour(&self.preview_border_color)
                .map_or_else(|| brush([0x60, 0xA5, 0xFA]), Brush::SolidColor),
            program_border: parse_colour(&self.program_border_color)
                .map_or_else(|| brush([0xF8, 0x71, 0x71]), Brush::SolidColor),
            meter: brush([0x22, 0xC5, 0x5E]),
            meter_muted: brush([0x7F, 0x1D, 0x1D]),
            warning: brush([0xFB, 0xBF, 0x24]),
        }
    }
}

/// Formats `now` as `YYYY-MM-DD HH-MM-SS` in UTC.
///
/// Recording file names are derived from the clock so two recordings in one
/// session cannot collide. UTC is deliberate: a local-time name would jump
/// backwards across a daylight-saving change and produce a name that already
/// exists.
pub(crate) fn recording_stamp(now: std::time::SystemTime) -> String {
    let seconds = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3_600, (rest % 3_600) / 60, rest % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}-{minute:02}-{second:02}")
}

/// Converts days since the Unix epoch into a civil `(year, month, day)`.
///
/// This is Howard Hinnant's `civil_from_days`, which is exact for every date
/// the proleptic Gregorian calendar defines and needs no lookup tables.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the era.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The colour scheme a theme produces once its style is applied.
///
/// Styles transform the preset rather than replacing it, so a new theme is
/// automatically available in all three styles.
struct StyledPreset {
    window_bg: Rgb,
    panel_bg: Rgb,
    header_bg: Rgb,
    header_active_bg: Rgb,
    border: Rgb,
    border_strong: Rgb,
    row_bg: Rgb,
    row_selected_bg: Rgb,
    control_bg: Rgb,
    text: Rgb,
    text_strong: Rgb,
    text_muted: Rgb,
    accent: Rgb,
    canvas_bg: Rgb,
}

fn styled(preset: &ThemePreset, style: UiStyle) -> StyledPreset {
    let base = StyledPreset {
        window_bg: preset.window_bg,
        panel_bg: preset.panel_bg,
        header_bg: preset.header_bg,
        header_active_bg: preset.header_active_bg,
        border: preset.border,
        border_strong: preset.border_strong,
        row_bg: preset.row_bg,
        row_selected_bg: preset.row_selected_bg,
        control_bg: preset.control_bg,
        text: preset.text,
        text_strong: preset.text_strong,
        text_muted: preset.text_muted,
        accent: preset.accent,
        canvas_bg: preset.canvas_bg,
    };
    match style {
        UiStyle::Default => base,
        // Flat removes the panel/window separation and lets the borders
        // recede, which is the look OBS's flatter themes have.
        UiStyle::Flat => StyledPreset {
            panel_bg: base.window_bg,
            header_bg: mix(base.header_bg, base.window_bg, 160),
            row_bg: mix(base.row_bg, base.window_bg, 160),
            border: mix(base.border, base.window_bg, 180),
            border_strong: mix(base.border_strong, base.window_bg, 100),
            ..base
        },
        // Contrast pushes text and edges away from the background instead of
        // brightening everything, so the theme's identity survives.
        UiStyle::Contrast => StyledPreset {
            text: lighten(base.text, 60),
            text_strong: lighten(base.text_strong, 40),
            text_muted: lighten(base.text_muted, 70),
            border: lighten(base.border, 50),
            border_strong: lighten(base.border_strong, 60),
            accent: lighten(base.accent, 40),
            row_selected_bg: lighten(base.row_selected_bg, 30),
            ..base
        },
    }
}

/// Blends `colour` toward `other` by `amount` in 0..=255.
fn mix(colour: Rgb, other: Rgb, amount: u8) -> Rgb {
    let blend = |left: u8, right: u8| {
        let left = u16::from(left) * u16::from(255 - amount);
        let right = u16::from(right) * u16::from(amount);
        u8::try_from((left + right) / 255).unwrap_or(u8::MAX)
    };
    [
        blend(colour[0], other[0]),
        blend(colour[1], other[1]),
        blend(colour[2], other[2]),
    ]
}

/// Moves `colour` toward white by `amount` in 0..=255.
fn lighten(colour: Rgb, amount: u8) -> Rgb {
    mix(colour, [0xFF, 0xFF, 0xFF], amount)
}

/// Reads a Slint model into a plain vector.
fn read_model<T: Clone + 'static>(model: &ModelRc<T>) -> Vec<T> {
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

fn flag(config: &Config, key: &str, fallback: bool) -> bool {
    config
        .get(key)
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(fallback)
}

fn text(config: &Config, key: &str, fallback: &str) -> String {
    config
        .get(key)
        .map_or_else(|| fallback.to_owned(), str::to_owned)
}

/// Reads a shortcut through the shared structured parser and stores its
/// canonical display form. Empty input intentionally remains an unbinding;
/// malformed input falls back to the last known-good setting.
pub(crate) fn validated_hotkey(value: &str, fallback: &str) -> String {
    match Shortcut::parse(value) {
        Ok(Some(shortcut)) => shortcut.to_string(),
        Ok(None) => String::new(),
        Err(_) => fallback.to_owned(),
    }
}

/// Compiles the settings document into the bounded, toolkit-neutral shortcut
/// table used by the running desktop state.
///
/// Empty values are intentional unbindings. Settings loading and draft
/// validation already canonicalize each value, but parsing again here keeps
/// the runtime boundary defensive if a caller constructs an `AppSettings`
/// value directly.
pub(crate) fn shortcut_bindings(
    settings: &AppSettings,
) -> Result<Vec<(Shortcut, UiAction)>, UiError> {
    let values = [
        (settings.hotkey_swap.as_str(), UiAction::SwapPreviewProgram),
        (
            settings.hotkey_start_recording.as_str(),
            UiAction::StartRecording,
        ),
        (
            settings.hotkey_stop_recording.as_str(),
            UiAction::StopRecording,
        ),
        (
            settings.hotkey_start_streaming.as_str(),
            UiAction::StartStreaming,
        ),
        (
            settings.hotkey_stop_streaming.as_str(),
            UiAction::StopStreaming,
        ),
        (settings.hotkey_undo.as_str(), UiAction::Undo),
        (settings.hotkey_redo.as_str(), UiAction::Redo),
        (settings.hotkey_save_project.as_str(), UiAction::SaveProject),
        (
            settings.hotkey_fade_transition.as_str(),
            UiAction::FadeTransition,
        ),
        (
            settings.hotkey_save_replay.as_str(),
            UiAction::SaveReplayBuffer,
        ),
        (
            settings.hotkey_start_replay.as_str(),
            UiAction::StartReplayBuffer,
        ),
        (
            settings.hotkey_stop_replay.as_str(),
            UiAction::StopReplayBuffer,
        ),
    ];
    let mut bindings = Vec::with_capacity(values.len());
    for (text, action) in values {
        if let Some(shortcut) = Shortcut::parse(text)? {
            bindings.push((shortcut, action));
        }
    }
    Ok(bindings)
}

/// Returns canonical hotkeys that are assigned to more than one local action.
///
/// Empty values are intentional unbindings and are ignored. Parsing here also
/// makes conflict detection agree with runtime matching when users type aliases
/// such as `Option+F9` and `Alt+F9` into different fields.
#[must_use]
pub(crate) fn hotkey_conflicts(settings: &AppSettings) -> Vec<String> {
    let values = [
        settings.hotkey_swap.as_str(),
        settings.hotkey_start_recording.as_str(),
        settings.hotkey_stop_recording.as_str(),
        settings.hotkey_start_streaming.as_str(),
        settings.hotkey_stop_streaming.as_str(),
        settings.hotkey_undo.as_str(),
        settings.hotkey_redo.as_str(),
        settings.hotkey_save_project.as_str(),
        settings.hotkey_fade_transition.as_str(),
        settings.hotkey_save_replay.as_str(),
        settings.hotkey_start_replay.as_str(),
        settings.hotkey_stop_replay.as_str(),
    ];
    let mut counts = BTreeMap::new();
    for value in values {
        if let Ok(Some(shortcut)) = Shortcut::parse(value) {
            *counts.entry(shortcut).or_insert(0usize) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(shortcut, count)| (count > 1).then(|| shortcut.to_string()))
        .collect()
}

fn hotkey(config: &Config, key: &str, fallback: &str) -> String {
    validated_hotkey(config.get(key).unwrap_or(fallback), fallback)
}

fn serialize_project_scene_selections(
    selections: &[ProjectSceneSelection],
    preferred_key: Option<&str>,
) -> String {
    // Keep a small bounded insertion-ordered set. The profile is part of the
    // identity, so two profiles for one document must survive independently;
    // preserving order also keeps settings round-trips stable for callers that
    // compare the typed snapshot vector.
    let mut unique: Vec<&ProjectSceneSelection> =
        Vec::with_capacity(MAX_PERSISTED_PROJECT_SCENE_SELECTIONS * 2);
    for selection in selections {
        let key = selection.key();
        if key.is_empty()
            || key.len() > MAX_PERSISTED_SELECTION_KEY_BYTES
            || selection.profile().is_empty()
        {
            continue;
        }
        let existing = unique
            .iter()
            .position(|current| current.key() == key && current.profile() == selection.profile());
        if let Some(existing) = existing {
            unique[existing] = selection;
        } else if unique.len() < MAX_PERSISTED_PROJECT_SCENE_SELECTIONS * 2 {
            unique.push(selection);
        }
    }

    let mut encoded = String::from("v1");
    let ordered = unique
        .iter()
        .filter(|selection| preferred_key == Some(selection.key()))
        .copied()
        .chain(
            unique
                .iter()
                .filter(|selection| preferred_key != Some(selection.key()))
                .copied(),
        )
        .take(MAX_PERSISTED_PROJECT_SCENE_SELECTIONS);
    for selection in ordered {
        let record = [
            selection_component(selection.key()),
            selection_component(selection.profile()),
            selection_component(selection.preview().unwrap_or_default()),
            selection_component(selection.program().unwrap_or_default()),
        ]
        .join("|");
        let required = 1_usize.saturating_add(record.len());
        if encoded.len().saturating_add(required) > obs_rs_config::MAX_VALUE_BYTES {
            break;
        }
        encoded.push(';');
        encoded.push_str(&record);
    }
    encoded
}

fn parse_project_scene_selections(value: &str) -> Vec<ProjectSceneSelection> {
    let mut records: Vec<ProjectSceneSelection> =
        Vec::with_capacity(MAX_PERSISTED_PROJECT_SCENE_SELECTIONS);
    let mut parts = value.split(';');
    if parts.next() != Some("v1") {
        return Vec::new();
    }
    for record in parts {
        if records.len() == MAX_PERSISTED_PROJECT_SCENE_SELECTIONS || record.is_empty() {
            break;
        }
        let mut fields = record.split('|');
        let (Some(key), Some(profile), Some(preview), Some(program)) = (
            fields.next().and_then(selection_component_decode),
            fields.next().and_then(selection_component_decode),
            fields.next().and_then(selection_component_decode),
            fields.next().and_then(selection_component_decode),
        ) else {
            continue;
        };
        if fields.next().is_some()
            || key.is_empty()
            || key.len() > MAX_PERSISTED_SELECTION_KEY_BYTES
            || profile.is_empty()
        {
            continue;
        }
        let selection = ProjectSceneSelection::new(
            key,
            profile,
            (!preview.is_empty()).then_some(preview),
            (!program.is_empty()).then_some(program),
        );
        if let Some(existing) = records.iter().position(|current| {
            current.key() == selection.key() && current.profile() == selection.profile()
        }) {
            records[existing] = selection;
        } else {
            records.push(selection);
        }
    }
    records
}

fn selection_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if (b' '..=b'~').contains(&byte) && !matches!(byte, b'%' | b';' | b'|') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    encoded
}

fn selection_component_decode(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let high = bytes.next().and_then(hex_digit)?;
        let low = bytes.next().and_then(hex_digit)?;
        decoded.push((high << 4) | low);
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn bounded_text(config: &Config, key: &str, fallback: &str, maximum: usize) -> String {
    let mut value = text(config, key, fallback);
    value.truncate(maximum);
    value
}

fn optional_text(config: &Config, key: &str) -> Option<String> {
    config
        .get(key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn optional_secret(config: &Config, key: &str) -> Option<SecretString> {
    optional_text(config, key).map(SecretString::new)
}

fn rtmp_from_config(config: &Config, defaults: &RtmpConfig) -> RtmpConfig {
    RtmpConfig {
        service: text(config, "rtmp_service", &defaults.service),
        server: text(config, "rtmp_server", &defaults.server),
        stream_key: SecretString::new(text(
            config,
            "rtmp_stream_key",
            defaults.stream_key.expose_secret(),
        )),
        video: VideoEncoderConfig {
            codec: VideoCodec::H264,
            implementation: EncoderImplementation::new(text(
                config,
                "stream_video_encoder",
                defaults.video.implementation.id(),
            )),
            rate_control: config
                .get("stream_rate_control")
                .and_then(RateControl::from_id)
                .unwrap_or(defaults.video.rate_control),
            bitrate_kbps: number(
                config,
                "stream_video_bitrate_kbps",
                defaults.video.bitrate_kbps,
            ),
            max_bitrate_kbps: None,
            keyframe_interval_secs: number(
                config,
                "stream_keyframe_interval_secs",
                defaults.video.keyframe_interval_secs,
            ),
            preset: config
                .get("stream_preset")
                .and_then(EncoderPreset::from_id)
                .unwrap_or(defaults.video.preset),
            profile: optional_text(config, "stream_profile")
                .or_else(|| defaults.video.profile.clone()),
            b_frames: number(config, "stream_b_frames", defaults.video.b_frames),
        },
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            implementation: EncoderImplementation::new(text(
                config,
                "stream_audio_encoder",
                defaults.audio.implementation.id(),
            )),
            bitrate_kbps: number(
                config,
                "stream_audio_bitrate_kbps",
                defaults.audio.bitrate_kbps,
            ),
            sample_rate: number(
                config,
                "stream_audio_sample_rate",
                defaults.audio.sample_rate,
            ),
            channels: number(config, "stream_audio_channels", defaults.audio.channels),
            complexity: config
                .get("stream_audio_complexity")
                .and_then(|value| value.parse().ok()),
        },
        reconnect: flag(config, "stream_reconnect", defaults.reconnect),
        maximum_retries: number(config, "stream_maximum_retries", defaults.maximum_retries),
        network_buffer_ms: number(
            config,
            "stream_network_buffer_ms",
            defaults.network_buffer_ms,
        ),
    }
}

fn srt_from_config(config: &Config, defaults: &SrtConfig) -> SrtConfig {
    SrtConfig {
        host: text(config, "srt_host", &defaults.host),
        port: number(config, "srt_port", defaults.port),
        mode: config
            .get("srt_mode")
            .and_then(SrtMode::from_id)
            .unwrap_or(defaults.mode),
        latency_ms: number(config, "srt_latency_ms", defaults.latency_ms),
        passphrase: optional_secret(config, "srt_passphrase"),
        pbkeylen: config
            .get("srt_pbkeylen")
            .and_then(|value| value.parse::<u16>().ok())
            .and_then(SrtKeyLength::from_bytes),
        stream_id: optional_text(config, "srt_stream_id"),
        connect_timeout_ms: number(
            config,
            "srt_connect_timeout_ms",
            defaults.connect_timeout_ms,
        ),
    }
}

fn number<T>(config: &Config, key: &str, fallback: T) -> T
where
    T: std::str::FromStr,
{
    config
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// Colour fields fall back rather than persisting text the palette cannot use.
fn colour_text(config: &Config, key: &str, fallback: &str) -> String {
    config
        .get(key)
        .filter(|value| parse_colour(value).is_some())
        .map_or_else(|| fallback.to_owned(), str::to_owned)
}

fn brush(rgb: Rgb) -> Brush {
    Brush::SolidColor(colour(rgb))
}

fn colour([red, green, blue]: Rgb) -> Color {
    Color::from_rgb_u8(red, green, blue)
}

/// Parses `#RRGGBB` (with or without the hash) into a colour.
pub(crate) fn parse_colour(value: &str) -> Option<Color> {
    let digits = value.trim().trim_start_matches('#');
    if digits.len() != 6
        || !digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some(colour([channel(0..2)?, channel(2..4)?, channel(4..6)?]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock_tree::DockAxis;

    #[test]
    fn settings_round_trip_through_the_config_document() {
        let settings = AppSettings {
            locale: "es".to_owned(),
            theme: 2,
            confirm_start_stream: true,
            auto_record_when_streaming: true,
            sample_rate: 2,
            channels: 1,
            audio_monitor_output_id: "pipewire-output-7".to_owned(),
            microphone_monitor_mode: AudioMonitorMode::MonitorOnly,
            desktop_audio_monitor_mode: AudioMonitorMode::MonitorAndOutput,
            audio_input_sync_offset_millis: 125,
            desktop_audio_sync_offset_millis: 2_500,
            hotkey_swap: "F1".to_owned(),
            hotkey_undo: "Alt+U".to_owned(),
            hotkey_redo: "Alt+Y".to_owned(),
            hotkey_save_project: "Alt+S".to_owned(),
            hotkey_fade_transition: "Alt+F".to_owned(),
            hotkey_save_replay: "Alt+R".to_owned(),
            hotkey_start_replay: "Shift+Alt+R".to_owned(),
            hotkey_stop_replay: "Ctrl+Alt+R".to_owned(),
            preview_border_color: "#00FF88".to_owned(),
            last_preview_scene: "source_scene".to_owned(),
            last_program_scene: "program".to_owned(),
            project_scene_selections: vec![
                ProjectSceneSelection::new(
                    "/tmp/a|b;c%ñ.obsrproj",
                    "live",
                    Some("source_scene".to_owned()),
                    Some("program".to_owned()),
                ),
                ProjectSceneSelection::new(
                    "/tmp/a|b;c%ñ.obsrproj",
                    "alternate",
                    Some("alternate_preview".to_owned()),
                    Some("alternate_program".to_owned()),
                ),
            ],
            recording_format: RecordingFormat::ReferencePacket,
            recording_path: "/tmp/reference.obsr".to_owned(),
            stream_protocol: StreamProtocol::Rtmps,
            rtmp: RtmpConfig {
                service: "Example Live".to_owned(),
                server: "media.example/live".to_owned(),
                stream_key: SecretString::new("test-key"),
                video: VideoEncoderConfig {
                    implementation: EncoderImplementation::new("nvh264enc"),
                    rate_control: RateControl::Vbr,
                    bitrate_kbps: 8_500,
                    keyframe_interval_secs: 3,
                    preset: EncoderPreset::Quality,
                    profile: Some("main".to_owned()),
                    b_frames: 3,
                    ..VideoEncoderConfig::default()
                },
                audio: AudioEncoderConfig {
                    implementation: EncoderImplementation::new("avenc_aac"),
                    bitrate_kbps: 192,
                    ..AudioEncoderConfig::default()
                },
                reconnect: false,
                maximum_retries: 7,
                network_buffer_ms: 2_500,
            },
            srt: SrtConfig {
                host: "srt.example".to_owned(),
                port: 10_000,
                mode: SrtMode::Rendezvous,
                latency_ms: 400,
                passphrase: Some(SecretString::new("long-enough-passphrase")),
                pbkeylen: Some(SrtKeyLength::Bits256),
                stream_id: Some("publish/feed".to_owned()),
                connect_timeout_ms: 12_000,
            },
            whip_bearer_token: Some(SecretString::new("private-whip-token")),
            rist: RistConfig {
                shared_secret: Some(SecretString::new("private-rist-secret")),
                ..RistConfig::default()
            },
            ..AppSettings::default()
        };

        let decoded = AppSettings::from_config(&settings.to_config());

        assert_eq!(decoded, settings);
        assert_eq!(decoded.sample_rate_hz(), 96_000);
        assert_eq!(decoded.channel_count(), 1);
    }

    #[test]
    fn hotkeys_load_in_canonical_form_and_invalid_values_use_defaults() {
        let defaults = AppSettings::default();
        let mut config = defaults.to_config();
        config
            .set("hotkey_swap", " option + shift + f9 ")
            .expect("hotkey key");
        config
            .set("hotkey_start_recording", "Ctrl+Ctrl+R")
            .expect("hotkey key");
        config.set("hotkey_stop_recording", "").expect("hotkey key");

        let decoded = AppSettings::from_config(&config);

        assert_eq!(decoded.hotkey_swap, "Shift+Alt+F9");
        assert_eq!(
            decoded.hotkey_start_recording,
            defaults.hotkey_start_recording
        );
        assert!(decoded.hotkey_stop_recording.is_empty());
        assert_eq!(validated_hotkey("ctrl + r", "F1"), "Ctrl+R");
        assert_eq!(validated_hotkey("Ctrl+", "F1"), "F1");
    }

    #[test]
    fn hotkey_conflicts_use_canonical_shortcuts_and_ignore_unbindings() {
        let settings = AppSettings {
            hotkey_start_recording: "Ctrl+R".to_owned(),
            hotkey_stop_recording: "Alt+R".to_owned(),
            hotkey_start_streaming: String::new(),
            hotkey_undo: " control + r ".to_owned(),
            hotkey_redo: String::new(),
            hotkey_save_project: "CTRL+R".to_owned(),
            hotkey_fade_transition: String::new(),
            hotkey_save_replay: String::new(),
            hotkey_start_replay: String::new(),
            hotkey_stop_replay: String::new(),
            ..AppSettings::default()
        };

        assert_eq!(hotkey_conflicts(&settings), vec!["Ctrl+R"]);
    }

    #[test]
    fn dock_tree_round_trips_without_losing_legacy_order() {
        let tree = DockNode::Split {
            axis: DockAxis::Vertical,
            ratio_milli: 625,
            first: Box::new(DockNode::Tabs {
                docks: vec![1, 0],
                active: 0,
            }),
            second: Box::new(DockNode::Split {
                axis: DockAxis::Horizontal,
                ratio_milli: 400,
                first: Box::new(DockNode::Dock(2)),
                second: Box::new(DockNode::Tabs {
                    docks: vec![3, 4],
                    active: 1,
                }),
            }),
        };
        let mut settings = AppSettings::default();
        settings.layout.dock_tree = tree.clone();
        settings.layout.panel_order = tree.leaf_order();

        let decoded = AppSettings::from_config(&settings.to_config());

        assert_eq!(decoded.layout.dock_tree, tree);
        assert_eq!(decoded.layout.panel_order, vec![1, 0, 2, 3, 4]);
    }

    #[test]
    fn floating_geometry_round_trips_and_rejects_unsafe_records() {
        let mut settings = AppSettings::default();
        settings.layout.floating_geometry = vec![
            FloatingGeometry::new(2, -1_920, 84, 720, 520, 1_250).expect("valid geometry"),
            FloatingGeometry::new(4, 2_560, 120, 480, 700, 2_000).expect("valid geometry"),
        ];

        let decoded = AppSettings::from_config(&settings.to_config());

        assert_eq!(
            decoded.layout.floating_geometry,
            settings.layout.floating_geometry
        );

        let mut config = Config::new();
        config
            .set(
                "layout_floating_geometry",
                "v1:2:-1920:84:720:520:1250;4:0:0:99999:700:2000;bogus",
            )
            .expect("geometry key");
        let decoded = AppSettings::from_config(&config);
        assert_eq!(
            decoded.layout.floating_geometry,
            vec![FloatingGeometry::new(2, -1_920, 84, 720, 520, 1_250).expect("valid geometry")]
        );
    }

    #[test]
    fn projector_geometry_round_trips_and_rejects_unsafe_records() {
        let mut settings = AppSettings::default();
        settings.layout.projector_geometry = vec![
            ProjectorGeometry::new(ProjectorKind::Preview, -1_920, 84, 960, 540, 1_250)
                .expect("valid geometry"),
            ProjectorGeometry::new(ProjectorKind::Scene, 2_560, 120, 1_280, 720, 2_000)
                .expect("valid geometry")
                .with_fullscreen(true),
        ];

        let decoded = AppSettings::from_config(&settings.to_config());

        assert_eq!(
            decoded.layout.projector_geometry,
            settings.layout.projector_geometry
        );

        let mut config = Config::new();
        config
            .set(
                "layout_projector_geometry",
                "v1:preview:-1920:84:960:540:1250;scene:2560:120:1280:720:2000;preview:300:200:960:540:1250;source:0:0:99999:540:1250;unknown:0:0:960:540:1250",
            )
            .expect("geometry key");
        let decoded = AppSettings::from_config(&config);
        assert_eq!(
            decoded.layout.projector_geometry,
            vec![
                ProjectorGeometry::new(ProjectorKind::Preview, -1_920, 84, 960, 540, 1_250)
                    .expect("valid geometry"),
                ProjectorGeometry::new(ProjectorKind::Scene, 2_560, 120, 1_280, 720, 2_000)
                    .expect("valid geometry"),
            ]
        );

        config
            .set(
                "layout_projector_geometry",
                "v2:preview:-1920:84:960:540:1250:0;scene:2560:120:1280:720:2000:1;source:0:0:960:540:1250:9;multiview:0:0:960:540:1250:1;multiview:10:10:960:540:1250:0",
            )
            .expect("fullscreen geometry key");
        let decoded = AppSettings::from_config(&config);
        assert!(!decoded.layout.projector_geometry[0].fullscreen);
        assert!(decoded
            .layout
            .projector_geometry
            .iter()
            .find(|entry| entry.projector == ProjectorKind::Scene)
            .is_some_and(|entry| entry.fullscreen));
        assert_eq!(
            decoded
                .layout
                .projector_geometry
                .iter()
                .filter(|entry| entry.projector == ProjectorKind::Multiview)
                .count(),
            1,
            "the first valid duplicate wins"
        );
        assert!(decoded
            .layout
            .projector_geometry
            .iter()
            .find(|entry| entry.projector == ProjectorKind::Multiview)
            .is_some_and(|entry| entry.fullscreen));
    }

    #[test]
    fn appearance_video_and_output_round_trip_through_the_document() {
        let settings = AppSettings {
            style: UiStyle::Contrast,
            font_size: 16,
            density: UiDensity::Comfortable,
            output_mode: OutputMode::Advanced,
            stream_custom_encoder: true,
            recording_quality: RecordingQuality::IndistinguishableQuality,
            recording_directory: "/tmp/obs-rs-recordings".to_owned(),
            recording_filename_without_spaces: true,
            recording_auto_remux: true,
            recording_audio_encoder: EncoderImplementation::new("avenc_aac"),
            replay_buffer_duration_seconds: 90,
            replay_buffer_capacity_mib: 128,
            recording_split_enabled: true,
            recording_split_duration_minutes: 90,
            recording_split_size_mib: 128,
            recording_split_max_segments: 12,
            video: VideoSettings {
                base_width: 2_560,
                base_height: 1_440,
                output_width: 1_600,
                output_height: 900,
                scale_filter: ScaleFilter::Lanczos,
                fps_mode: FpsMode::Fractional,
                fps_numerator: 30_000,
                fps_denominator: 1_001,
            },
            ..AppSettings::default()
        };

        let decoded = AppSettings::from_config(&settings.to_config());

        assert_eq!(decoded, settings);
        assert_eq!(decoded.video.frame_rate().numerator(), 30_000);
        assert_eq!(decoded.video.frame_rate().denominator(), 1_001);
        assert!(!decoded.video.is_unscaled());
    }

    #[test]
    fn the_shipped_defaults_match_the_reference_output_setup() {
        let settings = AppSettings::default();

        assert_eq!(settings.output_mode, OutputMode::Simple);
        assert_eq!(settings.video.base_width, 1_920);
        assert_eq!(settings.video.base_height, 1_080);
        assert_eq!(settings.video.output_width, 1_280);
        assert_eq!(settings.video.output_height, 720);
        assert_eq!(settings.video.scale_filter, ScaleFilter::Bicubic);
        assert_eq!(settings.video.fps_mode, FpsMode::Common);
        assert_eq!(settings.video.frame_rate().numerator(), 60);
        assert_eq!(settings.rtmp.video.bitrate_kbps, 6_000);
        assert_eq!(settings.rtmp.audio.bitrate_kbps, 160);
        assert_eq!(settings.density, UiDensity::Normal);
        assert_eq!(settings.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn appearance_and_video_values_outside_their_range_fall_back() {
        let mut config = AppSettings::default().to_config();
        for (key, value) in [
            ("appearance_font_size", "96"),
            ("appearance_density", "roomy"),
            ("appearance_style", "neon"),
            ("video_base_width", "0"),
            ("video_output_height", "99999"),
            ("video_scale_filter", "nearest"),
            ("video_fps_mode", "smpte"),
            ("canvas_snap_distance", "0"),
            ("show_safe_areas", "not-bool"),
            ("recording_quality", "perfect"),
            ("replay_buffer_duration_seconds", "0"),
            ("replay_buffer_capacity_mib", "999"),
            ("recording_split_duration_minutes", "0"),
            ("recording_split_size_mib", "999"),
            ("recording_split_max_segments", "0"),
            ("output_mode", "expert"),
        ] {
            config.set(key, value).expect("settings key");
        }

        let decoded = AppSettings::from_config(&config);
        let defaults = AppSettings::default();

        assert_eq!(decoded.font_size, defaults.font_size);
        assert_eq!(decoded.density, defaults.density);
        assert_eq!(decoded.style, defaults.style);
        assert_eq!(decoded.video, defaults.video);
        assert_eq!(decoded.recording_quality, defaults.recording_quality);
        assert_eq!(decoded.canvas_snap_distance, defaults.canvas_snap_distance);
        assert_eq!(decoded.show_safe_areas, defaults.show_safe_areas);
        assert_eq!(
            decoded.replay_buffer_duration_seconds,
            defaults.replay_buffer_duration_seconds
        );
        assert_eq!(
            decoded.replay_buffer_capacity_mib,
            defaults.replay_buffer_capacity_mib
        );
        assert_eq!(
            decoded.recording_split_duration_minutes,
            defaults.recording_split_duration_minutes
        );
        assert_eq!(
            decoded.recording_split_size_mib,
            defaults.recording_split_size_mib
        );
        assert_eq!(
            decoded.recording_split_max_segments,
            defaults.recording_split_max_segments
        );
        assert_eq!(decoded.output_mode, defaults.output_mode);
    }

    #[test]
    fn a_document_written_before_these_settings_existed_still_loads() {
        // Everything the new pages own is absent here, which is exactly what a
        // settings file from an older build looks like.
        let mut config = Config::new();
        config.set("theme", "slate").expect("theme key");
        config.set("locale", "es").expect("locale key");

        let decoded = AppSettings::from_config(&config);
        let defaults = AppSettings::default();

        assert_eq!(decoded.theme, 3);
        assert_eq!(decoded.locale, "es");
        assert_eq!(decoded.video, defaults.video);
        assert_eq!(decoded.style, defaults.style);
        assert_eq!(decoded.output_mode, defaults.output_mode);
        assert_eq!(decoded.recording_quality, defaults.recording_quality);
    }

    #[test]
    fn setup_state_round_trips_and_legacy_documents_do_not_open_the_wizard() {
        let settings = AppSettings {
            setup_state: SetupState::Skipped,
            setup_benchmark_summary: "recommended=720p30".to_owned(),
            ..AppSettings::default()
        };
        let decoded = AppSettings::from_config(&settings.to_config());
        assert_eq!(decoded.setup_state, SetupState::Skipped);
        assert_eq!(
            decoded.setup_benchmark_summary,
            settings.setup_benchmark_summary
        );

        let path = std::env::temp_dir().join("obs-rs-settings-legacy-setup-test.toml");
        let mut legacy = Config::new();
        legacy.set("theme", "dark").expect("legacy theme");
        std::fs::write(&path, legacy.serialize()).expect("write legacy settings");
        let loaded = AppSettings::load_with_status(&path);
        assert!(!loaded.show_setup);
        assert_eq!(loaded.settings.setup_state, SetupState::Completed);
        std::fs::remove_file(&path).expect("remove legacy settings");
    }

    #[test]
    fn missing_settings_are_pending_first_run() {
        let path = std::env::temp_dir().join("obs-rs-settings-first-run-test.toml");
        let _ = std::fs::remove_file(&path);
        let loaded = AppSettings::load_with_status(&path);
        assert!(loaded.show_setup);
        assert_eq!(loaded.settings.setup_state, SetupState::Pending);
    }

    #[test]
    fn unreadable_and_invalid_documents_fall_back_to_defaults() {
        let missing = std::env::temp_dir().join("obs-rs-settings-does-not-exist.toml");
        assert_eq!(
            AppSettings::load_with_status(&missing).settings,
            AppSettings::default()
        );

        let mut config = Config::new();
        config.set("theme", "not-a-theme").expect("theme key");
        config
            .set("audio_sample_rate", "12345")
            .expect("sample rate key");
        config
            .set("preview_border_color", "not-a-colour")
            .expect("colour key");
        let decoded = AppSettings::from_config(&config);

        assert_eq!(decoded.theme, AppSettings::default().theme);
        assert_eq!(decoded.sample_rate, AppSettings::default().sample_rate);
        assert_eq!(
            decoded.preview_border_color,
            AppSettings::default().preview_border_color
        );
    }

    #[test]
    fn audio_sync_offsets_are_bounded_before_runtime_use() {
        let defaults = AppSettings::default();
        let mut config = defaults.to_config();
        config
            .set("audio_input_sync_offset_millis", "5001")
            .expect("input offset key");
        config
            .set("desktop_audio_sync_offset_millis", "-1")
            .expect("desktop offset key");

        let decoded = AppSettings::from_config(&config);

        assert_eq!(
            decoded.audio_input_sync_offset_millis,
            defaults.audio_input_sync_offset_millis
        );
        assert_eq!(
            decoded.desktop_audio_sync_offset_millis,
            defaults.desktop_audio_sync_offset_millis
        );
    }

    #[test]
    fn audio_monitor_settings_reject_unknown_modes_and_round_trip_valid_values() {
        let settings = AppSettings {
            audio_monitor_output_id: "pipewire-output-7".to_owned(),
            microphone_monitor_mode: AudioMonitorMode::MonitorOnly,
            desktop_audio_monitor_mode: AudioMonitorMode::MonitorAndOutput,
            ..AppSettings::default()
        };
        let mut config = settings.to_config();
        config
            .set("microphone_monitor_mode", "future_mode")
            .expect("future mode");
        let decoded = AppSettings::from_config(&config);

        assert_eq!(
            decoded.audio_monitor_output_id,
            settings.audio_monitor_output_id
        );
        assert_eq!(
            decoded.microphone_monitor_mode,
            AppSettings::default().microphone_monitor_mode
        );
        assert_eq!(
            decoded.desktop_audio_monitor_mode,
            settings.desktop_audio_monitor_mode
        );
        assert_eq!(
            audio_monitor_mode_from_id(audio_monitor_mode_id(AudioMonitorMode::MonitorOnly)),
            Some(AudioMonitorMode::MonitorOnly)
        );
    }

    #[test]
    fn standard_audio_layout_indices_round_trip_by_channel_count() {
        let mut config = AppSettings::default().to_config();
        config.set("audio_channels", "6").expect("layout key");

        let decoded = AppSettings::from_config(&config);

        assert_eq!(decoded.channels, 4);
        assert_eq!(decoded.channel_count(), 6);
        assert_eq!(
            CHANNEL_LAYOUTS[decoded.channels],
            AudioChannelLayout::FivePointOne
        );

        config.set("audio_channels", "7").expect("discrete key");
        let fallback = AppSettings::from_config(&config);
        assert_eq!(fallback.channels, AppSettings::default().channels);
    }

    #[test]
    fn default_stream_config_selects_the_production_rtmp_path() {
        assert_eq!(
            AppSettings::default().stream_endpoint().as_deref(),
            Some("rtmp://127.0.0.1/live/stream")
        );
    }

    #[test]
    fn matroska_is_the_default_production_recording_format() {
        let settings = AppSettings::default();
        assert_eq!(settings.recording_format, RecordingFormat::Matroska);
        assert_eq!(
            Path::new(&settings.recording_path)
                .extension()
                .and_then(|value| value.to_str()),
            Some("mkv")
        );
    }

    #[test]
    fn mp4_recording_format_selects_a_production_extension() {
        let settings = AppSettings {
            recording_format: RecordingFormat::Mp4,
            ..AppSettings::default()
        };
        assert_eq!(
            settings.recording_file_path("2024-02-29 12-30-45"),
            format!("{}/2024-02-29 12-30-45.mp4", settings.recording_directory)
        );
    }

    #[test]
    fn automatic_remux_selects_an_mp4_final_path_only_for_unsplit_matroska() {
        let settings = AppSettings {
            recording_auto_remux: true,
            ..AppSettings::default()
        };
        assert_eq!(
            settings.recording_file_path("2024-02-29 12-30-45"),
            format!("{}/2024-02-29 12-30-45.mp4", settings.recording_directory)
        );

        let split = AppSettings {
            recording_split_enabled: true,
            ..settings.clone()
        };
        assert_eq!(
            Path::new(&split.recording_file_path("stamp"))
                .extension()
                .and_then(|value| value.to_str()),
            Some("mkv")
        );

        let lossless = AppSettings {
            recording_quality: RecordingQuality::Lossless,
            ..settings
        };
        assert_eq!(
            Path::new(&lossless.recording_file_path("stamp"))
                .extension()
                .and_then(|value| value.to_str()),
            Some("obsr")
        );
    }

    #[test]
    fn fragmented_mp4_recording_format_selects_the_same_container_extension() {
        let settings = AppSettings {
            recording_format: RecordingFormat::FragmentedMp4,
            ..AppSettings::default()
        };
        assert_eq!(
            settings.recording_file_path("2024-02-29 12-30-45"),
            format!("{}/2024-02-29 12-30-45.mp4", settings.recording_directory)
        );
        assert_eq!(
            RecordingFormat::from_id("fragmented-mp4"),
            Some(RecordingFormat::FragmentedMp4)
        );
    }

    #[test]
    fn flv_recording_format_selects_a_production_extension() {
        let settings = AppSettings {
            recording_format: RecordingFormat::Flv,
            ..AppSettings::default()
        };
        assert_eq!(
            settings.recording_file_path("2024-02-29 12-30-45"),
            format!("{}/2024-02-29 12-30-45.flv", settings.recording_directory)
        );
    }

    #[test]
    fn mov_recording_format_selects_a_production_extension() {
        let settings = AppSettings {
            recording_format: RecordingFormat::Mov,
            ..AppSettings::default()
        };
        assert_eq!(
            settings.recording_file_path("2024-02-29 12-30-45"),
            format!("{}/2024-02-29 12-30-45.mov", settings.recording_directory)
        );
    }

    #[test]
    fn settings_debug_output_redacts_stream_secrets() {
        let settings = AppSettings {
            rtmp: RtmpConfig {
                stream_key: SecretString::new("private-stream-key"),
                ..RtmpConfig::default()
            },
            srt: SrtConfig {
                passphrase: Some(SecretString::new("private-passphrase")),
                ..SrtConfig::default()
            },
            ..AppSettings::default()
        };
        let debug = format!("{settings:?}");
        assert!(!debug.contains("private-stream-key"));
        assert!(!debug.contains("private-passphrase"));
        assert!(!debug.contains("private-whip-token"));
        assert!(!debug.contains("private-rist-secret"));
    }

    #[test]
    fn recording_stamps_are_sortable_utc_civil_times() {
        let epoch = recording_stamp(std::time::UNIX_EPOCH);
        assert_eq!(epoch, "1970-01-01 00-00-00");

        let leap_day = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_709_209_845);
        assert_eq!(recording_stamp(leap_day), "2024-02-29 12-30-45");

        // A name generated without spaces must still be a legal file name and
        // must not lose any of the fields the stamp encodes.
        let settings = AppSettings {
            recording_directory: "/tmp".to_owned(),
            recording_filename_without_spaces: true,
            ..AppSettings::default()
        };
        assert_eq!(
            settings.recording_file_path("2024-02-29 12-30-45"),
            "/tmp/2024-02-29-12-30-45.mkv"
        );
        let spaced = AppSettings {
            recording_filename_without_spaces: false,
            ..settings
        };
        assert_eq!(
            spaced.recording_file_path("2024-02-29 12-30-45"),
            "/tmp/2024-02-29 12-30-45.mkv"
        );
    }

    #[test]
    fn the_lossless_preset_forces_the_container_that_can_carry_it() {
        let settings = AppSettings {
            recording_quality: RecordingQuality::Lossless,
            recording_format: RecordingFormat::Matroska,
            recording_codec: VideoCodec::H264,
            ..AppSettings::default()
        };

        assert_eq!(
            settings.effective_recording_format(),
            RecordingFormat::ReferencePacket
        );
        assert_eq!(
            settings.effective_recording_codec(),
            VideoCodec::ReferenceRle
        );
        assert_eq!(
            Path::new(&settings.recording_file_path("stamp"))
                .extension()
                .and_then(|value| value.to_str()),
            Some("obsr")
        );
    }

    #[test]
    fn styles_transform_the_theme_rather_than_replacing_it() {
        let default_style = AppSettings::default();
        let flat = AppSettings {
            style: UiStyle::Flat,
            ..AppSettings::default()
        };
        let contrast = AppSettings {
            style: UiStyle::Contrast,
            ..AppSettings::default()
        };

        // Flat merges the panel into the window; the default keeps them apart.
        assert_ne!(
            default_style.tokens().panel_bg,
            default_style.tokens().window_bg
        );
        assert_eq!(flat.tokens().panel_bg, flat.tokens().window_bg);
        // Contrast lifts the text away from the background it sits on.
        assert_ne!(contrast.tokens().text, default_style.tokens().text);
        assert_eq!(
            contrast.tokens().window_bg,
            default_style.tokens().window_bg
        );
    }

    #[test]
    fn colour_parsing_accepts_hashed_and_bare_hex_only() {
        assert_eq!(parse_colour("#FF8800"), Some(colour([0xFF, 0x88, 0x00])));
        assert_eq!(parse_colour(" ff8800 "), Some(colour([0xFF, 0x88, 0x00])));
        assert_eq!(parse_colour("#FF88"), None);
        assert_eq!(parse_colour("#GGGGGG"), None);
    }

    #[test]
    fn accessibility_colours_override_the_theme_preset() {
        let settings = AppSettings {
            program_border_color: "#00FF00".to_owned(),
            ..AppSettings::default()
        };

        let tokens = settings.tokens();

        assert_eq!(
            tokens.program_border,
            Brush::SolidColor(colour([0x00, 0xFF, 0x00]))
        );
    }

    #[test]
    fn settings_document_persists_to_disk_and_reloads() {
        let path = std::env::temp_dir().join("obs-rs-settings-persist-test.toml");
        let settings = AppSettings {
            theme: 3,
            locale: "es".to_owned(),
            hotkey_start_streaming: "F9".to_owned(),
            program_border_color: "#123456".to_owned(),
            ..AppSettings::default()
        };

        settings.save(&path).expect("settings should persist");
        let reloaded = AppSettings::load_with_status(&path).settings;

        assert_eq!(reloaded, settings);
        assert_eq!(reloaded.ui_locale(), UiLocale::Spanish);
        std::fs::remove_file(&path).expect("remove settings fixture");
    }
}
