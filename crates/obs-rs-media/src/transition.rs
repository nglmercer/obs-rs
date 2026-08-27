use super::error::MediaError;

/// Smallest accepted scene-transition duration in milliseconds.
pub const MIN_TRANSITION_DURATION_MILLIS: u32 = 1;
/// Largest accepted scene-transition duration in milliseconds.
pub const MAX_TRANSITION_DURATION_MILLIS: u32 = 60_000;
/// Default scene-transition duration in milliseconds.
pub const DEFAULT_TRANSITION_DURATION_MILLIS: u32 = 300;

/// Smallest accepted Luma Wipe softness, represented in thousandths.
pub const MIN_LUMA_WIPE_SOFTNESS_MILLI: u16 = 0;
/// Largest accepted Luma Wipe softness, represented in thousandths.
pub const MAX_LUMA_WIPE_SOFTNESS_MILLI: u16 = 1_000;
/// Default Luma Wipe softness, matching OBS's default of approximately .03.
pub const DEFAULT_LUMA_WIPE_SOFTNESS_MILLI: u16 = 30;

/// Built-in Luma Wipe patterns that do not require external assets or native
/// plugin state. More OBS asset-backed patterns can be added without changing
/// the transition renderer boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LumaWipePattern {
    /// A left-to-right linear luminance ramp.
    LinearHorizontal,
    /// A top-to-bottom linear luminance ramp.
    LinearVertical,
}

impl LumaWipePattern {
    /// Returns the stable serialized pattern identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LinearHorizontal => "linear-h",
            Self::LinearVertical => "linear-v",
        }
    }

    /// Parses a stable serialized pattern identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "linear-h" => Some(Self::LinearHorizontal),
            "linear-v" => Some(Self::LinearVertical),
            _ => None,
        }
    }
}

/// Direction supported by the bounded portable slide and swipe transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlideDirection {
    /// The source moves left and the destination enters from the right.
    Left,
    /// The source moves right and the destination enters from the left.
    Right,
    /// The source moves up and the destination enters from the bottom.
    Up,
    /// The source moves down and the destination enters from the top.
    Down,
}

impl SlideDirection {
    /// Returns the stable serialized direction identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    /// Parses a stable serialized direction identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

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
    /// Slides the destination in from the configured direction.
    Slide {
        progress_milli: u16,
        direction: SlideDirection,
    },
    /// Swipes the source out while the destination fills the revealed area,
    /// or brings the destination in over the stationary source when
    /// `swipe_in` is true.
    Swipe {
        progress_milli: u16,
        direction: SlideDirection,
        swipe_in: bool,
    },
    /// Reveals the destination according to a bounded built-in luminance
    /// pattern. `softness_milli` controls the edge blend width.
    LumaWipe {
        progress_milli: u16,
        pattern: LumaWipePattern,
        invert: bool,
        softness_milli: u16,
    },
}

/// The persistent kind of a scene transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionKind {
    /// Selects the destination frame immediately.
    Cut,
    /// Linearly interpolates source and destination frames.
    CrossFade,
    /// Covers the source with a color before revealing the destination.
    FadeToColor { color: [u8; 4] },
    /// Slides the destination in from the configured direction.
    Slide { direction: SlideDirection },
    /// Swipes the source out in the configured direction, or brings the
    /// destination in when `swipe_in` is true.
    Swipe {
        direction: SlideDirection,
        swipe_in: bool,
    },
    /// Reveals the destination according to a bounded built-in luminance
    /// pattern. `softness_milli` controls the edge blend width.
    LumaWipe {
        pattern: LumaWipePattern,
        invert: bool,
        softness_milli: u16,
    },
}

/// A validated transition policy that can be persisted on a scene.
///
/// [`FrameTransition`] is deliberately a render-time sample containing a
/// progress value. This type contains the stable kind and duration that a
/// project or UI command needs to retain, and creates a sample only at the
/// compositor boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionSpec {
    kind: TransitionKind,
    duration_millis: u32,
}

