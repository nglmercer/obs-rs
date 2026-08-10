//! Portable media values for the OBS-RS reference engine.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::fmt;

/// A monotonic media position expressed in nanoseconds.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(u64);

impl Timestamp {
    /// The beginning of a media timeline.
    pub const ZERO: Self = Self(0);

    /// Creates a timestamp from nanoseconds.
    #[must_use]
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        Self(nanoseconds)
    }

    /// Creates a timestamp from milliseconds.
    #[must_use]
    pub const fn from_millis(milliseconds: u64) -> Self {
        Self(milliseconds.saturating_mul(1_000_000))
    }

    /// Returns the timestamp in nanoseconds.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Adds nanoseconds, returning `None` if the timeline would overflow.
    #[must_use]
    pub const fn checked_add(self, nanoseconds: u64) -> Option<Self> {
        match self.0.checked_add(nanoseconds) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// A reduced, positive rational video frame rate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FrameRate {
    numerator: u32,
    denominator: u32,
}

impl FrameRate {
    /// Creates and reduces a frame rate such as `30/1` or `30000/1001`.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidFrameRate`] when either component is zero.
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, MediaError> {
        if numerator == 0 || denominator == 0 {
            return Err(MediaError::InvalidFrameRate);
        }

        let divisor = greatest_common_divisor(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    /// Returns the reduced numerator.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// Returns the reduced denominator.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    /// Returns the number of nanoseconds per frame when it fits in `u64`.
    #[must_use]
    pub fn period_nanos(self) -> Option<u64> {
        let period = 1_000_000_000_u128 * u128::from(self.denominator) / u128::from(self.numerator);
        u64::try_from(period).ok()
    }
}

/// A validated CPU video format for the reference renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VideoFormat {
    width: u32,
    height: u32,
    frame_rate: FrameRate,
}

/// A deterministic nearest-neighbor scene transform.
///
/// Scale values use thousandths: `1000` is 100%, `2000` is 200%. Translation is
/// expressed in output pixels and the transform is anchored at the top-left corner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTransform {
    scale_x_milli: u32,
    scale_y_milli: u32,
    translate_x: i32,
    translate_y: i32,
    flip_x: bool,
    flip_y: bool,
    opacity: u8,
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
}

/// A video transition applied between two frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameTransition {
    /// Selects the destination frame immediately.
    Cut,
    /// Linearly interpolates source and destination bytes from 0 to 1000.
    CrossFade { progress_milli: u16 },
}

impl FrameTransition {
    /// Creates a cross-fade at a validated progress value.
    ///
    /// `0` selects the source frame and `1000` selects the destination frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is greater
    /// than `1000`.
    pub const fn cross_fade(progress_milli: u16) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        Ok(Self::CrossFade { progress_milli })
    }
}

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

/// Errors raised by the portable media value model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaError {
    /// A frame rate has a zero numerator or denominator.
    InvalidFrameRate,
    /// A video format has a zero width or height.
    ZeroDimension,
    /// A video format exceeds the reference renderer's pixel budget.
    FrameTooLarge,
    /// A frame's buffer length does not match its format.
    BufferSize { expected: usize, actual: usize },
    /// A transform has an unsupported scale.
    InvalidTransform,
    /// A transition progress value is outside the inclusive 0..=1000 range.
    InvalidTransition { progress_milli: u16 },
    /// A pixel layout requires dimensions that the format does not provide.
    UnsupportedPixelDimensions { pixel_format: PixelFormat },
    /// Two frames cannot be combined because their formats differ.
    FormatMismatch {
        /// The format expected by the operation.
        expected: VideoFormat,
        /// The format supplied by the caller.
        actual: VideoFormat,
    },
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameRate => formatter.write_str("frame rate must be non-zero"),
            Self::ZeroDimension => formatter.write_str("video dimensions must be non-zero"),
            Self::FrameTooLarge => formatter.write_str("video format exceeds pixel budget"),
            Self::BufferSize { expected, actual } => {
                write!(
                    formatter,
                    "frame buffer has {actual} bytes; expected {expected}"
                )
            }
            Self::InvalidTransform => formatter.write_str("video transform scale is invalid"),
            Self::InvalidTransition { progress_milli } => write!(
                formatter,
                "video transition progress {progress_milli} is outside 0..=1000"
            ),
            Self::UnsupportedPixelDimensions { pixel_format } => write!(
                formatter,
                "pixel format {pixel_format:?} does not support these dimensions"
            ),
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "frame format {actual:?} does not match {expected:?}"
                )
            }
        }
    }
}

