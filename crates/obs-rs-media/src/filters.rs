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
}
