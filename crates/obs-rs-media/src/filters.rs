/// A deterministic CPU filter applied after a scene-item transform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameFilter {
    /// Converts RGB to a luma approximation while preserving alpha.
    Grayscale,
    /// Multiplies RGB by `milli / 1000`, clamping to the byte range.
    Brightness { milli: i16 },
    /// Multiplies alpha by the supplied byte factor.
    Opacity(u8),
}
