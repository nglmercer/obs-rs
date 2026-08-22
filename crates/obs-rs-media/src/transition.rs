use super::error::MediaError;

/// Parses a bounded `#RRGGBB` or `#RRGGBBAA` color into RGBA8.
///
/// Six-digit colors receive an opaque alpha channel. The helper is shared by
/// console and desktop frontends so both entry points apply the same color
/// syntax and length bound.
#[must_use]
pub fn parse_rgba8_hex(value: &str) -> Option<[u8; 4]> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 && value.len() != 8 {
        return None;
    }
    let mut color = [0_u8; 4];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        color[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    if value.len() == 6 {
        color[3] = 255;
    }
    Some(color)
}
/// A video transition applied between two frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameTransition {
    /// Selects the destination frame immediately.
    Cut,
    /// Linearly interpolates source and destination bytes from 0 to 1000.
    CrossFade { progress_milli: u16 },
    /// Fades from the source frame to a solid color and then into the
    /// destination frame over progress 0..=1000.
    ///
    /// Progress 500 is the fully covered color frame. The color is RGBA8 so
    /// the portable reference can represent transparent transition colors as
    /// well as OBS's usual opaque color picker value.
    FadeToColor { progress_milli: u16, color: [u8; 4] },
}

impl FrameTransition {
    /// Creates a cross-fade at a validated progress value.
    ///
    /// `0` selects the source frame and `1000` selects the destination frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is greater
    /// than `1000`.
    pub const fn cross_fade(progress_milli: u16) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        Ok(Self::CrossFade { progress_milli })
    }

    /// Creates a fade-to-color transition at a validated progress value.
    ///
    /// Progress `0` selects the source frame, `500` selects the solid color,
    /// and `1000` selects the destination frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is
    /// greater than `1000`.
    pub const fn fade_to_color(progress_milli: u16, color: [u8; 4]) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        Ok(Self::FadeToColor {
            progress_milli,
            color,
        })
    }
}
