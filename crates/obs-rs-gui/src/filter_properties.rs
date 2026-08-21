//! Typed property schemas for source-filter instances.
//!
//! A filter window consumes the same `PropertyRow` shape as source properties,
//! but the schema is selected by filter kind rather than source kind. Keeping
//! the schema in Rust means adding a filter kind does not require another Slint
//! component or another comma-separated serialization format.

use obs_rs_config::Config;
use obs_rs_media::ColorCorrection;
use obs_rs_ui::UiLocale;
use slint::{Brush, ModelRc, SharedString, VecModel};

use crate::PropertyRow;

const KIND_NUMBER: i32 = 1;

#[derive(Clone, Copy)]
struct Field {
    key: &'static str,
    english: &'static str,
    spanish: &'static str,
    minimum: i32,
    maximum: i32,
    default: &'static str,
}

const BRIGHTNESS: [Field; 1] = [Field {
    key: "milli",
    english: "Brightness",
    spanish: "Brillo",
    minimum: -1_000,
    maximum: 1_000,
    default: "0",
}];

const OPACITY: [Field; 1] = [Field {
    key: "value",
    english: "Opacity",
    spanish: "Opacidad",
    minimum: 0,
    maximum: 255,
    default: "255",
}];

const CROP_PAD: [Field; 4] = [
    Field {
        key: "left",
        english: "Left",
        spanish: "Izquierda",
        minimum: 0,
        maximum: 32_768,
        default: "0",
    },
    Field {
        key: "top",
        english: "Top",
        spanish: "Arriba",
        minimum: 0,
        maximum: 32_768,
        default: "0",
    },
    Field {
        key: "right",
        english: "Right",
        spanish: "Derecha",
        minimum: 0,
        maximum: 32_768,
        default: "0",
    },
    Field {
        key: "bottom",
        english: "Bottom",
        spanish: "Abajo",
        minimum: 0,
        maximum: 32_768,
        default: "0",
    },
];

const COLOR_CORRECTION: [Field; 6] = [
    Field {
        key: "gamma",
        english: "Gamma",
        spanish: "Gamma",
        minimum: ColorCorrection::MIN_GAMMA_MILLI,
        maximum: ColorCorrection::MAX_GAMMA_MILLI,
        default: "0",
    },
    Field {
        key: "contrast",
        english: "Contrast",
        spanish: "Contraste",
        minimum: ColorCorrection::MIN_CONTRAST_MILLI,
        maximum: ColorCorrection::MAX_CONTRAST_MILLI,
        default: "0",
    },
    Field {
        key: "brightness",
        english: "Brightness",
        spanish: "Brillo",
        minimum: ColorCorrection::MIN_BRIGHTNESS_MILLI,
        maximum: ColorCorrection::MAX_BRIGHTNESS_MILLI,
        default: "0",
    },
    Field {
        key: "saturation",
        english: "Saturation",
        spanish: "Saturación",
        minimum: ColorCorrection::MIN_SATURATION_MILLI,
        maximum: ColorCorrection::MAX_SATURATION_MILLI,
        default: "0",
    },
    Field {
        key: "hue_shift",
        english: "Hue shift",
        spanish: "Cambio de tono",
        minimum: ColorCorrection::MIN_HUE_SHIFT_DEGREES,
        maximum: ColorCorrection::MAX_HUE_SHIFT_DEGREES,
        default: "0",
    },
    Field {
        key: "opacity",
        english: "Opacity",
        spanish: "Opacidad",
        minimum: ColorCorrection::MIN_OPACITY_MILLI,
        maximum: ColorCorrection::MAX_OPACITY_MILLI,
        default: "1000",
    },
];

fn fields(kind: &str) -> &'static [Field] {
    match kind {
        "brightness" => &BRIGHTNESS,
        "opacity" => &OPACITY,
        "crop_pad" => &CROP_PAD,
        "color_correction" => &COLOR_CORRECTION,
        _ => &[],
    }
}

/// Returns the default settings document for a filter kind.
pub(crate) fn default_settings(kind: &str) -> Config {
    let mut settings = Config::new();
    for field in fields(kind) {
        // The static defaults are valid identifiers and values by construction.
        let _ = settings.set(field.key, field.default);
    }
    settings
}

/// Builds typed property rows for a selected filter instance.
pub(crate) fn rows(kind: &str, document: &str, locale: UiLocale) -> Vec<PropertyRow> {
    let settings = Config::parse(document).unwrap_or_else(|_| default_settings(kind));
    fields(kind)
        .iter()
        .map(|field| {
            let value = settings.get(field.key).unwrap_or(field.default);
            PropertyRow {
                key: field.key.into(),
                label: match locale {
                    UiLocale::English => field.english.into(),
                    UiLocale::Spanish => field.spanish.into(),
                },
                hint: SharedString::new(),
                kind: KIND_NUMBER,
                text: value.into(),
                number: value
                    .parse::<i32>()
                    .unwrap_or_else(|_| field.default.parse().unwrap_or(0))
                    .clamp(field.minimum, field.maximum),
                minimum: field.minimum,
                maximum: field.maximum,
                toggle: false,
                choices: ModelRc::new(VecModel::from(Vec::<SharedString>::new())),
                choice_index: 0,
                swatch: Brush::default(),
            }
        })
        .collect()
}

/// Writes one typed property into a filter settings document.
pub(crate) fn apply(kind: &str, document: &str, key: &str, value: &str) -> Option<String> {
    let field = fields(kind).iter().find(|field| field.key == key)?;
    let number = value
        .parse::<i32>()
        .ok()?
        .clamp(field.minimum, field.maximum);
    let mut settings = Config::parse(document).unwrap_or_else(|_| default_settings(kind));
    settings.set(key, &number.to_string()).ok()?;
    Some(settings.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_correction_schema_uses_shared_bounded_ranges() {
        let rows = rows("color_correction", "", UiLocale::English);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].minimum, ColorCorrection::MIN_GAMMA_MILLI);
        assert_eq!(rows[0].maximum, ColorCorrection::MAX_GAMMA_MILLI);
        assert_eq!(rows[3].maximum, ColorCorrection::MAX_SATURATION_MILLI);
        assert_eq!(rows[5].number, ColorCorrection::MAX_OPACITY_MILLI);
    }
}
