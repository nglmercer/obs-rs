use super::{CaptureDeviceInfo, CaptureError, VideoCaptureDevice};

/// Platform boundary for discovery and opening without exposing OS handles.
pub trait PlatformCaptureAdapter: Send + Sync {
    /// Stable backend name used in diagnostics and capability selection.
    fn backend_name(&self) -> &'static str;

    /// Returns one complete hot-plug/discovery snapshot.
    ///
    /// # Errors
    ///
    /// Returns a typed platform/permission/protocol failure.
    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError>;

    /// Opens a stable device ID without starting frame delivery.
    ///
    /// # Errors
    ///
    /// Returns a typed unavailable/permission/device error.
    fn open(&self, stable_id: &str) -> Result<Box<dyn VideoCaptureDevice>, CaptureError>;
}
