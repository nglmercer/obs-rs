use super::error::MediaError;
/// A deterministic nearest-neighbor scene transform.
///
/// Scale values use thousandths: `1000` is 100%, `2000` is 200%. Translation is
/// expressed in output pixels and the transform is anchored at the top-left corner.
/// Rotation is stored in thousandths of a degree and is applied around the
/// centre of the visible, scaled source rectangle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrameTransform {
    pub(crate) scale_x_milli: u32,
    pub(crate) scale_y_milli: u32,
    pub(crate) translate_x: i32,
    pub(crate) translate_y: i32,
    pub(crate) flip_x: bool,
    pub(crate) flip_y: bool,
    pub(crate) opacity: u8,
    pub(crate) rotation_milli_degrees: i32,
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
        rotation_milli_degrees: 0,
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

    /// Largest supported absolute rotation, in thousandths of a degree.
    pub const MAX_ROTATION_MILLI_DEGREES: i32 = 360_000;

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
            rotation_milli_degrees: 0,
            crop_left: 0,
            crop_top: 0,
            crop_right: 0,
            crop_bottom: 0,
        })
    }

    /// Returns this transform with a validated rotation around the visible
    /// source rectangle's centre.
    ///
    /// The fixed-point representation keeps project files deterministic while
    /// still allowing sub-degree values from non-GUI callers.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] when the absolute rotation is
    /// greater than [`Self::MAX_ROTATION_MILLI_DEGREES`].
    pub const fn with_rotation_milli_degrees(
        self,
        rotation_milli_degrees: i32,
    ) -> Result<Self, MediaError> {
        if rotation_milli_degrees.unsigned_abs() > Self::MAX_ROTATION_MILLI_DEGREES.unsigned_abs() {
            return Err(MediaError::InvalidTransform);
        }
        Ok(Self {
            rotation_milli_degrees,
            ..self
        })
    }

    /// Returns this transform with an integer rotation in degrees.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] when the degree value cannot
    /// be represented in the fixed-point range.
    pub const fn with_rotation_degrees(self, rotation_degrees: i32) -> Result<Self, MediaError> {
        let Some(rotation_milli_degrees) = rotation_degrees.checked_mul(1_000) else {
            return Err(MediaError::InvalidTransform);
        };
        self.with_rotation_milli_degrees(rotation_milli_degrees)
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

    /// Returns the rotation in thousandths of a degree.
    #[must_use]
    pub const fn rotation_milli_degrees(self) -> i32 {
        self.rotation_milli_degrees
    }

    /// Returns the rotation rounded toward zero to whole degrees.
    #[must_use]
    pub const fn rotation_degrees(self) -> i32 {
        self.rotation_milli_degrees / 1_000
    }

    /// Returns whether this transform rotates the visible source.
    #[must_use]
    pub const fn is_rotated(self) -> bool {
        self.rotation_milli_degrees != 0
    }

    /// Composes a child transform with a parent transform for nested scenes.
    ///
    /// The compact scene representation can exactly flatten the axis-aligned
    /// scale/translation/opacity subset. Cropping, rotation, and mirroring are
    /// rejected here because their centre/edge semantics depend on the nested
    /// scene's rendered canvas and cannot be silently approximated as a source
    /// transform.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] when either transform uses a
    /// crop, rotation, or flip, or when the composed fixed-point values exceed
    /// the bounded representation.
    pub fn compose_simple(self, parent: Self) -> Result<Self, MediaError> {
        if self.is_cropped()
            || parent.is_cropped()
            || self.is_rotated()
            || parent.is_rotated()
            || self.flip_x
            || self.flip_y
            || parent.flip_x
            || parent.flip_y
        {
            return Err(MediaError::InvalidTransform);
        }
        let scale_x = (u64::from(self.scale_x_milli) * u64::from(parent.scale_x_milli)) / 1_000;
        let scale_y = (u64::from(self.scale_y_milli) * u64::from(parent.scale_y_milli)) / 1_000;
        if scale_x == 0
            || scale_y == 0
            || scale_x > u64::from(Self::MAX_SCALE_MILLI)
            || scale_y > u64::from(Self::MAX_SCALE_MILLI)
        {
            return Err(MediaError::InvalidTransform);
        }
        let translate_x = (i64::from(self.translate_x) * i64::from(parent.scale_x_milli)) / 1_000
            + i64::from(parent.translate_x);
        let translate_y = (i64::from(self.translate_y) * i64::from(parent.scale_y_milli)) / 1_000
            + i64::from(parent.translate_y);
        let (Ok(scale_x), Ok(scale_y), Ok(translate_x), Ok(translate_y)) = (
            u32::try_from(scale_x),
            u32::try_from(scale_y),
            i32::try_from(translate_x),
            i32::try_from(translate_y),
        ) else {
            return Err(MediaError::InvalidTransform);
        };
        let opacity = (u16::from(self.opacity) * u16::from(parent.opacity) + 127) / 255;
        Ok(Self {
            scale_x_milli: scale_x,
            scale_y_milli: scale_y,
            translate_x,
            translate_y,
            opacity: u8::try_from(opacity).unwrap_or(u8::MAX),
            ..Self::IDENTITY
        })
    }

    /// Composes nested transforms that remain representable by one frame.
    ///
    /// This is the flattening path for nested scenes and groups. In addition to
    /// scale, translation, and opacity, it preserves horizontal and vertical
    /// mirroring by reflecting the child visible rectangle around the parent
    /// canvas before combining its fixed-point geometry. A leaf crop is exact
    /// because the cropped source rectangle remains the same source rectangle
    /// after an axis-aligned parent scale. A leaf rotation is exact only under
    /// a uniform, unmirrored parent scale; a non-uniform scale would introduce
    /// shear, and a reflected boundary changes the rotation ordering.
    ///
    /// Crop or rotation on the parent is still rejected. That operation clips
    /// or rotates an intermediate scene canvas and therefore needs a richer
    /// layer representation than one flattened [`FrameTransform`].
    ///
    /// `canvas_width` and `canvas_height` are the dimensions of the nested
    /// scene/group canvas, not the current viewport.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] when the canvas is empty, the
    /// parent uses cropping or rotation, a rotated child crosses a non-uniform
    /// or mirrored parent, a crop consumes the nested canvas, or a composed
    /// value exceeds the bounded representation.
    pub fn compose_axis_aligned(
        self,
        parent: Self,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<Self, MediaError> {
        if canvas_width == 0 || canvas_height == 0 || parent.is_cropped() || parent.is_rotated() {
            return Err(MediaError::InvalidTransform);
        }
        if self.is_rotated()
            && (parent.scale_x_milli != parent.scale_y_milli || parent.flip_x || parent.flip_y)
        {
            return Err(MediaError::InvalidTransform);
        }

        let visible_width = canvas_width
            .checked_sub(
                self.crop_left
                    .checked_add(self.crop_right)
                    .ok_or(MediaError::InvalidTransform)?,
            )
            .ok_or(MediaError::InvalidTransform)?;
        let visible_height = canvas_height
            .checked_sub(
                self.crop_top
                    .checked_add(self.crop_bottom)
                    .ok_or(MediaError::InvalidTransform)?,
            )
            .ok_or(MediaError::InvalidTransform)?;
        if visible_width == 0 || visible_height == 0 {
            return Err(MediaError::InvalidTransform);
        }

        let scale_x = (u64::from(self.scale_x_milli) * u64::from(parent.scale_x_milli)) / 1_000;
        let scale_y = (u64::from(self.scale_y_milli) * u64::from(parent.scale_y_milli)) / 1_000;
        if scale_x == 0
            || scale_y == 0
            || scale_x > u64::from(Self::MAX_SCALE_MILLI)
            || scale_y > u64::from(Self::MAX_SCALE_MILLI)
        {
            return Err(MediaError::InvalidTransform);
        }

        let translate_x = compose_axis_aligned_translation(
            self.translate_x,
            parent.translate_x,
            self.scale_x_milli,
            parent.scale_x_milli,
            canvas_width,
            visible_width,
            parent.flip_x,
        )
        .ok_or(MediaError::InvalidTransform)?;
        let translate_y = compose_axis_aligned_translation(
            self.translate_y,
            parent.translate_y,
            self.scale_y_milli,
            parent.scale_y_milli,
            canvas_height,
            visible_height,
            parent.flip_y,
        )
        .ok_or(MediaError::InvalidTransform)?;
        let (Ok(scale_x), Ok(scale_y), Ok(translate_x), Ok(translate_y)) = (
            u32::try_from(scale_x),
            u32::try_from(scale_y),
            i32::try_from(translate_x),
            i32::try_from(translate_y),
        ) else {
            return Err(MediaError::InvalidTransform);
        };
        let opacity = (u16::from(self.opacity) * u16::from(parent.opacity) + 127) / 255;
        Ok(Self {
            scale_x_milli: scale_x,
            scale_y_milli: scale_y,
            translate_x,
            translate_y,
            flip_x: self.flip_x != parent.flip_x,
            flip_y: self.flip_y != parent.flip_y,
            opacity: u8::try_from(opacity).unwrap_or(u8::MAX),
            rotation_milli_degrees: self.rotation_milli_degrees,
            crop_left: self.crop_left,
            crop_top: self.crop_top,
            crop_right: self.crop_right,
            crop_bottom: self.crop_bottom,
        })
    }
}

fn compose_axis_aligned_translation(
    child_translation: i32,
    parent_translation: i32,
    child_scale_milli: u32,
    parent_scale_milli: u32,
    canvas_dimension: u32,
    visible_dimension: u32,
    parent_flipped: bool,
) -> Option<i64> {
    let child_extent = i64::from(visible_dimension)
        .checked_mul(i64::from(child_scale_milli))?
        .checked_div(1_000)?;
    let child_origin = if parent_flipped {
        i64::from(canvas_dimension)
            .checked_sub(i64::from(child_translation))?
            .checked_sub(child_extent)?
    } else {
        i64::from(child_translation)
    };
    child_origin
        .checked_mul(i64::from(parent_scale_milli))?
        .checked_div(1_000)?
        .checked_add(i64::from(parent_translation))
}
