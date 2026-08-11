use super::{error::MediaError, format::VideoFormat, frame::VideoFrame, time::Timestamp};
use rayon::prelude::*;
/// Pixel layouts accepted at a portable video boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PixelFormat {
    /// Packed red, green, blue, alpha bytes.
    Rgba8,
    /// Packed blue, green, red, alpha bytes.
    Bgra8,
    /// Packed red, green, blue bytes.
    Rgb8,
    /// One luma byte per pixel.
    Gray8,
    /// Planar 4:2:0 YUV with Y, U, and V planes.
    I420,
}

impl PixelFormat {
    /// Returns the exact byte count required for one frame in this layout.
    ///
    /// I420 requires even width and height because each chroma sample covers a
    /// 2x2 luma block.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::UnsupportedPixelDimensions`] for an odd I420
    /// dimension or [`MediaError::FrameTooLarge`] if arithmetic cannot be
    /// represented by the host.
    pub fn bytes_for(self, format: VideoFormat) -> Result<usize, MediaError> {
        let pixels = format.pixel_count();
        match self {
            Self::Rgba8 | Self::Bgra8 => pixels.checked_mul(4).ok_or(MediaError::FrameTooLarge),
            Self::Rgb8 => pixels.checked_mul(3).ok_or(MediaError::FrameTooLarge),
            Self::Gray8 => Ok(pixels),
            Self::I420 => {
                if !format.width.is_multiple_of(2) || !format.height.is_multiple_of(2) {
                    return Err(MediaError::UnsupportedPixelDimensions { pixel_format: self });
                }
                pixels
                    .checked_add(pixels / 2)
                    .ok_or(MediaError::FrameTooLarge)
            }
        }
    }
}

/// An owned frame at a packed or planar pixel boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawVideoFrame {
    format: VideoFormat,
    pixel_format: PixelFormat,
    timestamp: Timestamp,
    bytes: Vec<u8>,
}

impl RawVideoFrame {
    /// Creates a raw frame after validating its exact layout and byte length.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::UnsupportedPixelDimensions`] for an invalid planar
    /// layout or [`MediaError::BufferSize`] when `bytes` has the wrong length.
    pub fn new(
        format: VideoFormat,
        pixel_format: PixelFormat,
        timestamp: Timestamp,
        bytes: Vec<u8>,
    ) -> Result<Self, MediaError> {
        let expected = pixel_format.bytes_for(format)?;
        if bytes.len() != expected {
            return Err(MediaError::BufferSize {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Self {
            format,
            pixel_format,
            timestamp,
            bytes,
        })
    }

    /// Returns the video format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the input pixel layout.
    #[must_use]
    pub const fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// Returns the media timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns the owned-layout bytes without exposing mutable aliasing.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Converts this frame into the engine's owned RGBA8 reference format.
    ///
    /// I420 conversion uses the BT.601 limited-range integer matrix. All
    /// conversion is bounded by the validated frame dimensions and performs no
    /// native or unchecked operations.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BufferSize`] only if an invalid value bypassed the
    /// constructor invariant; valid [`RawVideoFrame`] values convert completely.
    pub fn into_rgba8(self) -> Result<VideoFrame, MediaError> {
        let Self {
            format,
            pixel_format,
            timestamp,
            bytes,
        } = self;

        let rgba = match pixel_format {
            // Already the engine's reference layout: move the owned buffer into
            // the frame instead of allocating a second one and copying.
            PixelFormat::Rgba8 => bytes,
            PixelFormat::Bgra8 => {
                let mut rgba = vec![0; format.rgba_bytes()];
                bytes
                    .par_chunks_exact(4)
                    .zip(rgba.par_chunks_exact_mut(4))
                    .for_each(|(source, target)| {
                        target[0] = source[2];
                        target[1] = source[1];
                        target[2] = source[0];
                        target[3] = source[3];
                    });
                rgba
            }
            PixelFormat::Rgb8 => {
                let mut rgba = vec![0; format.rgba_bytes()];
                bytes
                    .par_chunks_exact(3)
                    .zip(rgba.par_chunks_exact_mut(4))
                    .for_each(|(source, target)| {
                        target[..3].copy_from_slice(source);
                        target[3] = u8::MAX;
                    });
                rgba
            }
            PixelFormat::Gray8 => {
                let mut rgba = vec![0; format.rgba_bytes()];
                bytes
                    .par_iter()
                    .zip(rgba.par_chunks_exact_mut(4))
                    .for_each(|(luma, target)| {
                        target[0] = *luma;
                        target[1] = *luma;
                        target[2] = *luma;
                        target[3] = u8::MAX;
                    });
                rgba
            }
            PixelFormat::I420 => {
                let mut rgba = vec![0; format.rgba_bytes()];
                convert_i420_to_rgba(format, &bytes, &mut rgba);
                rgba
            }
        };

        VideoFrame::new(format, timestamp, rgba)
    }
}
fn convert_i420_to_rgba(format: VideoFormat, source: &[u8], target: &mut [u8]) {
    let width = format.width_index();
    let height = format.height_index();
    let luma_len = width.saturating_mul(height);
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let chroma_len = chroma_width.saturating_mul(chroma_height);
    let Some((luma, remainder)) = source.get(..luma_len).zip(source.get(luma_len..)) else {
        return;
    };
    let Some((u_plane, v_plane)) = remainder.get(..chroma_len).zip(remainder.get(chroma_len..))
    else {
        return;
    };
    let Some(target_rows) = target.get_mut(..luma_len * 4) else {
        return;
    };

    target_rows
        .par_chunks_exact_mut(width * 4)
        .enumerate()
        .for_each(|(y, target_line)| {
            // Hoisted out of the inner loop: both the luma row and the chroma
            // row are constant for the whole scanline.
            let luma_row = y * width;
            let chroma_row = (y >> 1) * chroma_width;
            let luma_line = &luma[luma_row..luma_row + width];

            for (x, (luma_value, pixel)) in luma_line
                .iter()
                .zip(target_line.chunks_exact_mut(4))
                .enumerate()
            {
                let chroma_index = chroma_row + (x >> 1);
                let u_value = u_plane[chroma_index];
                let v_value = v_plane[chroma_index];
                let u = i32::from(u_value) - 128;
                let v = i32::from(v_value) - 128;
                let c = i32::from(*luma_value) - 16;
                // `298 * c` is shared by all three channel formulas.
                let c298 = 298 * c + 128;
                pixel[0] = clamp_channel((c298 + 409 * v) >> 8);
                pixel[1] = clamp_channel((c298 - 100 * u - 208 * v) >> 8);
                pixel[2] = clamp_channel((c298 + 516 * u) >> 8);
                pixel[3] = u8::MAX;
            }
        });
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamp constrains the value to 0..=255, so the cast is exact and non-negative"
)]
fn clamp_channel(value: i32) -> u8 {
    value.clamp(0, i32::from(u8::MAX)) as u8
}
