//! The typed property form behind the source properties dialog.
//!
//! OBS describes every source with a list of typed properties and renders the
//! matching widget for each one. This module is that description: it maps a
//! source kind to its fields, reads their current values out of the settings
//! document, and writes one edited field back. The raw document stays available
//! in the dialog's advanced section, so a property this build does not model is
//! still reachable.

use obs_rs_capture::{CameraMode, CameraPixelFormat};
use obs_rs_config::Config;
use obs_rs_ui::UiLocale;
use slint::{Brush, ModelRc, SharedString, VecModel};

use crate::{settings::parse_colour, PropertyRow};

/// Widget codes shared with `source_properties_window.slint`.
const KIND_TEXT: i32 = 0;
const KIND_NUMBER: i32 = 1;
const KIND_TOGGLE: i32 = 2;
const KIND_CHOICE: i32 = 3;
const KIND_COLOR: i32 = 4;

/// Sentinel value written for "capture the whole desktop".
pub(crate) const WHOLE_DESKTOP: &str = "";

/// One property of a source kind.
struct Field {
    key: &'static str,
    label: fn(&crate::UiText) -> SharedString,
    hint: fn(&crate::UiText) -> SharedString,
    kind: FieldKind,
}

enum FieldKind {
    Text,
    /// A bounded integer, rendered as a spin box.
    Number {
        minimum: i32,
        maximum: i32,
    },
    Toggle,
    /// `(stored value, display label)` pairs, rendered as a drop-down.
    Choice(fn(&str) -> Vec<(String, String)>),
    /// A drop-down whose values come from the selected camera descriptor.
    CameraChoice(CameraChoiceFn),
    Color,
}

type CameraChoiceFn = fn(&[CameraMode], &Config) -> Vec<(String, String)>;

/// The canvas size every built-in source declares.
const SIZE_FIELDS: [Field; 2] = [
    Field {
        key: "width",
        label: |text| text.property_ui.width.clone(),
        hint: |text| text.property_ui.size_hint.clone(),
        kind: FieldKind::Number {
            minimum: 16,
            maximum: 7680,
        },
    },
    Field {
        key: "height",
        label: |text| text.property_ui.height.clone(),
        hint: |_| SharedString::new(),
        kind: FieldKind::Number {
            minimum: 16,
            maximum: 4320,
        },
    },
];

