use std::fmt;

use super::{format::VideoFormat, pixel::PixelFormat};

/// Coarse, typed failures reported by a native Stinger resource adapter.
///
/// The resource path and backend diagnostic remain outside the real-time media
/// model. Keeping the failure kind copyable lets worker results cross the
/// bounded queue without retaining unbounded native error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StingerResourceFailure {
    /// The resource could not be opened or read.
    Unreadable,
    /// No decoder or required native element is available.
    DecoderUnavailable,
    /// The decoder pipeline reported a failure.
    Decoder,
    /// The resource completed without publishing a video frame.
    NoVideoFrames,
    /// A decoded sample did not match the negotiated bounded RGBA format.
    InvalidFrame,
    /// The worker cancelled the decode before it completed.
    Cancelled,
    /// The decoder did not complete within the bounded wait interval.
    Timeout,
}

impl fmt::Display for StingerResourceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Unreadable => "resource unreadable",
            Self::DecoderUnavailable => "decoder unavailable",
            Self::Decoder => "decoder pipeline failed",
            Self::NoVideoFrames => "resource has no video frames",
            Self::InvalidFrame => "decoded frame is invalid",
            Self::Cancelled => "decode cancelled",
            Self::Timeout => "decode timed out",
        };
        formatter.write_str(label)
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
    /// A persisted or requested scene transition duration is outside the
    /// inclusive supported range.
    InvalidTransitionDuration { duration_millis: u32 },
    /// A Luma Wipe softness value is outside the inclusive 0..=1000 range.
    InvalidLumaWipeSoftness { softness_milli: u16 },
    /// A stinger clip has no frames or exceeds the bounded frame count.
    InvalidStingerFrameCount { count: usize },
    /// A stinger frame-duration list does not match the frame list.
    InvalidStingerFrameDurations { expected: usize, actual: usize },
    /// A stinger frame duration is zero or exceeds the bounded per-frame limit.
    InvalidStingerFrameDuration { duration_nanos: u64 },
    /// A stinger transition point is outside the safe interior of the clip.
    InvalidStingerTransitionPoint { transition_point_milli: u16 },
    /// A stinger clip exceeds the bounded resident memory budget.
    StingerTooLarge { bytes: usize },
    /// A stinger clip duration exceeds the bounded playback duration.
    StingerDurationTooLong { duration_nanos: u64 },
    /// A persisted stinger resource path is empty, oversized, or contains a
    /// control character.
    InvalidStingerResourcePath { bytes: usize },
    /// A native resource adapter could not resolve a persisted Stinger.
    StingerResource { failure: StingerResourceFailure },
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
            Self::InvalidTransitionDuration { duration_millis } => write!(
                formatter,
                "scene transition duration {duration_millis} ms is outside 1..=60000"
            ),
            Self::InvalidLumaWipeSoftness { softness_milli } => write!(
                formatter,
                "luma wipe softness {softness_milli} is outside 0..=1000"
            ),
            Self::InvalidStingerFrameCount { count } => {
                write!(formatter, "stinger frame count {count} is outside 1..=256")
            }
            Self::InvalidStingerFrameDurations { expected, actual } => write!(
                formatter,
                "stinger has {actual} frame durations for {expected} frames"
            ),
            Self::InvalidStingerFrameDuration { duration_nanos } => write!(
                formatter,
                "stinger frame duration {duration_nanos} ns is outside 1ms..=60s"
            ),
            Self::InvalidStingerTransitionPoint {
                transition_point_milli,
            } => write!(
                formatter,
                "stinger transition point {transition_point_milli} is outside 1..=999"
            ),
            Self::StingerTooLarge { bytes } => write!(
                formatter,
                "stinger decoded storage {bytes} bytes exceeds the 256 MiB limit"
            ),
            Self::StingerDurationTooLong { duration_nanos } => write!(
                formatter,
                "stinger duration {duration_nanos} ns exceeds the 120s limit"
            ),
            Self::InvalidStingerResourcePath { bytes } => write!(
                formatter,
                "stinger resource path has {bytes} bytes; expected 1..=1024 bytes without control characters"
            ),
            Self::StingerResource { failure } => {
                write!(formatter, "stinger resource adapter failed: {failure}")
            }
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
