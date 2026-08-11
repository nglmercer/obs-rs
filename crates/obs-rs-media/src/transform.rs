use super::error::MediaError;
/// A deterministic nearest-neighbor scene transform.
///
/// Scale values use thousandths: `1000` is 100%, `2000` is 200%. Translation is
/// expressed in output pixels and the transform is anchored at the top-left corner.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrameTransform {
    pub(crate) scale_x_milli: u32,
    pub(crate) scale_y_milli: u32,
    pub(crate) translate_x: i32,
    pub(crate) translate_y: i32,
    pub(crate) flip_x: bool,
    pub(crate) flip_y: bool,
    pub(crate) opacity: u8,
    /// Source pixels removed from each edge before scaling.
    ///
    /// Cropping is expressed against the source rather than the output because
    /// that is what stays meaningful when the scale changes: "trim 40 pixels of
    /// letterbox off this capture" is a property of the capture, not of how
    /// large it happens to be drawn.
    pub(crate) crop_left: u32,
    pub(crate) crop_top: u32,
    pub(crate) crop_right: u32,
    pub(crate) crop_bottom: u32,
}
impl FrameTransform {
    /// The identity transform.
    pub const IDENTITY: Self = Self {
        scale_x_milli: 1_000,
        scale_y_milli: 1_000,
        translate_x: 0,
        translate_y: 0,
        flip_x: false,
        flip_y: false,
        opacity: u8::MAX,
        crop_left: 0,
        crop_top: 0,
        crop_right: 0,
        crop_bottom: 0,
    };

    /// Maximum supported scale in thousandths.
    pub const MAX_SCALE_MILLI: u32 = 100_000;

    /// Largest crop accepted on one edge, in source pixels.
    ///
    /// A crop wider than any supported frame can only be a mistake, and
    /// bounding it here keeps the arithmetic in the resampler in range.
    pub const MAX_CROP: u32 = 32_768;

    /// Creates a transform with validated non-zero scales.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] for a zero or excessive scale.
    pub const fn new(
        horizontal_scale_milli: u32,
        vertical_scale_milli: u32,
        translate_x: i32,
        translate_y: i32,
        flip_x: bool,
        flip_y: bool,
        opacity: u8,
    ) -> Result<Self, MediaError> {
        if horizontal_scale_milli == 0
            || vertical_scale_milli == 0
            || horizontal_scale_milli > Self::MAX_SCALE_MILLI
            || vertical_scale_milli > Self::MAX_SCALE_MILLI
        {
            return Err(MediaError::InvalidTransform);
        }
        Ok(Self {
            scale_x_milli: horizontal_scale_milli,
            scale_y_milli: vertical_scale_milli,
            translate_x,
            translate_y,
            flip_x,
            flip_y,
            opacity,
            crop_left: 0,
            crop_top: 0,
            crop_right: 0,
            crop_bottom: 0,
        })
    }

    /// Returns this transform with source edges cropped away.
    ///
    /// A crop that consumes the whole frame is rejected here rather than left
    /// to produce an empty layer at render time, so an impossible crop is
    /// visible when it is entered instead of when it is drawn.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] when an edge exceeds
    /// [`Self::MAX_CROP`] or when opposite edges together leave nothing.
    pub const fn with_crop(
        self,
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
    ) -> Result<Self, MediaError> {
        if left > Self::MAX_CROP
            || top > Self::MAX_CROP
            || right > Self::MAX_CROP
            || bottom > Self::MAX_CROP
        {
            return Err(MediaError::InvalidTransform);
        }
        Ok(Self {
            crop_left: left,
            crop_top: top,
            crop_right: right,
            crop_bottom: bottom,
            ..self
        })
    }

    /// Returns the source pixels trimmed from the left edge.
    #[must_use]
    pub const fn crop_left(self) -> u32 {
        self.crop_left
    }

    /// Returns the source pixels trimmed from the top edge.
    #[must_use]
    pub const fn crop_top(self) -> u32 {
        self.crop_top
    }

    /// Returns the source pixels trimmed from the right edge.
    #[must_use]
    pub const fn crop_right(self) -> u32 {
        self.crop_right
    }

    /// Returns the source pixels trimmed from the bottom edge.
    #[must_use]
    pub const fn crop_bottom(self) -> u32 {
        self.crop_bottom
    }

    /// Returns whether any edge is cropped.
    #[must_use]
    pub const fn is_cropped(self) -> bool {
        self.crop_left != 0 || self.crop_top != 0 || self.crop_right != 0 || self.crop_bottom != 0
    }

    /// Returns the horizontal scale in thousandths.
    #[must_use]
    pub const fn scale_x_milli(self) -> u32 {
        self.scale_x_milli
    }

    /// Returns the vertical scale in thousandths.
    #[must_use]
    pub const fn scale_y_milli(self) -> u32 {
        self.scale_y_milli
    }

    /// Returns the horizontal output translation in pixels.
    #[must_use]
    pub const fn translate_x(self) -> i32 {
        self.translate_x
    }

    /// Returns the vertical output translation in pixels.
    #[must_use]
    pub const fn translate_y(self) -> i32 {
        self.translate_y
    }

    /// Returns whether the source is mirrored horizontally.
    #[must_use]
    pub const fn flip_x(self) -> bool {
        self.flip_x
    }

    /// Returns whether the source is mirrored vertically.
    #[must_use]
    pub const fn flip_y(self) -> bool {
        self.flip_y
    }

    /// Returns the alpha multiplier.
    #[must_use]
    pub const fn opacity(self) -> u8 {
        self.opacity
    }
}