/// Returns the fields for `kind`, in the order the dialog shows them.
#[allow(
    clippy::too_many_lines,
    reason = "the typed source-property catalog keeps each built-in schema together"
)]
fn fields(kind: &str, camera_modes: &[CameraMode]) -> Vec<&'static Field> {
    static COLOR: Field = Field {
        key: "color",
        label: |text| text.property_ui.color.clone(),
        hint: |text| text.property_ui.color_hint.clone(),
        kind: FieldKind::Color,
    };
    static TEXT: Field = Field {
        key: "text",
        label: |text| text.property_ui.text.clone(),
        hint: |text| text.property_ui.text_hint.clone(),
        kind: FieldKind::Text,
    };
    static PATH: Field = Field {
        key: "path",
        label: |text| text.property_ui.path.clone(),
        hint: |text| text.property_ui.path_hint.clone(),
        kind: FieldKind::Text,
    };
    static PATHS: Field = Field {
        key: "paths",
        label: |text| text.property_ui.paths.clone(),
        hint: |text| text.property_ui.paths_hint.clone(),
        kind: FieldKind::Text,
    };
    static SLIDE_TIME: Field = Field {
        key: "slide_time_ms",
        label: |text| text.property_ui.slide_time_ms.clone(),
        hint: |text| text.property_ui.slide_time_hint.clone(),
        kind: FieldKind::Number {
            minimum: 50,
            maximum: 3_600_000,
        },
    };
    static FADE: Field = Field {
        key: "fade",
        label: |text| text.property_ui.fade.clone(),
        hint: |text| text.property_ui.fade_hint.clone(),
        kind: FieldKind::Toggle,
    };
    static TRANSITION_TIME: Field = Field {
        key: "transition_ms",
        label: |text| text.property_ui.transition_ms.clone(),
        hint: |text| text.property_ui.transition_hint.clone(),
        kind: FieldKind::Number {
            minimum: 0,
            maximum: 60_000,
        },
    };
    static LOOP: Field = Field {
        key: "loop",
        label: |text| text.property_ui.loop_label.clone(),
        hint: |text| text.property_ui.loop_hint.clone(),
        kind: FieldKind::Toggle,
    };
    static RANDOMIZE: Field = Field {
        key: "randomize",
        label: |text| text.property_ui.randomize.clone(),
        hint: |text| text.property_ui.randomize_hint.clone(),
        kind: FieldKind::Toggle,
    };
    static FONT_SIZE: Field = Field {
        key: "font_size",
        label: |text| text.property_ui.font_size.clone(),
        hint: |text| text.property_ui.font_size_hint.clone(),
        kind: FieldKind::Number {
            minimum: 1,
            maximum: 128,
        },
    };
    static DEVICE: Field = Field {
        key: "device_id",
        label: |text| text.capture_device.clone(),
        hint: |text| text.capture_device_hint.clone(),
        kind: FieldKind::Choice(device_choices),
    };
    static CAMERA_FORMAT: Field = Field {
        key: "capture_pixel_format",
        label: |text| text.property_ui.video_format.clone(),
        hint: |text| text.property_ui.camera_mode_hint.clone(),
        kind: FieldKind::CameraChoice(camera_pixel_choices),
    };
    static CAMERA_RESOLUTION: Field = Field {
        // This display key is expanded into capture_width and capture_height
        // when the choice is applied.
        key: "capture_resolution",
        label: |text| text.property_ui.resolution.clone(),
        hint: |text| text.property_ui.camera_mode_hint.clone(),
        kind: FieldKind::CameraChoice(camera_resolution_choices),
    };
    static CAMERA_FPS: Field = Field {
        key: "capture_fps",
        label: |text| text.property_ui.fps.clone(),
        hint: |text| text.property_ui.camera_mode_hint.clone(),
        kind: FieldKind::CameraChoice(camera_fps_choices),
    };
    static MONITOR: Field = Field {
        key: "monitor",
        label: |text| text.property_ui.monitor.clone(),
        hint: |text| text.property_ui.monitor_hint.clone(),
        kind: FieldKind::Choice(monitor_choices),
    };
    static DISPLAY: Field = Field {
        key: "display",
        label: |text| text.property_ui.display.clone(),
        hint: |text| text.property_ui.display_hint.clone(),
        kind: FieldKind::Text,
    };
    static WINDOW: Field = Field {
        key: "window",
        label: |text| text.property_ui.window.clone(),
        hint: |text| text.property_ui.window_hint.clone(),
        kind: FieldKind::Choice(window_choices),
    };
    static CURSOR: Field = Field {
        key: "capture_cursor",
        label: |text| text.property_ui.capture_cursor.clone(),
        hint: |_| SharedString::new(),
        kind: FieldKind::Toggle,
    };
    #[cfg(target_os = "windows")]
    static BORDER: Field = Field {
        key: "capture_border",
        label: |text| text.property_ui.capture_border.clone(),
        hint: |_| SharedString::new(),
        kind: FieldKind::Toggle,
    };

    let mut fields = match kind.trim() {
        "color_source" => vec![&COLOR],
        "image_source" => vec![&PATH],
        "image_slideshow" => vec![
            &PATHS,
            &SLIDE_TIME,
            &FADE,
            &TRANSITION_TIME,
            &LOOP,
            &RANDOMIZE,
        ],
        "text_source" => vec![&TEXT, &COLOR, &FONT_SIZE],
        #[cfg(target_os = "windows")]
        "screen_capture" => vec![&MONITOR, &DEVICE, &CURSOR, &BORDER],
        #[cfg(not(target_os = "windows"))]
        "screen_capture" => vec![&DEVICE],
        #[cfg(target_os = "windows")]
        "window_capture" => vec![&DEVICE, &CURSOR, &BORDER],
        #[cfg(not(target_os = "windows"))]
        "window_capture" => vec![&DEVICE],
        "camera_capture" => {
            let mut camera_fields = vec![&DEVICE];
            if !camera_modes.is_empty() {
                camera_fields.extend([&CAMERA_FORMAT, &CAMERA_RESOLUTION, &CAMERA_FPS]);
            }
            camera_fields
        }
        "x11_screen_capture" => vec![&MONITOR, &DISPLAY],
        "x11_window_capture" => vec![&WINDOW, &DISPLAY],
        "wayland_screen_capture" => vec![&CURSOR],
        _ => Vec::new(),
    };
    if kind.trim() != "camera_capture" {
        fields.extend(SIZE_FIELDS.iter());
    }
    fields
}

