use super::{error::MediaError, time::FrameRate};
/// A validated CPU video format for the reference renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VideoFormat {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) frame_rate: FrameRate,
}
impl VideoFormat {
    /// Maximum pixel count accepted by the CPU reference frame model.
    pub const MAX_PIXELS: usize = 16_777_216;

    /// Creates a format with non-zero dimensions and a valid frame rate.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::ZeroDimension`] for zero dimensions or
    /// [`MediaError::FrameTooLarge`] when the pixel budget is exceeded.
    pub fn new(width: u32, height: u32, frame_rate: FrameRate) -> Result<Self, MediaError> {
        if width == 0 || height == 0 {
            return Err(MediaError::ZeroDimension);
        }

        let pixels = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(MediaError::FrameTooLarge)?;
        if pixels > Self::MAX_PIXELS {
            return Err(MediaError::FrameTooLarge);
        }

        Ok(Self {
            width,
            height,
            frame_rate,
        })
    }

    /// Returns the frame width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Returns the frame height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Returns the frame rate.
    #[must_use]
    pub const fn frame_rate(self) -> FrameRate {
        self.frame_rate
    }

    /// Returns the required RGBA8 byte count.
    #[must_use]
    pub fn rgba_bytes(self) -> usize {
        let Ok(width) = usize::try_from(self.width) else {
            return 0;
        };
        let Ok(height) = usize::try_from(self.height) else {
            return 0;
        };
        width.saturating_mul(height).saturating_mul(4)
    }

    /// Returns the validated pixel count.
    #[must_use]
    pub fn pixel_count(self) -> usize {
        let Ok(width) = usize::try_from(self.width) else {
            return 0;
        };
        let Ok(height) = usize::try_from(self.height) else {
            return 0;
        };
        width.saturating_mul(height)
    }
}
