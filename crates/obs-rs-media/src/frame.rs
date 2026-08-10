use super::{
    error::MediaError, filters::FrameFilter, format::VideoFormat, time::Timestamp,
    transform::FrameTransform, transition::FrameTransition,
};
/// An owned, tightly packed RGBA8 video frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFrame {
    format: VideoFormat,
    timestamp: Timestamp,
    pixels: Vec<u8>,
}

impl VideoFrame {
    /// Creates a frame after checking the buffer length against `format`.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BufferSize`] when `pixels` is not exactly the number
    /// of RGBA8 bytes required by `format`.
    pub fn new(
        format: VideoFormat,
        timestamp: Timestamp,
        pixels: Vec<u8>,
    ) -> Result<Self, MediaError> {
        let expected = format.rgba_bytes();
        if pixels.len() != expected {
            return Err(MediaError::BufferSize {
                expected,
                actual: pixels.len(),
            });
        }

        Ok(Self {
            format,
            timestamp,
            pixels,
        })
    }

    /// Creates a solid-color frame.
    #[must_use]
    pub fn solid(format: VideoFormat, timestamp: Timestamp, color: [u8; 4]) -> Self {
        let mut pixels = vec![0; format.rgba_bytes()];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }

        Self {
            format,
            timestamp,
            pixels,
        }
    }

    /// Returns the frame format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the frame timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns the immutable RGBA8 bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns one pixel if `(x, y)` is inside the frame.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.format.width || y >= self.format.height {
            return None;
        }

        let width = usize::try_from(self.format.width).ok()?;
        let offset = (usize::try_from(y).ok()? * width + usize::try_from(x).ok()?) * 4;
        let pixel = self.pixels.get(offset..offset + 4)?;
        Some([pixel[0], pixel[1], pixel[2], pixel[3]])
    }

    /// Overlays `foreground` using straight-alpha RGBA blending.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] when the two frames do not share the
    /// same video format.
    pub fn blend_over(&mut self, foreground: &Self) -> Result<(), MediaError> {
        if self.format != foreground.format {
            return Err(MediaError::FormatMismatch {
                expected: self.format,
                actual: foreground.format,
            });
        }

        for (background, source) in self
            .pixels
            .chunks_exact_mut(4)
            .zip(foreground.pixels.chunks_exact(4))
        {
            let source_alpha = u32::from(source[3]);
            let inverse_alpha = 255 - source_alpha;
            let background_alpha = u32::from(background[3]);
            let output_alpha = source_alpha + background_alpha * inverse_alpha / 255;
            background[0] = blend_channel(
                background[0],
                source[0],
                source_alpha,
                background_alpha,
                inverse_alpha,
                output_alpha,
            );
            background[1] = blend_channel(
                background[1],
                source[1],
                source_alpha,
                background_alpha,
                inverse_alpha,
                output_alpha,
            );
            background[2] = blend_channel(
                background[2],
                source[2],
                source_alpha,
                background_alpha,
                inverse_alpha,
                output_alpha,
            );
            background[3] = to_byte(output_alpha);
        }

        Ok(())
    }

    /// Produces a deterministic transition between two same-format frames.
    ///
    /// The destination timestamp is used for the result. Cross-fades interpolate
    /// every RGBA byte with integer arithmetic, which makes offline previews and
    /// live output use the same correctness oracle.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] for different formats or
    /// [`MediaError::InvalidTransition`] for an invalid cross-fade progress value.
    pub fn transitioned(
        source: &Self,
        destination: &Self,
        transition: FrameTransition,
    ) -> Result<Self, MediaError> {
        if source.format != destination.format {
            return Err(MediaError::FormatMismatch {
                expected: source.format,
                actual: destination.format,
            });
        }

        match transition {
            FrameTransition::Cut => Ok(destination.clone()),
            FrameTransition::CrossFade { progress_milli } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                let destination_weight = u32::from(progress_milli);
                let source_weight = 1_000 - destination_weight;
                let pixels = source
                    .pixels
                    .iter()
                    .zip(&destination.pixels)
                    .map(|(source, destination)| {
                        let value = u32::from(*source) * source_weight
                            + u32::from(*destination) * destination_weight;
                        u8::try_from((value + 500) / 1_000).unwrap_or(u8::MAX)
                    })
                    .collect();
                Self::new(destination.format, destination.timestamp, pixels)
            }
        }
    }

    /// Applies a nearest-neighbor transform into a new transparent frame.
    ///
    /// Pixels outside the transformed source remain transparent. Alpha is
    /// multiplied by the transform opacity; RGB values are otherwise preserved.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] if the transform was not constructed
    /// through a valid constructor.
    pub fn transformed(&self, transform: FrameTransform) -> Result<Self, MediaError> {
        if transform.scale_x_milli == 0
            || transform.scale_y_milli == 0
            || transform.scale_x_milli > FrameTransform::MAX_SCALE_MILLI
            || transform.scale_y_milli > FrameTransform::MAX_SCALE_MILLI
        {
            return Err(MediaError::InvalidTransform);
        }

        let mut output = Self::solid(self.format, self.timestamp, [0, 0, 0, 0]);
        let width = i64::from(self.format.width);
        let height = i64::from(self.format.height);
        let scale_x = i64::from(transform.scale_x_milli);
        let scale_y = i64::from(transform.scale_y_milli);
        let output_width =
            usize::try_from(self.format.width).map_err(|_| MediaError::FrameTooLarge)?;

        for y in 0..self.format.height {
            for x in 0..self.format.width {
                let local_x = i64::from(x) - i64::from(transform.translate_x);
                let local_y = i64::from(y) - i64::from(transform.translate_y);
                if local_x < 0 || local_y < 0 {
                    continue;
                }
                let mut source_x = local_x * 1_000 / scale_x;
                let mut source_y = local_y * 1_000 / scale_y;
                if source_x >= width || source_y >= height {
                    continue;
                }
                if transform.flip_x {
                    source_x = width - 1 - source_x;
                }
                if transform.flip_y {
                    source_y = height - 1 - source_y;
                }

                let source_offset = (usize::try_from(source_y)
                    .map_err(|_| MediaError::FrameTooLarge)?
                    * output_width
                    + usize::try_from(source_x).map_err(|_| MediaError::FrameTooLarge)?)
                    * 4;
                let output_offset = (usize::try_from(y).map_err(|_| MediaError::FrameTooLarge)?
                    * output_width
                    + usize::try_from(x).map_err(|_| MediaError::FrameTooLarge)?)
                    * 4;
                output.pixels[output_offset..output_offset + 4]
                    .copy_from_slice(&self.pixels[source_offset..source_offset + 4]);
                let alpha = u32::from(output.pixels[output_offset + 3])
                    * u32::from(transform.opacity)
                    / u32::from(u8::MAX);
                output.pixels[output_offset + 3] = to_byte(alpha);
            }
        }

        Ok(output)
    }

    /// Applies one CPU filter and returns a new owned frame.
    #[must_use]
    pub fn filtered(&self, filter: FrameFilter) -> Self {
        let mut output = self.clone();
        output.apply_filter(filter);
        output
    }

    /// Applies one CPU filter in place without allocating another frame.
    pub fn apply_filter(&mut self, filter: FrameFilter) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            match filter {
                FrameFilter::Grayscale => {
                    let luma = (u32::from(pixel[0]) * 77
                        + u32::from(pixel[1]) * 150
                        + u32::from(pixel[2]) * 29)
                        / 256;
                    let luma = to_byte(luma);
                    pixel[0] = luma;
                    pixel[1] = luma;
                    pixel[2] = luma;
                }
                FrameFilter::Brightness { milli } => {
                    let multiplier = i32::from(milli) + 1_000;
                    for channel in &mut pixel[..3] {
                        let value = i32::from(*channel) * multiplier / 1_000;
                        *channel = to_byte(u32::try_from(value.max(0)).unwrap_or(u32::MAX));
                    }
                }
                FrameFilter::Opacity(opacity) => {
                    let alpha = u32::from(pixel[3]) * u32::from(opacity) / 255;
                    pixel[3] = to_byte(alpha);
                }
            }
        }
    }

    /// Clears RGB values on fully transparent pixels for canonical composition.
    pub fn clear_transparent_rgb(&mut self) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            if pixel[3] == 0 {
                pixel[..3].fill(0);
            }
        }
    }

    /// Calculates a stable FNV-1a checksum of the frame bytes.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        self.pixels
            .iter()
            .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
            })
    }
}
fn blend_channel(
    background: u8,
    source: u8,
    source_alpha: u32,
    background_alpha: u32,
    inverse_alpha: u32,
    output_alpha: u32,
) -> u8 {
    if output_alpha == 0 {
        return 0;
    }
    let numerator = u32::from(source) * source_alpha * 255
        + u32::from(background) * background_alpha * inverse_alpha;
    to_byte(numerator / (output_alpha * 255))
}

fn to_byte(value: u32) -> u8 {
    u8::try_from(value.min(u32::from(u8::MAX))).unwrap_or(u8::MAX)
}