/// Builds the dialog rows for `kind` from its current settings document.
pub(crate) fn rows(kind: &str, document: &str, locale: UiLocale) -> Vec<PropertyRow> {
    let settings = Config::parse(document).unwrap_or_default();
    let camera_modes = camera_modes_for_settings(kind, &settings);
    crate::i18n::with_catalog(locale, |text| {
        fields(kind, &camera_modes)
            .into_iter()
            .map(|field| row(field, kind, &settings, &camera_modes, text))
            .collect()
    })
}

fn row(
    field: &Field,
    kind: &str,
    settings: &Config,
    camera_modes: &[CameraMode],
    text: &crate::UiText,
) -> PropertyRow {
    let stored = stored_value(field, settings, camera_modes);
    let mut row = PropertyRow {
        key: field.key.into(),
        label: (field.label)(text),
        hint: (field.hint)(text),
        kind: KIND_TEXT,
        text: stored.clone().into(),
        number: 0,
        minimum: 0,
        maximum: 0,
        toggle: false,
        choices: ModelRc::new(VecModel::from(Vec::<SharedString>::new())),
        choice_index: 0,
        swatch: Brush::default(),
    };
    match &field.kind {
        FieldKind::Text => {}
        FieldKind::Number { minimum, maximum } => {
            row.kind = KIND_NUMBER;
            row.minimum = *minimum;
            row.maximum = *maximum;
            row.number = stored
                .parse::<i32>()
                .unwrap_or(*minimum)
                .clamp(*minimum, *maximum);
        }
        FieldKind::Toggle => {
            row.kind = KIND_TOGGLE;
            // An absent flag reads as enabled: capturing the cursor is the
            // default OBS ships, and older documents predate the key.
            row.toggle = stored.parse::<bool>().unwrap_or(true);
        }
        FieldKind::Choice(choices) => {
            row.kind = KIND_CHOICE;
            let choices = choices_for_field(field.key, choices(kind), &stored);
            row.choice_index = choices
                .iter()
                .position(|(value, _)| value == &stored)
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(0);
            row.choices = ModelRc::new(VecModel::from(
                choices
                    .iter()
                    .map(|(_, label)| SharedString::from(label.as_str()))
                    .collect::<Vec<_>>(),
            ));
        }
        FieldKind::CameraChoice(choices) => {
            row.kind = KIND_CHOICE;
            let choices = choices_for_field(field.key, choices(camera_modes, settings), &stored);
            row.choice_index = choices
                .iter()
                .position(|(value, _)| value == &stored)
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(0);
            row.choices = ModelRc::new(VecModel::from(
                choices
                    .iter()
                    .map(|(_, label)| SharedString::from(label.as_str()))
                    .collect::<Vec<_>>(),
            ));
        }
        FieldKind::Color => {
            row.kind = KIND_COLOR;
            row.swatch =
                parse_colour(strip_alpha(&stored)).map_or_else(Brush::default, Brush::SolidColor);
        }
    }
    row
}

/// Writes one edited row back into the settings document.
///
/// Returns the new document, or `None` when the edit cannot be represented,
/// which leaves the previous document in place rather than corrupting it.
pub(crate) fn apply(kind: &str, document: &str, key: &str, value: &str) -> Option<String> {
    let mut settings = Config::parse(document).ok()?;
    let camera_modes = camera_modes_for_settings(kind, &settings);
    let previous_device = settings.get("device_id").map(str::to_owned);
    // A choice row reports its index; the stored value comes from the schema.
    let field = fields(kind, &camera_modes)
        .into_iter()
        .find(|field| field.key == key);
    let value = field
        .as_ref()
        .and_then(|field| match &field.kind {
            FieldKind::Choice(choices) => {
                let index = value.parse::<usize>().ok()?;
                let stored = settings.get(field.key).unwrap_or_default();
                Some(
                    choices_for_field(field.key, choices(kind), stored)
                        .get(index)?
                        .0
                        .clone(),
                )
            }
            FieldKind::CameraChoice(choices) => {
                let index = value.parse::<usize>().ok()?;
                let stored = settings.get(field.key).unwrap_or_default();
                Some(
                    choices_for_field(field.key, choices(&camera_modes, &settings), stored)
                        .get(index)?
                        .0
                        .clone(),
                )
            }
            _ => None,
        })
        .unwrap_or_else(|| value.to_owned());
    if key == "capture_resolution" && field.is_some() {
        let (width, height) = value.split_once('x')?;
        width.parse::<u32>().ok()?;
        height.parse::<u32>().ok()?;
        settings.set("capture_width", width).ok()?;
        settings.set("capture_height", height).ok()?;
    } else {
        settings.set(key, &value).ok()?;
    }
    if kind.trim() == "camera_capture"
        && key == "device_id"
        && previous_device.as_deref() != Some(value.as_str())
    {
        let new_modes = camera_modes_for_settings(kind, &settings);
        if let Some(mode) = new_modes.first().copied() {
            write_camera_mode(&mut settings, mode).ok()?;
        } else {
            for native_key in [
                "capture_width",
                "capture_height",
                "capture_fps",
                "capture_pixel_format",
            ] {
                settings.remove(native_key);
            }
        }
    }
    Some(settings.serialize())
}

