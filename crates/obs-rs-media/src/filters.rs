/// Fixed-point parameters for the portable Color Correction filter slice.
///
/// The scalar fields use thousandths (`1000` is `1.0`) so project values stay
/// deterministic without putting floating-point state in the project model.
/// Hue is stored in whole degrees and opacity uses thousandths. The ranges
/// match the OBS v2 filter's six numeric controls; sub-degree hue editing,
/// color multiply/add, and HDR color-space behavior are separate capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorCorrection {
    gamma_milli: i32,
    contrast_milli: i32,
    brightness_milli: i32,
    saturation_milli: i32,
    hue_shift_degrees: i32,
    opacity_milli: i32,
}

/// Fixed-point RGB color wash parameters from OBS's Color Correction filter.
///
/// OBS stores these as two color properties (`color_multiply` and
/// `color_add`). The portable RGBA8 contract keeps their RGB channels
/// explicit so validation and project serialization never depend on a packed
/// platform color integer. Alpha remains owned by the source and by the
/// separate opacity control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorMultiplyAdd {
    multiply: [u8; 3],
    add: [u8; 3],
}

impl ColorMultiplyAdd {
    /// Creates a bounded RGB multiply/add operation.
    #[must_use]
    pub const fn new(multiply: [u8; 3], add: [u8; 3]) -> Self {
        Self { multiply, add }
    }

    /// Returns the per-channel multiplier, where 255 is identity.
    #[must_use]
    pub const fn multiply(self) -> [u8; 3] {
        self.multiply
    }

    /// Returns the per-channel additive color component.
    #[must_use]
    pub const fn add(self) -> [u8; 3] {
        self.add
    }
}

impl ColorCorrection {
    /// Smallest gamma value, in thousandths.
    pub const MIN_GAMMA_MILLI: i32 = -3_000;
    /// Largest gamma value, in thousandths.
    pub const MAX_GAMMA_MILLI: i32 = 3_000;
    /// Smallest contrast value, in thousandths.
    pub const MIN_CONTRAST_MILLI: i32 = -4_000;
    /// Largest contrast value, in thousandths.
    pub const MAX_CONTRAST_MILLI: i32 = 4_000;
    /// Smallest brightness value, in thousandths.
    pub const MIN_BRIGHTNESS_MILLI: i32 = -1_000;
    /// Largest brightness value, in thousandths.
    pub const MAX_BRIGHTNESS_MILLI: i32 = 1_000;
    /// Smallest saturation value, in thousandths.
    pub const MIN_SATURATION_MILLI: i32 = -1_000;
    /// Largest saturation value, in thousandths.
    pub const MAX_SATURATION_MILLI: i32 = 5_000;
    /// Smallest hue shift, in degrees.
    pub const MIN_HUE_SHIFT_DEGREES: i32 = -180;
    /// Largest hue shift, in degrees.
    pub const MAX_HUE_SHIFT_DEGREES: i32 = 180;
    /// Smallest opacity value, in thousandths.
    pub const MIN_OPACITY_MILLI: i32 = 0;
    /// Largest opacity value, in thousandths.
    pub const MAX_OPACITY_MILLI: i32 = 1_000;

    /// Creates validated fixed-point color-correction parameters.
    #[must_use]
    pub const fn new(
        gamma_milli: i32,
        contrast_milli: i32,
        brightness_milli: i32,
        saturation_milli: i32,
        hue_shift_degrees: i32,
        opacity_milli: i32,
    ) -> Option<Self> {
        if gamma_milli < Self::MIN_GAMMA_MILLI
            || gamma_milli > Self::MAX_GAMMA_MILLI
            || contrast_milli < Self::MIN_CONTRAST_MILLI
            || contrast_milli > Self::MAX_CONTRAST_MILLI
            || brightness_milli < Self::MIN_BRIGHTNESS_MILLI
            || brightness_milli > Self::MAX_BRIGHTNESS_MILLI
            || saturation_milli < Self::MIN_SATURATION_MILLI
            || saturation_milli > Self::MAX_SATURATION_MILLI
            || hue_shift_degrees < Self::MIN_HUE_SHIFT_DEGREES
            || hue_shift_degrees > Self::MAX_HUE_SHIFT_DEGREES
            || opacity_milli < Self::MIN_OPACITY_MILLI
            || opacity_milli > Self::MAX_OPACITY_MILLI
        {
            return None;
        }
        Some(Self {
            gamma_milli,
            contrast_milli,
            brightness_milli,
            saturation_milli,
            hue_shift_degrees,
            opacity_milli,
        })
    }

