use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{error::CaptureError, types::CaptureDeviceInfo};

/// A running or stopped source of owned video frames.
pub trait VideoCaptureDevice: Send {
    /// Returns immutable device metadata.
    fn info(&self) -> &CaptureDeviceInfo;

    /// Starts delivery at one fixed output format.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::AlreadyRunning`] or
    /// [`CaptureError::UnsupportedFormat`] when the lifecycle request is invalid.
    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError>;

    /// Stops delivery; stopping an already-stopped device is a no-op.
    fn stop(&mut self);

    /// Returns whether the device is running.
    fn is_running(&self) -> bool;

    /// Produces the next frame at `timestamp`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::NotRunning`] before `start` or a backend-specific
    /// capture error.
    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError>;
}
