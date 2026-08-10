use std::fmt;

use super::{format::VideoFormat, pixel::PixelFormat};
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