impl std::error::Error for MediaError {}

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

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> VideoFormat {
        VideoFormat::new(2, 2, FrameRate::new(60, 2).expect("valid rate")).expect("valid format")
    }

    #[test]
    fn reduces_rates_and_reports_period() {
        let rate = FrameRate::new(60, 2).expect("valid rate");

        assert_eq!(rate.numerator(), 30);
        assert_eq!(rate.denominator(), 1);
        assert_eq!(rate.period_nanos(), Some(33_333_333));
    }

    #[test]
    fn rejects_invalid_formats_and_buffers() {
        let rate = FrameRate::new(30, 1).expect("valid rate");

        assert_eq!(FrameRate::new(0, 1), Err(MediaError::InvalidFrameRate));
        assert_eq!(VideoFormat::new(0, 2, rate), Err(MediaError::ZeroDimension));
        assert_eq!(
            VideoFrame::new(format(), Timestamp::ZERO, vec![0; 3]),
            Err(MediaError::BufferSize {
                expected: 16,
                actual: 3
            })
        );
    }

    #[test]
    fn blends_and_reads_pixels_deterministically() {
        let background = VideoFrame::solid(format(), Timestamp::ZERO, [0, 0, 255, 255]);
        let foreground = VideoFrame::solid(format(), Timestamp::ZERO, [255, 0, 0, 128]);
        let mut result = background;
        result.blend_over(&foreground).expect("matching formats");

        assert_eq!(result.pixel(0, 0), Some([128, 0, 127, 255]));
        assert_eq!(result.pixel(2, 0), None);
        assert_ne!(result.checksum(), 0);
    }

    #[test]
    fn transforms_flip_and_apply_opacity() {
        let format = VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        let frame = VideoFrame::new(
            format,
            Timestamp::ZERO,
            vec![255, 0, 0, 255, 0, 0, 255, 255],
        )
        .expect("valid pixels");
        let transform =
            FrameTransform::new(1_000, 1_000, 0, 0, true, false, 128).expect("valid transform");
        let transformed = frame.transformed(transform).expect("transform succeeds");

        assert_eq!(transformed.pixel(0, 0), Some([0, 0, 255, 128]));
        assert_eq!(transformed.pixel(1, 0), Some([255, 0, 0, 128]));
    }

    #[test]
    fn filters_modify_owned_pixels_without_mutating_the_input() {
        let format = VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        let frame = VideoFrame::solid(format, Timestamp::ZERO, [100, 150, 200, 255]);
        let filtered = frame
            .filtered(FrameFilter::Grayscale)
            .filtered(FrameFilter::Brightness { milli: 500 })
            .filtered(FrameFilter::Opacity(128));

        assert_eq!(frame.pixel(0, 0), Some([100, 150, 200, 255]));
        assert_eq!(filtered.pixel(0, 0), Some([210, 210, 210, 128]));
    }

    #[test]
    fn transparent_rgb_can_be_canonicalized_for_composition() {
        let frame = VideoFrame::solid(format(), Timestamp::ZERO, [100, 150, 200, 0]);
        let mut canonical = frame.clone();
        canonical.clear_transparent_rgb();

        assert_eq!(frame.pixel(0, 0), Some([100, 150, 200, 0]));
        assert_eq!(canonical.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn converts_packed_layouts_to_rgba() {
        let format = VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
        let bgra = RawVideoFrame::new(
            format,
            PixelFormat::Bgra8,
            Timestamp::from_millis(4),
            vec![3, 2, 1, 4, 7, 6, 5, 8],
        )
        .expect("bgra");
        assert_eq!(
            bgra.into_rgba8().expect("converted").pixels(),
            &[1, 2, 3, 4, 5, 6, 7, 8]
        );

        let gray = RawVideoFrame::new(format, PixelFormat::Gray8, Timestamp::ZERO, vec![9, 10])
            .expect("gray");
        assert_eq!(
            gray.into_rgba8().expect("converted").pixels(),
            &[9, 9, 9, 255, 10, 10, 10, 255]
        );
    }

    #[test]
    fn converts_i420_and_rejects_odd_planar_dimensions() {
        let format = format();
        let i420 = RawVideoFrame::new(
            format,
            PixelFormat::I420,
            Timestamp::ZERO,
            vec![235, 235, 235, 235, 128, 128],
        )
        .expect("i420");
        assert_eq!(
            i420.into_rgba8().expect("converted").pixel(1, 1),
            Some([255, 255, 255, 255])
        );

        let odd = VideoFormat::new(3, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
        assert_eq!(
            PixelFormat::I420.bytes_for(odd),
            Err(MediaError::UnsupportedPixelDimensions {
                pixel_format: PixelFormat::I420
            })
        );
    }

    #[test]
    fn transitions_are_deterministic_and_validate_progress() {
        let source = VideoFrame::solid(format(), Timestamp::ZERO, [0, 0, 0, 0]);
        let destination =
            VideoFrame::solid(format(), Timestamp::from_millis(10), [100, 200, 255, 255]);
        let transition = FrameTransition::cross_fade(500).expect("valid progress");
        let halfway =
            VideoFrame::transitioned(&source, &destination, transition).expect("transition");
        assert_eq!(halfway.timestamp(), Timestamp::from_millis(10));
        assert_eq!(halfway.pixel(0, 0), Some([50, 100, 128, 128]));
        assert_eq!(
            FrameTransition::cross_fade(1_001),
            Err(MediaError::InvalidTransition {
                progress_milli: 1_001
            })
        );
    }
}
