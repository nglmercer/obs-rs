#[allow(
    clippy::wildcard_imports,
    reason = "settings submodules share the validated settings namespace"
)]
use super::*;

/// An sRGB colour as `[red, green, blue]`.
pub(super) type Rgb = [u8; 3];

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
/// The colour scheme a theme produces once its style is applied.
///
/// Styles transform the preset rather than replacing it, so a new theme is
/// automatically available in all three styles.
pub(crate) struct StyledPreset {
    pub(crate) window_bg: Rgb,
    pub(crate) panel_bg: Rgb,
    pub(crate) header_bg: Rgb,
    pub(crate) header_active_bg: Rgb,
    pub(crate) border: Rgb,
    pub(crate) border_strong: Rgb,
    pub(crate) row_bg: Rgb,
    pub(crate) row_selected_bg: Rgb,
    pub(crate) control_bg: Rgb,
    pub(crate) text: Rgb,
    pub(crate) text_strong: Rgb,
    pub(crate) text_muted: Rgb,
    pub(crate) accent: Rgb,
    pub(crate) canvas_bg: Rgb,
}

pub(super) fn styled(preset: &ThemePreset, style: UiStyle) -> StyledPreset {
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
pub(super) fn mix(colour: Rgb, other: Rgb, amount: u8) -> Rgb {
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
pub(super) fn lighten(colour: Rgb, amount: u8) -> Rgb {
    mix(colour, [0xFF, 0xFF, 0xFF], amount)
}
pub(super) fn brush(rgb: Rgb) -> Brush {
    Brush::SolidColor(colour(rgb))
}

pub(super) fn colour([red, green, blue]: Rgb) -> Color {
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