    /// Returns the gamma value in thousandths.
    #[must_use]
    pub const fn gamma_milli(self) -> i32 {
        self.gamma_milli
    }

    /// Returns the contrast value in thousandths.
    #[must_use]
    pub const fn contrast_milli(self) -> i32 {
        self.contrast_milli
    }

    /// Returns the brightness value in thousandths.
    #[must_use]
    pub const fn brightness_milli(self) -> i32 {
        self.brightness_milli
    }

    /// Returns the saturation value in thousandths.
    #[must_use]
    pub const fn saturation_milli(self) -> i32 {
        self.saturation_milli
    }

    /// Returns the hue shift in degrees.
    #[must_use]
    pub const fn hue_shift_degrees(self) -> i32 {
        self.hue_shift_degrees
    }

    /// Returns the opacity value in thousandths.
    #[must_use]
    pub const fn opacity_milli(self) -> i32 {
        self.opacity_milli
    }
}

/// Fixed-point parameters for the portable Luma Key filter slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LumaKey {
    max: i32,
    min: i32,
    max_smooth: i32,
    min_smooth: i32,
}

impl LumaKey {
    /// Smallest luma threshold, in thousandths.
    pub const MIN_LUMA_MILLI: i32 = 0;
    /// Largest luma threshold, in thousandths.
    pub const MAX_LUMA_MILLI: i32 = 1_000;
    /// Smallest smoothness width, in thousandths.
    pub const MIN_SMOOTH_MILLI: i32 = 0;
    /// Largest smoothness width, in thousandths.
    pub const MAX_SMOOTH_MILLI: i32 = 1_000;

    /// Creates bounded luma thresholds and transition widths.
    #[must_use]
    pub const fn new(
        luma_max_milli: i32,
        luma_min_milli: i32,
        luma_max_smooth_milli: i32,
        luma_min_smooth_milli: i32,
    ) -> Option<Self> {
        if luma_max_milli < Self::MIN_LUMA_MILLI
            || luma_max_milli > Self::MAX_LUMA_MILLI
            || luma_min_milli < Self::MIN_LUMA_MILLI
            || luma_min_milli > Self::MAX_LUMA_MILLI
            || luma_max_smooth_milli < Self::MIN_SMOOTH_MILLI
            || luma_max_smooth_milli > Self::MAX_SMOOTH_MILLI
            || luma_min_smooth_milli < Self::MIN_SMOOTH_MILLI
            || luma_min_smooth_milli > Self::MAX_SMOOTH_MILLI
        {
            return None;
        }
        Some(Self {
            max: luma_max_milli,
            min: luma_min_milli,
            max_smooth: luma_max_smooth_milli,
            min_smooth: luma_min_smooth_milli,
        })
    }

    /// Returns the upper luma threshold in thousandths.
    #[must_use]
    pub const fn luma_max_milli(self) -> i32 {
        self.max
    }

    /// Returns the lower luma threshold in thousandths.
    #[must_use]
    pub const fn luma_min_milli(self) -> i32 {
        self.min
    }

    /// Returns the upper transition width in thousandths.
    #[must_use]
    pub const fn luma_max_smooth_milli(self) -> i32 {
        self.max_smooth
    }

    /// Returns the lower transition width in thousandths.
    #[must_use]
    pub const fn luma_min_smooth_milli(self) -> i32 {
        self.min_smooth
    }
}

