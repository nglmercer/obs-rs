//! Persistent application settings behind the OBS-style settings window.
//!
//! Everything the settings window edits is stored in one validated
//! [`obs_rs_config::Config`] document, so persistence is the same key/value
//! format the rest of OBS-RS uses and a malformed file degrades to defaults
//! rather than failing startup.

use std::path::Path;

use obs_rs_config::Config;
use obs_rs_ui::UiLocale;
use slint::{Brush, Color};

use crate::ThemeTokens;

/// File the settings document is read from and written to.
pub(crate) const SETTINGS_FILE: &str = "obs-rs-settings.txt";

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppSettings {
    pub(crate) locale: String,
    pub(crate) theme: usize,
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
    pub(crate) streaming_address: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            locale: "en".to_owned(),
            theme: 0,
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
            project_path: "obs-rs-project.txt".to_owned(),
            diagnostics_path: "obs-rs-diagnostics.obsrdg".to_owned(),
            recording_path: "obs-rs-recording.y4m".to_owned(),
            streaming_address: "127.0.0.1:9000".to_owned(),
        }
    }
}

impl AppSettings {
    /// Reads settings from `path`, falling back to defaults for anything the
    /// document does not contain or cannot express.
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(document) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(config) = Config::parse(&document) else {
            return Self::default();
        };
        Self::from_config(&config)
    }

    /// Writes the settings document, creating the file when it is missing.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the document cannot be written.
    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_config().serialize())
    }

    fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        Self {
            locale: config
                .get("locale")
                .filter(|value| UiLocale::from_code(value).is_some())
                .map_or_else(|| defaults.locale.clone(), str::to_owned),
            theme: config
                .get("theme")
                .and_then(|value| THEMES.iter().position(|theme| theme.key == value))
                .unwrap_or(defaults.theme),
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
            streaming_address: text(config, "streaming_address", &defaults.streaming_address),
        }
    }

    fn to_config(&self) -> Config {
        let mut config = Config::new();
        let entries: [(&str, String); 19] = [
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
            ("streaming_address", self.streaming_address.clone()),
        ];
        for (key, value) in entries {
            // Every key here is a literal identifier and every value is bounded
            // UI text, so a rejection can only mean a programming error.
            debug_assert!(config.set(key, &value).is_ok(), "settings key {key}");
            let _ = config.set(key, &value);
        }
        config
    }

    /// Returns the stored locale, falling back to English for an unknown code.
    pub(crate) fn ui_locale(&self) -> UiLocale {
        UiLocale::from_code(&self.locale).unwrap_or(UiLocale::English)
    }

    /// Returns the selected sample rate in hertz.
    pub(crate) fn sample_rate_hz(&self) -> u32 {
        SAMPLE_RATES[self.sample_rate.min(SAMPLE_RATES.len() - 1)]
    }

    /// Returns the selected channel count.
    pub(crate) fn channel_count(&self) -> u16 {
        CHANNEL_LAYOUTS[self.channels.min(CHANNEL_LAYOUTS.len() - 1)]
    }

    /// Builds the complete token set for the selected theme, with the
    /// accessibility colour overrides applied on top.
    pub(crate) fn tokens(&self) -> ThemeTokens {
        self.tokens_for_theme(self.theme)
    }

    /// Builds a palette preview for `theme` while retaining this settings
    /// value's accessibility colour overrides.
    pub(crate) fn tokens_for_theme(&self, theme: usize) -> ThemeTokens {
        let preset = &THEMES[theme.min(THEMES.len() - 1)];
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
            streaming_address: "10.0.0.5:9000".to_owned(),
            ..AppSettings::default()
        };

        let decoded = AppSettings::from_config(&settings.to_config());

        assert_eq!(decoded, settings);
        assert_eq!(decoded.sample_rate_hz(), 96_000);
        assert_eq!(decoded.channel_count(), 1);
    }

    #[test]
    fn unreadable_and_invalid_documents_fall_back_to_defaults() {
        let missing = std::env::temp_dir().join("obs-rs-settings-does-not-exist.txt");
        assert_eq!(AppSettings::load(&missing), AppSettings::default());

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
        let path = std::env::temp_dir().join("obs-rs-settings-persist-test.txt");
        let settings = AppSettings {
            theme: 3,
            locale: "es".to_owned(),
            hotkey_start_streaming: "F9".to_owned(),
            program_border_color: "#123456".to_owned(),
            ..AppSettings::default()
        };

        settings.save(&path).expect("settings should persist");
        let reloaded = AppSettings::load(&path);

        assert_eq!(reloaded, settings);
        assert_eq!(reloaded.ui_locale(), UiLocale::Spanish);
        std::fs::remove_file(&path).expect("remove settings fixture");
    }
}
