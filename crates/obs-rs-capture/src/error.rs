use std::fmt;

use obs_rs_media::{MediaError, VideoFormat};
use obs_rs_util::Identifier;

/// Capture lifecycle and frame-delivery errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureError {
    /// The descriptor could not be constructed.
    InvalidDevice { reason: String },
    /// The catalog already contains this device ID.
    DuplicateDevice(Identifier),
    /// A catalog event targeted an unknown device ID.
    UnknownDevice(Identifier),
    /// The device has already been started.
    AlreadyRunning,
    /// A frame was requested before start.
    NotRunning,
    /// The backend cannot produce the requested format.
    UnsupportedFormat(VideoFormat),
    /// The operating system denied capture permission.
    PermissionDenied,
    /// Permission must be requested before capture can start.
    PermissionRequired,
    /// Permission handling is unavailable for this device.
    PermissionUnavailable,
    /// The backend's frame counter cannot advance.
    FrameCounterExhausted,
    /// A frame-stream reader failed.
    Io { message: String },
    /// A platform capture service is unavailable on this host.
    PlatformUnavailable { message: String },
    /// A platform capture protocol returned an invalid response.
    Protocol { message: String },
    /// A platform capture reply exceeds the bounded decoder budget.
    ReplyTooLarge { bytes: u64 },
    /// A frame-stream packet did not begin with [`crate::FRAME_STREAM_MAGIC`].
    InvalidFrameHeader,
    /// A frame-stream packet ended before its declared fields or pixels.
    TruncatedFrame,
    /// A frame-stream packet uses a different format than the started device.
    FrameFormatMismatch {
        /// Format requested when the device was started.
        expected: VideoFormat,
        /// Format declared by the packet.
        actual: VideoFormat,
    },
    /// A frame-stream packet declares a pixel length different from its format.
    FrameBufferSize { expected: usize, actual: usize },
    /// A frame-stream packet exceeds the bounded reader budget.
    FramePacketTooLarge { bytes: u64 },
    /// A media invariant failed while producing a frame.
    Media(MediaError),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDevice { reason } => write!(formatter, "invalid capture device: {reason}"),
            Self::DuplicateDevice(id) => write!(formatter, "capture device {id} is duplicated"),
            Self::UnknownDevice(id) => write!(formatter, "capture device {id} is unknown"),
            Self::AlreadyRunning => formatter.write_str("capture device is already running"),
            Self::NotRunning => formatter.write_str("capture device is not running"),
            Self::UnsupportedFormat(format) => {
                write!(formatter, "capture format is unsupported: {format:?}")
            }
            Self::PermissionDenied => formatter.write_str("capture permission was denied"),
            Self::PermissionRequired => formatter.write_str("capture permission is required"),
            Self::PermissionUnavailable => {
                formatter.write_str("capture permission handling is unavailable")
            }
            Self::FrameCounterExhausted => formatter.write_str("capture frame counter exhausted"),
            Self::Io { message } => write!(formatter, "capture frame stream I/O failed: {message}"),
            Self::PlatformUnavailable { message } => {
                write!(formatter, "platform capture is unavailable: {message}")
            }
            Self::Protocol { message } => {
                write!(formatter, "platform capture protocol failed: {message}")
            }
            Self::ReplyTooLarge { bytes } => {
                write!(
                    formatter,
                    "platform capture reply is too large: {bytes} bytes"
                )
            }
            Self::InvalidFrameHeader => {
                formatter.write_str("capture frame stream header is invalid")
            }
            Self::TruncatedFrame => formatter.write_str("capture frame stream packet is truncated"),
            Self::FrameFormatMismatch { expected, actual } => write!(
                formatter,
                "capture frame stream format {actual:?} does not match {expected:?}"
            ),
            Self::FrameBufferSize { expected, actual } => write!(
                formatter,
                "capture frame stream declares {actual} payload bytes; expected {expected}"
            ),
            Self::FramePacketTooLarge { bytes } => {
                write!(
                    formatter,
                    "capture frame stream packet is too large: {bytes} bytes"
                )
            }
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CaptureError {}
