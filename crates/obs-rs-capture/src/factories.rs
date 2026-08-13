use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

#[cfg(all(target_os = "linux", feature = "legacy-v4l2"))]
use super::v4l2::V4l2CaptureDevice;
use super::{
    device::{CaptureRequest, VideoCaptureDevice},
    settings::{parse_camera_mode, parse_format},
    simulated::{SimulatedCaptureDevice, TestPatternDevice},
    types::CaptureKind,
};

pub const TEST_PATTERN_SOURCE_KIND: &str = "test_pattern";
/// Stable source kind for a screen capture source with a portable fallback.
pub const SCREEN_CAPTURE_SOURCE_KIND: &str = "screen_capture";
/// Stable source kind for the direct Linux X11 screen adapter with an x11grab fallback.
#[cfg(target_os = "linux")]
pub const X11_SCREEN_CAPTURE_SOURCE_KIND: &str = "x11_screen_capture";
/// Stable source kind for the Wayland screen adapter driven by the desktop
/// portal and `PipeWire`.
#[cfg(target_os = "linux")]
pub const WAYLAND_SCREEN_CAPTURE_SOURCE_KIND: &str = "wayland_screen_capture";
/// Stable source kind for a window capture source with a portable fallback.
pub const WINDOW_CAPTURE_SOURCE_KIND: &str = "window_capture";
/// Stable source kind for the direct Linux X11 window adapter.
#[cfg(target_os = "linux")]
pub const X11_WINDOW_CAPTURE_SOURCE_KIND: &str = "x11_window_capture";
/// Stable source kind for a camera capture source with a Nokhwa/portable backend.
pub const CAMERA_CAPTURE_SOURCE_KIND: &str = "camera_capture";

/// Factory that adapts [`TestPatternDevice`] to the Rust source API.
pub struct TestPatternFactory {
    kind: Identifier,
}

impl TestPatternFactory {
    /// Creates the test-pattern source factory.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InvalidIdentifier`] only if the static kind is invalid.
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(TEST_PATTERN_SOURCE_KIND)
                .map_err(PluginError::InvalidIdentifier)?,
        })
    }
}

impl SourceFactory for TestPatternFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_format(settings)?;
        let mut device = TestPatternDevice::new("test_pattern", name)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        device
            .start(format)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        Ok(Box::new(TestPatternSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            format,
            device,
        }))
    }
}

struct TestPatternSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    device: TestPatternDevice,
}

impl Source for TestPatternSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let mut device = TestPatternDevice::new("test_pattern", &self.name)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        device
            .start(format)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        self.device = device;
        self.format = format;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        self.device
            .next_frame(request.timestamp())
            .map_err(|error| SourceError::Unavailable(error.to_string()))
    }
}

/// Factory for a screen, window, or camera source with a deterministic fallback.
pub struct SimulatedCaptureFactory {
    kind: Identifier,
    capture_kind: CaptureKind,
}

impl SimulatedCaptureFactory {
    /// Creates a factory with a stable source kind and device class.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InvalidIdentifier`] when `kind` is invalid.
    pub fn new(kind: &str, capture_kind: CaptureKind) -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(kind).map_err(PluginError::InvalidIdentifier)?,
            capture_kind,
        })
    }
}

impl SourceFactory for SimulatedCaptureFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_format(settings)?;
        let device_id = settings
            .get("device_id")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.kind.as_str());
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        if self.capture_kind == CaptureKind::Camera
            && (device_id.starts_with("v4l2-") || device_id.starts_with("nokhwa-camera-"))
        {
            let native_mode = parse_camera_mode(settings)?;
            let mut source = NativeCameraSource {
                kind: self.kind.clone(),
                name: name.to_owned(),
                format,
                device_id: device_id.to_owned(),
                native_mode,
                device: None,
                failure: None,
                retry_countdown: CAMERA_RETRY_FRAMES,
            };
            // A camera that is unplugged, busy, or missing must not stop the
            // scene from being created: the source stays, reports why, and
            // recovers on its own once the camera is available again.
            source.reopen();
            return Ok(Box::new(source));
        }
        let mut device = SimulatedCaptureDevice::new(device_id, name, self.capture_kind)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        device
            .start(format)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        Ok(Box::new(SimulatedCaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            format,
            capture_kind: self.capture_kind,
            device,
        }))
    }
}