fn write_camera_mode(
    settings: &mut Config,
    mode: CameraMode,
) -> Result<(), obs_rs_config::ConfigError> {
    settings.set("capture_width", &mode.width().to_string())?;
    settings.set("capture_height", &mode.height().to_string())?;
    settings.set("capture_fps", &fps_value(mode))?;
    settings.set("capture_pixel_format", mode.pixel_format().as_str())?;
    Ok(())
}

fn camera_modes_for_settings(kind: &str, settings: &Config) -> Vec<CameraMode> {
    if kind.trim() != "camera_capture" {
        return Vec::new();
    }
    settings
        .get("device_id")
        .map(crate::fixtures::camera_modes_for_device)
        .unwrap_or_default()
}

fn stored_value(field: &Field, settings: &Config, camera_modes: &[CameraMode]) -> String {
    if let Some(value) = settings.get(field.key) {
        return value.to_owned();
    }
    let Some(mode) = camera_modes.first().copied() else {
        return String::new();
    };
    match field.key {
        "capture_pixel_format" => mode.pixel_format().to_string(),
        "capture_resolution" => format!("{}x{}", mode.width(), mode.height()),
        "capture_fps" => fps_value(mode),
        _ => String::new(),
    }
}

fn camera_pixel_choices(modes: &[CameraMode], settings: &Config) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    for mode in camera_modes_for_field(modes, settings, "capture_pixel_format") {
        let value = mode.pixel_format().to_string();
        if choices.iter().all(|(candidate, _)| candidate != &value) {
            choices.push((value.clone(), value.to_ascii_uppercase()));
        }
    }
    choices
}

fn camera_resolution_choices(modes: &[CameraMode], settings: &Config) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    for mode in camera_modes_for_field(modes, settings, "capture_resolution") {
        let value = format!("{}x{}", mode.width(), mode.height());
        if choices.iter().all(|(candidate, _)| candidate != &value) {
            choices.push((value.clone(), value));
        }
    }
    choices
}

fn camera_fps_choices(modes: &[CameraMode], settings: &Config) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    for mode in camera_modes_for_field(modes, settings, "capture_fps") {
        let value = fps_value(mode);
        if choices.iter().all(|(candidate, _)| candidate != &value) {
            choices.push((value.clone(), format!("{value} FPS")));
        }
    }
    choices
}

/// Keeps camera choices dependent on the other selected native dimensions.
///
/// If an older document contains a combination the camera no longer reports,
/// the complete mode list is returned as a recovery path instead of rendering
/// an empty picker.
fn camera_modes_for_field(modes: &[CameraMode], settings: &Config, field: &str) -> Vec<CameraMode> {
    let filtered = modes
        .iter()
        .copied()
        .filter(|mode| {
            let format_matches = field == "capture_pixel_format"
                || settings
                    .get("capture_pixel_format")
                    .and_then(CameraPixelFormat::parse)
                    .is_none_or(|format| mode.pixel_format() == format);
            let resolution_matches = field == "capture_resolution"
                || match (
                    settings.get("capture_width"),
                    settings.get("capture_height"),
                ) {
                    (Some(width), Some(height)) => {
                        mode.width().to_string() == width && mode.height().to_string() == height
                    }
                    _ => true,
                };
            let fps_matches = field == "capture_fps"
                || settings
                    .get("capture_fps")
                    .is_none_or(|fps| fps_value(*mode) == fps);
            format_matches && resolution_matches && fps_matches
        })
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        modes.to_vec()
    } else {
        filtered
    }
}

