use super::error::MediaError;
/// A deterministic nearest-neighbor scene transform.
///
/// Scale values use thousandths: `1000` is 100%, `2000` is 200%. Translation is
/// expressed in output pixels and the transform is anchored at the top-left corner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTransform {
    pub(crate) scale_x_milli: u32,
    pub(crate) scale_y_milli: u32,
    pub(crate) translate_x: i32,
    pub(crate) translate_y: i32,
    pub(crate) flip_x: bool,
    pub(crate) flip_y: bool,
    pub(crate) opacity: u8,
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
    };

    /// Maximum supported scale in thousandths.
    pub const MAX_SCALE_MILLI: u32 = 100_000;

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
        })
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