impl TransitionSpec {
    /// Creates a validated persistent transition policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn new(kind: TransitionKind, duration_millis: u32) -> Result<Self, MediaError> {
        if duration_millis < MIN_TRANSITION_DURATION_MILLIS
            || duration_millis > MAX_TRANSITION_DURATION_MILLIS
        {
            return Err(MediaError::InvalidTransitionDuration { duration_millis });
        }
        if let TransitionKind::LumaWipe { softness_milli, .. } = kind {
            if softness_milli > MAX_LUMA_WIPE_SOFTNESS_MILLI {
                return Err(MediaError::InvalidLumaWipeSoftness { softness_milli });
            }
        }
        Ok(Self {
            kind,
            duration_millis,
        })
    }

    /// Returns the default cut policy.
    #[must_use]
    pub const fn cut() -> Self {
        Self {
            kind: TransitionKind::Cut,
            duration_millis: DEFAULT_TRANSITION_DURATION_MILLIS,
        }
    }

    /// Creates a validated cross-fade policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn cross_fade(duration_millis: u32) -> Result<Self, MediaError> {
        Self::new(TransitionKind::CrossFade, duration_millis)
    }

    /// Creates a validated fade-to-color policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn fade_to_color(duration_millis: u32, color: [u8; 4]) -> Result<Self, MediaError> {
        Self::new(TransitionKind::FadeToColor { color }, duration_millis)
    }

    /// Creates the bounded left-to-right slide policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn slide_left(duration_millis: u32) -> Result<Self, MediaError> {
        Self::new(
            TransitionKind::Slide {
                direction: SlideDirection::Left,
            },
            duration_millis,
        )
    }

    /// Creates the bounded left-direction swipe policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn swipe_left(duration_millis: u32) -> Result<Self, MediaError> {
        Self::new(
            TransitionKind::Swipe {
                direction: SlideDirection::Left,
                swipe_in: false,
            },
            duration_millis,
        )
    }

    /// Creates the bounded left-direction swipe-in policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn swipe_in_left(duration_millis: u32) -> Result<Self, MediaError> {
        Self::new(
            TransitionKind::Swipe {
                direction: SlideDirection::Left,
                swipe_in: true,
            },
            duration_millis,
        )
    }

    /// Creates a validated portable Luma Wipe policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] or
    /// [`MediaError::InvalidLumaWipeSoftness`] when the policy is outside its
    /// bounded range.
    pub const fn luma_wipe(
        duration_millis: u32,
        pattern: LumaWipePattern,
        invert: bool,
        softness_milli: u16,
    ) -> Result<Self, MediaError> {
        Self::new(
            TransitionKind::LumaWipe {
                pattern,
                invert,
                softness_milli,
            },
            duration_millis,
        )
    }

    /// Converts one render-time sample into a persistent policy.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransitionDuration`] when the duration is
    /// outside the bounded scene-transition range.
    pub const fn from_frame_transition(
        transition: FrameTransition,
        duration_millis: u32,
    ) -> Result<Self, MediaError> {
        let kind = match transition {
            FrameTransition::Cut => TransitionKind::Cut,
            FrameTransition::CrossFade { .. } => TransitionKind::CrossFade,
            FrameTransition::FadeToColor { color, .. } => TransitionKind::FadeToColor { color },
            FrameTransition::Slide { direction, .. } => TransitionKind::Slide { direction },
            FrameTransition::Swipe {
                direction,
                swipe_in,
                ..
            } => TransitionKind::Swipe {
                direction,
                swipe_in,
            },
            FrameTransition::LumaWipe {
                pattern,
                invert,
                softness_milli,
                ..
            } => TransitionKind::LumaWipe {
                pattern,
                invert,
                softness_milli,
            },
        };
        Self::new(kind, duration_millis)
    }

    /// Returns the stable transition kind.
    #[must_use]
    pub const fn kind(self) -> TransitionKind {
        self.kind
    }

    /// Returns the validated duration in milliseconds.
    #[must_use]
    pub const fn duration_millis(self) -> u32 {
        self.duration_millis
    }

    /// Creates a render-time sample at a validated progress value.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when the progress value is
    /// outside the inclusive 0..=1000 range.
    pub const fn at_progress(self, progress_milli: u16) -> Result<FrameTransition, MediaError> {
        match self.kind {
            TransitionKind::Cut => Ok(FrameTransition::Cut),
            TransitionKind::CrossFade => FrameTransition::cross_fade(progress_milli),
            TransitionKind::FadeToColor { color } => {
                FrameTransition::fade_to_color(progress_milli, color)
            }
            TransitionKind::Slide { direction } => {
                FrameTransition::slide(progress_milli, direction)
            }
            TransitionKind::Swipe {
                direction,
                swipe_in,
            } => FrameTransition::swipe_with_mode(progress_milli, direction, swipe_in),
            TransitionKind::LumaWipe {
                pattern,
                invert,
                softness_milli,
            } => FrameTransition::luma_wipe(progress_milli, pattern, invert, softness_milli),
        }
    }
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

    /// Creates a slide at a validated progress value.
    ///
    /// `0` selects the source frame and `1000` selects the destination frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is
    /// greater than `1000`.
    pub const fn slide(progress_milli: u16, direction: SlideDirection) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        Ok(Self::Slide {
            progress_milli,
            direction,
        })
    }

    /// Creates a swipe at a validated progress value.
    ///
    /// `0` selects the source frame and `1000` selects the destination frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is
    /// greater than `1000`.
    pub const fn swipe(progress_milli: u16, direction: SlideDirection) -> Result<Self, MediaError> {
        Self::swipe_with_mode(progress_milli, direction, false)
    }

    /// Creates a swipe sample with an explicit incoming/outgoing mode.
    ///
    /// `swipe_in = false` moves the source out and leaves the destination
    /// stationary. `swipe_in = true` moves the destination in over the source.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is
    /// greater than `1000`.
    pub const fn swipe_with_mode(
        progress_milli: u16,
        direction: SlideDirection,
        swipe_in: bool,
    ) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        Ok(Self::Swipe {
            progress_milli,
            direction,
            swipe_in,
        })
    }

    /// Creates an incoming swipe sample.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is
    /// greater than `1000`.
    pub const fn swipe_in(
        progress_milli: u16,
        direction: SlideDirection,
    ) -> Result<Self, MediaError> {
        Self::swipe_with_mode(progress_milli, direction, true)
    }

    /// Creates a Luma Wipe sample at a validated progress value.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] or
    /// [`MediaError::InvalidLumaWipeSoftness`] when a sample value is outside
    /// its bounded range.
    pub const fn luma_wipe(
        progress_milli: u16,
        pattern: LumaWipePattern,
        invert: bool,
        softness_milli: u16,
    ) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        if softness_milli > MAX_LUMA_WIPE_SOFTNESS_MILLI {
            return Err(MediaError::InvalidLumaWipeSoftness { softness_milli });
        }
        Ok(Self::LumaWipe {
            progress_milli,
            pattern,
            invert,
            softness_milli,
        })
    }
}
