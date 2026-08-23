use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    error::CaptureError,
    types::{CameraMode, CaptureDeviceInfo},
};

/// The two resolutions involved in one capture request.
///
/// `output_format` is the normalized frame shape consumed by the existing
/// source/render contract. `native_mode`, when present, is the exact camera
/// mode requested from the backend. Keeping both values here prevents camera
/// negotiation from silently treating the scene canvas as a device
/// capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureRequest {
    output_format: VideoFormat,
    native_mode: Option<CameraMode>,
}

impl CaptureRequest {
    /// Creates a request for a normalized output without selecting a native
    /// camera mode. Screen and fallback devices use this form.
    #[must_use]
    pub const fn output(output_format: VideoFormat) -> Self {
        Self {
            output_format,
            native_mode: None,
        }
    }

    /// Creates a request with an explicit native camera mode and output size.
    #[must_use]
    pub const fn camera(output_format: VideoFormat, native_mode: CameraMode) -> Self {
        Self {
            output_format,
            native_mode: Some(native_mode),
        }
    }

    /// Returns the normalized frame shape.
    #[must_use]
    pub const fn output_format(self) -> VideoFormat {
        self.output_format
    }

    /// Returns the exact native mode, when one was selected.
    #[must_use]
    pub const fn native_mode(self) -> Option<CameraMode> {
        self.native_mode
    }
}

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

    /// Starts delivery for a normalized output and optional native camera
    /// mode.
    ///
    /// Existing screen and fallback adapters inherit the compatibility
    /// behavior and only see the output format. Camera adapters override this
    /// method to negotiate `native_mode` without changing the long-standing
    /// `start(VideoFormat)` API used by the rest of the workspace.
    ///
    /// # Errors
    ///
    /// Returns the same lifecycle or backend error as [`Self::start`].
    fn start_capture(&mut self, request: CaptureRequest) -> Result<(), CaptureError> {
        self.start(request.output_format())
    }

    /// Returns a backend-provided token that can restore the current session.
    ///
    /// Most capture devices do not have persistent session state. Wayland's
    /// screen-cast portal is the exception, so this optional hook keeps that
    /// state available without making every backend know about the portal.
    fn restore_token(&self) -> Option<&str> {
        None
    }

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
