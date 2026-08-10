use super::{error::MediaError, format::VideoFormat, frame::VideoFrame, time::Timestamp};
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
        let mut rgba = vec![0; self.format.rgba_bytes()];
        match self.pixel_format {
            PixelFormat::Rgba8 => rgba.copy_from_slice(&self.bytes),
            PixelFormat::Bgra8 => {
                for (source, target) in self.bytes.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                    target[0] = source[2];
                    target[1] = source[1];
                    target[2] = source[0];
                    target[3] = source[3];
                }
            }
            PixelFormat::Rgb8 => {
                for (source, target) in self.bytes.chunks_exact(3).zip(rgba.chunks_exact_mut(4)) {
                    target[..3].copy_from_slice(source);
                    target[3] = u8::MAX;
                }
            }
            PixelFormat::Gray8 => {
                for (luma, target) in self.bytes.iter().zip(rgba.chunks_exact_mut(4)) {
                    target[0] = *luma;
                    target[1] = *luma;
                    target[2] = *luma;
                    target[3] = u8::MAX;
                }
            }
            PixelFormat::I420 => convert_i420_to_rgba(self.format, &self.bytes, &mut rgba),
        }
        VideoFrame::new(self.format, self.timestamp, rgba)
    }
}
fn convert_i420_to_rgba(format: VideoFormat, source: &[u8], target: &mut [u8]) {
    let width = usize::try_from(format.width).unwrap_or(0);
    let height = usize::try_from(format.height).unwrap_or(0);
    let luma_len = width.saturating_mul(height);
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let chroma_len = chroma_width.saturating_mul(chroma_height);
    let (luma, remainder) = source.split_at(luma_len);
    let (u_plane, v_plane) = remainder.split_at(chroma_len);

    for y in 0..height {
        for x in 0..width {
            let luma_value = i32::from(luma[y * width + x]);
            let chroma_index = (y / 2) * chroma_width + (x / 2);
            let u = i32::from(u_plane[chroma_index]) - 128;
            let v = i32::from(v_plane[chroma_index]) - 128;
            let c = luma_value - 16;
            let red = (298 * c + 409 * v + 128) >> 8;
            let green = (298 * c - 100 * u - 208 * v + 128) >> 8;
            let blue = (298 * c + 516 * u + 128) >> 8;
            let offset = (y * width + x) * 4;
            target[offset] = clamp_channel(red);
            target[offset + 1] = clamp_channel(green);
            target[offset + 2] = clamp_channel(blue);
            target[offset + 3] = u8::MAX;
        }
    }
}

fn clamp_channel(value: i32) -> u8 {
    u8::try_from(value.clamp(0, i32::from(u8::MAX))).unwrap_or(u8::MAX)
}