/// Fixed-point parameters for the portable RGB-distance Color Key slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorKey {
    key_red: u8,
    key_green: u8,
    key_blue: u8,
    similarity_milli: i32,
    smoothness_milli: i32,
}

impl ColorKey {
    /// Smallest similarity threshold, in thousandths.
    pub const MIN_SIMILARITY_MILLI: i32 = 0;
    /// Largest similarity threshold, in thousandths.
    pub const MAX_SIMILARITY_MILLI: i32 = 1_000;
    /// Smallest smoothness threshold, in thousandths.
    pub const MIN_SMOOTHNESS_MILLI: i32 = 0;
    /// Largest smoothness threshold, in thousandths.
    pub const MAX_SMOOTHNESS_MILLI: i32 = 1_000;

    /// Creates bounded RGB-distance key parameters.
    #[must_use]
    pub const fn new(
        key_red: u8,
        key_green: u8,
        key_blue: u8,
        similarity_milli: i32,
        smoothness_milli: i32,
    ) -> Option<Self> {
        if similarity_milli < Self::MIN_SIMILARITY_MILLI
            || similarity_milli > Self::MAX_SIMILARITY_MILLI
            || smoothness_milli < Self::MIN_SMOOTHNESS_MILLI
            || smoothness_milli > Self::MAX_SMOOTHNESS_MILLI
        {
            return None;
        }
        Some(Self {
            key_red,
            key_green,
            key_blue,
            similarity_milli,
            smoothness_milli,
        })
    }

    /// Returns the red component of the key color.
    #[must_use]
    pub const fn key_red(self) -> u8 {
        self.key_red
    }

    /// Returns the green component of the key color.
    #[must_use]
    pub const fn key_green(self) -> u8 {
        self.key_green
    }

    /// Returns the blue component of the key color.
    #[must_use]
    pub const fn key_blue(self) -> u8 {
        self.key_blue
    }

    /// Returns the similarity threshold in thousandths of the maximum RGB distance.
    #[must_use]
    pub const fn similarity_milli(self) -> i32 {
        self.similarity_milli
    }

    /// Returns the smoothness threshold in thousandths of the maximum RGB distance.
    #[must_use]
    pub const fn smoothness_milli(self) -> i32 {
        self.smoothness_milli
    }
}

/// Fixed-point parameters for the portable YCbCr-distance Chroma Key slice.
///
/// OBS's production filter also owns color-space negotiation, four-neighbour
/// box filtering, opacity/contrast/brightness/gamma controls, and selectable
/// named key colors. This value type deliberately covers the bounded key,
/// feather, and spill controls that the current RGBA reference compositor can
/// represent without adding a second color-management state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChromaKey {
    key_red: u8,
    key_green: u8,
    key_blue: u8,
    similarity_milli: i32,
    smoothness_milli: i32,
    spill_milli: i32,
}

impl ChromaKey {
    /// Smallest similarity threshold, in thousandths.
    pub const MIN_SIMILARITY_MILLI: i32 = 1;
    /// Largest similarity threshold, in thousandths.
    pub const MAX_SIMILARITY_MILLI: i32 = 1_000;
    /// Smallest feather threshold, in thousandths.
    pub const MIN_SMOOTHNESS_MILLI: i32 = 1;
    /// Largest feather threshold, in thousandths.
    pub const MAX_SMOOTHNESS_MILLI: i32 = 1_000;
    /// Smallest spill-reduction threshold, in thousandths.
    pub const MIN_SPILL_MILLI: i32 = 1;
    /// Largest spill-reduction threshold, in thousandths.
    pub const MAX_SPILL_MILLI: i32 = 1_000;