fn fps_value(mode: CameraMode) -> String {
    let frame_rate = mode.frame_rate();
    if frame_rate.denominator() == 1 {
        frame_rate.numerator().to_string()
    } else {
        format!("{}/{}", frame_rate.numerator(), frame_rate.denominator())
    }
}

/// Colour settings are stored as `#RRGGBBAA`; the swatch parser reads `#RRGGBB`.
fn strip_alpha(value: &str) -> &str {
    let digits = value.trim();
    if digits.trim_start_matches('#').len() == 8 {
        &digits[..digits.len() - 2]
    } else {
        digits
    }
}

/// Capture devices for the kinds that select one.
fn device_choices(kind: &str) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    #[cfg(target_os = "windows")]
    match kind.trim() {
        "screen_capture" => choices.push((
            "wgc-screen-picker".to_owned(),
            "Primary display (automatic)".to_owned(),
        )),
        "window_capture" => choices.push((
            "wgc-window-picker".to_owned(),
            "Foreground window (automatic)".to_owned(),
        )),
        _ => {}
    }
    choices.extend(crate::capture_devices(kind));
    choices
}

/// Retains a persisted target that is not in the current discovery snapshot.
///
/// The old UI fell back to index zero in this case, which made a closed window
/// look like a different selected window. Keeping a visible unavailable entry
/// lets the runtime report the original target and lets the user deliberately
/// choose a replacement.
fn choices_for_field(
    key: &str,
    mut choices: Vec<(String, String)>,
    stored: &str,
) -> Vec<(String, String)> {
    if !stored.trim().is_empty() && choices.iter().all(|(value, _)| value != stored) {
        choices.push((stored.to_owned(), format!("Unavailable target ({stored})")));
    } else if key == "device_id" && stored.trim().is_empty() && choices.is_empty() {
        // An absent camera/device ID remains an explicit unavailable target;
        // an empty monitor/window sentinel is already represented by the
        // whole-desktop choice in its dedicated picker.
        choices.push((String::new(), "No target available".to_owned()));
    }
    choices
}

/// Windows for a window source, with the whole desktop first.
///
/// The desktop entry is what a freshly added source shows while the user is
/// still choosing, so the source is never blank and never an error.
fn window_choices(kind: &str) -> Vec<(String, String)> {
    let mut choices = vec![(
        WHOLE_DESKTOP.to_owned(),
        "Whole desktop (no window selected)".to_owned(),
    )];
    choices.extend(crate::capture_devices(kind));
    choices
}