struct SimulatedCaptureSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    capture_kind: CaptureKind,
    device: SimulatedCaptureDevice,
}

impl Source for SimulatedCaptureSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let device_id = settings
            .get("device_id")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.kind.as_str());
        let mut device = SimulatedCaptureDevice::new(device_id, &self.name, self.capture_kind)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        device
            .start(format)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        self.device = device;
        self.format = format;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        self.device
            .next_frame(request.timestamp())
            .map_err(|error| SourceError::Unavailable(error.to_string()))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct NativeCameraSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    device_id: String,
    native_mode: Option<super::types::CameraMode>,
    /// `None` while the camera cannot be opened; see `failure` for the reason.
    device: Option<Box<dyn VideoCaptureDevice>>,
    failure: Option<String>,
    /// Renders since the last attempt to reopen a camera that was unavailable.
    retry_countdown: u32,
}

/// Renders between attempts to reopen an unavailable camera.
///
/// Spawning a process on every frame would cost more than the capture itself,
/// so a camera that is busy or unplugged is retried about twice a second.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const CAMERA_RETRY_FRAMES: u32 = 15;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl NativeCameraSource {
    /// Opens the camera, recording why if it cannot be opened.
    fn reopen(&mut self) {
        self.device = None;
        self.retry_countdown = CAMERA_RETRY_FRAMES;
        let mut failures = Vec::new();
        match crate::NokhwaCaptureDevice::from_device_id(&self.device_id, &self.name) {
            Ok(mut device) => {
                let request = self.native_mode.map_or_else(
                    || CaptureRequest::output(self.format),
                    |mode| CaptureRequest::camera(self.format, mode),
                );
                match device.start_capture(request) {
                    Ok(()) => {
                        self.failure = None;
                        self.device = Some(Box::new(device));
                        return;
                    }
                    Err(error) => failures.push(error.to_string()),
                }
            }
            Err(error) => failures.push(error.to_string()),
        }

        #[cfg(all(target_os = "linux", feature = "legacy-v4l2"))]
        if self.device_id.starts_with("v4l2-") {
            match V4l2CaptureDevice::from_device_id(&self.device_id, &self.name) {
                Ok(mut device) => match device.start(self.format) {
                    Ok(()) => {
                        self.failure = None;
                        self.device = Some(Box::new(device));
                        return;
                    }
                    Err(error) => failures.push(error.to_string()),
                },
                Err(error) => failures.push(error.to_string()),
            }
        }
        self.failure = Some(
            failures
                .into_iter()
                .next()
                .unwrap_or_else(|| format!("camera {} is unavailable", self.device_id)),
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl Source for NativeCameraSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let device_id = settings
            .get("device_id")
            .filter(|value| value.starts_with("v4l2-") || value.starts_with("nokhwa-camera-"))
            .ok_or_else(|| {
                SourceError::invalid_setting("device_id", "expected a native camera ID")
            })?;
        let native_mode = parse_camera_mode(settings)?;
        // Restarting the camera stream drops frames and re-negotiates the
        // device, so it only happens when the camera, mode, or format changed —
        // not because an unrelated property was edited.
        if format == self.format
            && device_id == self.device_id
            && native_mode == self.native_mode
            && self.device.is_some()
        {
            return Ok(());
        }
        self.format = format;
        device_id.clone_into(&mut self.device_id);
        self.native_mode = native_mode;
        self.reopen();
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        let Some(device) = self.device.as_mut() else {
            self.retry_countdown = self.retry_countdown.saturating_sub(1);
            if self.retry_countdown == 0 {
                self.reopen();
            }
            return Err(SourceError::Unavailable(
                self.failure
                    .clone()
                    .unwrap_or_else(|| format!("camera {} is unavailable", self.device_id)),
            ));
        };
        match device.next_frame(request.timestamp()) {
            Ok(frame) => Ok(frame),
            Err(error) => {
                // A camera that disappears mid-session becomes unavailable
                // rather than poisoning the scene: the retry path takes over.
                self.failure = Some(error.to_string());
                self.device = None;
                self.retry_countdown = CAMERA_RETRY_FRAMES;
                Err(SourceError::Unavailable(error.to_string()))
            }
        }
    }
}
