//! Persistent application settings behind the OBS-style settings window.
//!
//! Everything the settings window edits is stored in one validated
//! [`obs_rs_config::Config`] document, so persistence is the same flat TOML
//! format the rest of OBS-RS uses and a malformed file degrades to defaults
//! rather than failing startup.

use std::path::{Path, PathBuf};

use obs_rs_config::Config;
use obs_rs_media::{ScaleFilter, VideoFormat};
use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, EncoderImplementation, EncoderPreset, HlsConfig, RateControl,
    RistConfig, RtmpConfig, SecretString, SrtConfig, SrtKeyLength, SrtMode, StreamProtocol,
    StreamTarget, VideoCodec, VideoEncoderConfig, WhipConfig,
};
use obs_rs_ui::UiLocale;
use slint::{Brush, Color, Model, ModelRc, VecModel};

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
    ReferencePacket,
}

impl RecordingFormat {
    pub(crate) const ALL: [Self; 2] = [Self::Matroska, Self::ReferencePacket];

    const fn id(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::ReferencePacket => "obsr-packet",
        }
    }

    fn from_id(value: &str) -> Option<Self> {
        match value {
            "matroska" | "mkv" => Some(Self::Matroska),
            "obsr-packet" | "obsr" => Some(Self::ReferencePacket),
            _ => None,
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Matroska => "mkv",
            Self::ReferencePacket => "obsr",
        }
    }

    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Matroska => "Matroska (.mkv)",
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
pub(crate) const CHANNEL_LAYOUTS: [u16; 2] = [2, 1];

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
    pub(crate) sample_rate: usize,
    pub(crate) channels: usize,
    pub(crate) hotkey_swap: String,
    pub(crate) hotkey_start_recording: String,
    pub(crate) hotkey_stop_recording: String,
    pub(crate) hotkey_start_streaming: String,
    pub(crate) hotkey_stop_streaming: String,
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
    pub(crate) recording_codec: VideoCodec,
    pub(crate) recording_audio_encoder: EncoderImplementation,
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
    /// Provider-stable `PipeWire` input ID; empty selects the first available
    /// input and keeps the deterministic fallback as a safe last resort.
    pub(crate) audio_input_id: String,
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
    /// Dock IDs in display order: 0 scenes, 1 sources, 2 mixer, 3 transitions,
    /// 4 controls.
    pub(crate) panel_order: Vec<i32>,
    pub(crate) show_scenes: bool,
    pub(crate) show_sources: bool,
    pub(crate) show_mixer: bool,
    pub(crate) show_transitions: bool,
    pub(crate) show_controls: bool,
    /// 0 is studio mode, 1 the single-canvas default.
    pub(crate) view_mode: i32,
    /// Height of the dock row in logical pixels.
    pub(crate) dock_height: u32,
    /// Width share per dock kind, as adjusted by the splitters.
    pub(crate) panel_weights: Vec<f32>,
    /// Dock kinds that were left detached in their own windows.
    pub(crate) floating_panels: Vec<i32>,
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
        Self {
            panel_order: DEFAULT_PANEL_ORDER.to_vec(),
            show_scenes: true,
            show_sources: true,
            show_mixer: true,
            show_transitions: true,
            show_controls: true,
            view_mode: 1,
            dock_height: 248,
            panel_weights: DEFAULT_PANEL_WEIGHTS.to_vec(),
            floating_panels: Vec::new(),
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

    fn panel_order_text(&self) -> String {
        self.panel_order
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
            // 48 kHz stereo, matching the mixer the desktop state starts with.
            sample_rate: 1,
            channels: 0,
            hotkey_swap: "Space".to_owned(),
            hotkey_start_recording: "Ctrl+R".to_owned(),
            hotkey_stop_recording: "Ctrl+Shift+R".to_owned(),
            hotkey_start_streaming: "Ctrl+B".to_owned(),
            hotkey_stop_streaming: "Ctrl+Shift+B".to_owned(),
            preview_border_color: "#60A5FA".to_owned(),
            program_border_color: "#F87171".to_owned(),
            project_path: user_file(PROJECT_FILE),
            diagnostics_path: user_file(DIAGNOSTICS_FILE),
            recording_path: user_file("obs-rs-recording.mkv"),
            recording_directory: default_recording_directory(),
            recording_filename_without_spaces: false,
            recording_quality: RecordingQuality::default(),
            recording_format: RecordingFormat::Matroska,
            recording_codec: VideoCodec::H264,
            recording_audio_encoder: EncoderImplementation::default(),
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
        Self {
            panel_order: config
                .get("layout_panel_order")
                .and_then(LayoutSettings::parse_panel_order)
                .unwrap_or(defaults.panel_order),
            show_scenes: flag(config, "layout_show_scenes", defaults.show_scenes),
            show_sources: flag(config, "layout_show_sources", defaults.show_sources),
            show_mixer: flag(config, "layout_show_mixer", defaults.show_mixer),
            show_transitions: flag(config, "layout_show_transitions", defaults.show_transitions),
            show_controls: flag(config, "layout_show_controls", defaults.show_controls),
            view_mode: config
                .get("layout_view_mode")
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|mode| (0..=1).contains(mode))
                .unwrap_or(defaults.view_mode),
            dock_height: config
                .get("layout_dock_height")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|height| (120..=1_200).contains(height))
                .unwrap_or(defaults.dock_height),
            panel_weights: config
                .get("layout_panel_weights")
                .and_then(Self::parse_panel_weights)
                .unwrap_or(defaults.panel_weights),
            floating_panels: config
                .get("layout_floating_panels")
                .map(Self::parse_floating)
                .unwrap_or(defaults.floating_panels),
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
            sample_rate: config
                .get("audio_sample_rate")
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(|rate| SAMPLE_RATES.iter().position(|value| *value == rate))
                .unwrap_or(defaults.sample_rate),
            channels: config
                .get("audio_channels")
                .and_then(|value| value.parse::<u16>().ok())
                .and_then(|count| CHANNEL_LAYOUTS.iter().position(|value| *value == count))
                .unwrap_or(defaults.channels),
            hotkey_swap: text(config, "hotkey_swap", &defaults.hotkey_swap),
            hotkey_start_recording: text(
                config,
                "hotkey_start_recording",
                &defaults.hotkey_start_recording,
            ),
            hotkey_stop_recording: text(
                config,
                "hotkey_stop_recording",
                &defaults.hotkey_stop_recording,
            ),
            hotkey_start_streaming: text(
                config,
                "hotkey_start_streaming",
                &defaults.hotkey_start_streaming,
            ),
            hotkey_stop_streaming: text(
                config,
                "hotkey_stop_streaming",
                &defaults.hotkey_stop_streaming,
            ),
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
            recording_audio_encoder: EncoderImplementation::new(text(
                config,
                "recording_audio_encoder",
                defaults.recording_audio_encoder.id(),
            )),
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
            ("audio_sample_rate", self.sample_rate_hz().to_string()),
            ("audio_channels", self.channel_count().to_string()),
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
                "recording_audio_encoder",
                self.recording_audio_encoder.id().to_owned(),
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
        ui.set_panel_order(ModelRc::new(VecModel::from(layout.panel_order.clone())));
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
        self.layout.floating_panels = read_model(&ui.get_panel_floating())
            .into_iter()
            .enumerate()
            .filter(|(_, floating)| *floating)
            .filter_map(|(kind, _)| i32::try_from(kind).ok())
            .collect();
        self.layout.view_mode = ui.get_view_mode().clamp(0, 1);
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
        path.push(format!(
            "{name}.{}",
            self.effective_recording_format().extension()
        ));
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
        CHANNEL_LAYOUTS[self.channels.min(CHANNEL_LAYOUTS.len() - 1)]
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

    #[test]
    fn settings_round_trip_through_the_config_document() {
        let settings = AppSettings {
            locale: "es".to_owned(),
            theme: 2,
            confirm_start_stream: true,
            auto_record_when_streaming: true,
            sample_rate: 2,
            channels: 1,
            hotkey_swap: "F1".to_owned(),
            preview_border_color: "#00FF88".to_owned(),
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
            recording_audio_encoder: EncoderImplementation::new("avenc_aac"),
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
            ("recording_quality", "perfect"),
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