/// Displays for a screen source, with the whole desktop first.
fn monitor_choices(_kind: &str) -> Vec<(String, String)> {
    let mut choices = vec![(
        WHOLE_DESKTOP.to_owned(),
        "All monitors (whole desktop)".to_owned(),
    )];
    choices.extend(
        crate::fixtures::screen_monitors()
            .into_iter()
            .map(|monitor| {
                let label = format!("{} ({})", monitor.name, monitor.geometry());
                (monitor.id.clone(), label)
            }),
    );
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_sources_expose_a_colour_and_a_size() {
        let document = "color = \"#405070FF\"\nheight = 360\nwidth = 640\n";

        let rows = rows("color_source", document, UiLocale::English);

        let keys = rows
            .iter()
            .map(|row| row.key.to_string())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["color", "width", "height"]);
        assert_eq!(rows[1].number, 640);
        assert_eq!(rows[1].kind, KIND_NUMBER);
        assert_eq!(rows[0].kind, KIND_COLOR);
    }

    #[test]
    fn text_sources_expose_text_colour_font_size_and_video_size() {
        let document =
            "color = \"#FFFFFFFF\"\nfont_size = 24\nheight = 360\ntext = \"OBS-RS\"\nwidth = 640\n";

        let rows = rows("text_source", document, UiLocale::English);

        let keys = rows
            .iter()
            .map(|row| row.key.to_string())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["text", "color", "font_size", "width", "height"]);
        assert_eq!(rows[0].kind, KIND_TEXT);
        assert_eq!(rows[2].kind, KIND_NUMBER);
        assert_eq!(rows[2].number, 24);
    }

    #[test]
    fn image_sources_expose_path_and_video_size() {
        let document = "height = 360\npath = \"/tmp/example.png\"\nwidth = 640\n";

        let rows = rows("image_source", document, UiLocale::English);

        let keys = rows
            .iter()
            .map(|row| row.key.to_string())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["path", "width", "height"]);
        assert_eq!(rows[0].kind, KIND_TEXT);
        assert_eq!(rows[0].text, "/tmp/example.png");
    }

    #[test]
    fn image_slideshows_expose_paths_interval_loop_and_video_size() {
        let document =
            "fade = false\nheight = 360\nloop = true\npaths = \"/tmp/a.png\\n/tmp/b.png\"\nrandomize = false\nslide_time_ms = 8000\ntransition_ms = 500\nwidth = 640\n";

        let rows = rows("image_slideshow", document, UiLocale::English);

        let keys = rows
            .iter()
            .map(|row| row.key.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "paths",
                "slide_time_ms",
                "fade",
                "transition_ms",
                "loop",
                "randomize",
                "width",
                "height"
            ]
        );
        assert_eq!(rows[1].kind, KIND_NUMBER);
        assert_eq!(rows[1].number, 8_000);
        assert!(!rows[2].toggle);
        assert!(rows[4].toggle);
    }

    #[test]
    fn an_unknown_kind_has_only_the_shared_size_fields() {
        let rows = rows("plugin_thing", "width = 2\nheight = 2\n", UiLocale::English);

        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_missing_persisted_capture_target_is_not_replaced_by_index_zero() {
        let choices = choices_for_field(
            "device_id",
            vec![("wgc-screen-1".to_owned(), "Primary".to_owned())],
            "wgc-screen-closed",
        );
        assert_eq!(
            choices,
            vec![
                ("wgc-screen-1".to_owned(), "Primary".to_owned()),
                (
                    "wgc-screen-closed".to_owned(),
                    "Unavailable target (wgc-screen-closed)".to_owned(),
                ),
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_capture_sources_expose_the_cursor_setting() {
        let screen_keys = fields("screen_capture", &[])
            .into_iter()
            .map(|field| field.key)
            .collect::<Vec<_>>();
        let window_keys = fields("window_capture", &[])
            .into_iter()
            .map(|field| field.key)
            .collect::<Vec<_>>();
        assert_eq!(
            screen_keys,
            [
                "monitor",
                "device_id",
                "capture_cursor",
                "capture_border",
                "width",
                "height"
            ]
        );
        assert_eq!(
            window_keys,
            [
                "device_id",
                "capture_cursor",
                "capture_border",
                "width",
                "height"
            ]
        );
    }

    #[test]
    fn editing_a_field_rewrites_only_that_key() {
        let document = "color = \"#405070FF\"\nheight = 360\nwidth = 640\n";

        let updated = apply("color_source", document, "width", "1280").expect("apply");

        let settings = Config::parse(&updated).expect("document");
        assert_eq!(settings.get("width"), Some("1280"));
        assert_eq!(settings.get("color"), Some("#405070FF"));
    }

    #[test]
    fn a_choice_row_stores_the_value_behind_its_index() {
        let document = "height = 360\nmonitor = \"\"\nwidth = 640\n";

        // Index 0 is always the whole desktop, whatever the host reports.
        let updated = apply("x11_screen_capture", document, "monitor", "0").expect("apply");

        let settings = Config::parse(&updated).expect("document");
        assert_eq!(settings.get("monitor"), Some(WHOLE_DESKTOP));
    }

    #[test]
    fn alpha_is_dropped_before_the_swatch_is_parsed() {
        assert_eq!(strip_alpha("#405070FF"), "#405070");
        assert_eq!(strip_alpha("#405070"), "#405070");
    }

    #[test]
    fn camera_choices_follow_the_other_selected_native_dimensions() {
        let modes = [
            CameraMode::new(
                CameraPixelFormat::Mjpeg,
                640,
                480,
                obs_rs_media::FrameRate::new(30, 1).expect("rate"),
            )
            .expect("mode"),
            CameraMode::new(
                CameraPixelFormat::Mjpeg,
                1280,
                720,
                obs_rs_media::FrameRate::new(30, 1).expect("rate"),
            )
            .expect("mode"),
            CameraMode::new(
                CameraPixelFormat::Yuyv,
                640,
                480,
                obs_rs_media::FrameRate::new(30, 1).expect("rate"),
            )
            .expect("mode"),
        ];
        let mut settings = Config::new();
        settings.set("capture_width", "1280").expect("width");
        settings.set("capture_height", "720").expect("height");
        settings.set("capture_fps", "30").expect("fps");

        assert_eq!(
            camera_pixel_choices(&modes, &settings),
            vec![("mjpeg".to_owned(), "MJPEG".to_owned())]
        );
    }
}