    /// Creates bounded YCbCr key, feather, and spill parameters.
    #[must_use]
    pub const fn new(
        key_red: u8,
        key_green: u8,
        key_blue: u8,
        similarity_milli: i32,
        smoothness_milli: i32,
        spill_milli: i32,
    ) -> Option<Self> {
        if similarity_milli < Self::MIN_SIMILARITY_MILLI
            || similarity_milli > Self::MAX_SIMILARITY_MILLI
            || smoothness_milli < Self::MIN_SMOOTHNESS_MILLI
            || smoothness_milli > Self::MAX_SMOOTHNESS_MILLI
            || spill_milli < Self::MIN_SPILL_MILLI
            || spill_milli > Self::MAX_SPILL_MILLI
        {
            return None;
        }
        Some(Self {
            key_red,
            key_green,
            key_blue,
            similarity_milli,
            smoothness_milli,
            spill_milli,
        })
    }

    /// Returns the red component of the key color.
    #[must_use]
    pub const fn key_red(self) -> u8 {
        self.key_red
    }

    /// Returns the green component of the key color.
    #[must_use]
    pub const fn key_green(self) -> u8 {
        self.key_green
    }

    /// Returns the blue component of the key color.
    #[must_use]
    pub const fn key_blue(self) -> u8 {
        self.key_blue
    }

    /// Returns the similarity threshold in thousandths.
    #[must_use]
    pub const fn similarity_milli(self) -> i32 {
        self.similarity_milli
    }

    /// Returns the feather threshold in thousandths.
    #[must_use]
    pub const fn smoothness_milli(self) -> i32 {
        self.smoothness_milli
    }

    /// Returns the spill-reduction threshold in thousandths.
    #[must_use]
    pub const fn spill_milli(self) -> i32 {
        self.spill_milli
    }
}

/// Smallest supported horizontal or vertical Scroll speed, in pixels/second.
pub const MIN_SCROLL_SPEED: i16 = -500;
/// Largest supported horizontal or vertical Scroll speed, in pixels/second.
pub const MAX_SCROLL_SPEED: i16 = 500;

/// A bounded source-level Render Delay. The runtime owns its timestamp history
/// before handing the resulting frame to CPU or WGPU pixel filters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderDelay {
    /// Delay in milliseconds, bounded by the portable OBS Render Delay range.
    pub milliseconds: u32,
}

/// A deterministic CPU filter applied after a scene-item transform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameFilter {
    /// Converts RGB to a luma approximation while preserving alpha.
    Grayscale,
    /// Multiplies RGB by `milli / 1000`, clamping to the byte range.
    Brightness { milli: i16 },
    /// Multiplies alpha by the supplied byte factor.
    Opacity(u8),
    /// Clears source edges while retaining the frame geometry.
    ///
    /// The current portable filter slice expresses the OBS Crop/Pad property
    /// as non-negative edge crops. A future pad mode can extend this value
    /// without changing the project filter schema.
    CropPad {
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
    },
    /// Applies the portable six-control Color Correction effect.
    ColorCorrection(ColorCorrection),
    /// Applies OBS Color Correction's RGB multiply/add color wash controls.
    ColorMultiplyAdd(ColorMultiplyAdd),
    /// Makes pixels outside a bounded luminance interval transparent.
    LumaKey(LumaKey),
    /// Makes pixels within a bounded RGB distance transparent.
    ColorKey(ColorKey),
    /// Makes pixels near a bounded YCbCr chroma distance transparent.
    ChromaKey(ChromaKey),
    /// Applies the bounded OBS 3x3 sharpen kernel in thousandths.
    Sharpen { milli: u16 },
    /// Scrolls a source in pixels per second, wrapping when `looped` is true.
    ///
    /// The filter keeps the frame geometry unchanged. Width/height limiting
    /// from OBS's full Scroll filter remains a separate capability.
    Scroll {
        speed_x: i16,
        speed_y: i16,
        looped: bool,
    },
    /// Delays a source frame in the runtime-owned timestamp history.
    ///
    /// This is intentionally not a pixel operation; the compositor resolves it
    /// before exposing the remaining ordered chain to CPU or WGPU composition.
    RenderDelay(RenderDelay),
}
