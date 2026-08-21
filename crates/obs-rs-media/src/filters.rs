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
}
