use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

use super::{
    device::{CaptureRequest, VideoCaptureDevice},
    settings::{parse_camera_mode, parse_format},
    simulated::{SimulatedCaptureDevice, TestPatternDevice},
    types::CaptureKind,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::{lifecycle::CaptureLifecycleState, threaded::ThreadedCaptureDevice};

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
/// Stable source kind for the Nokhwa-backed camera capture source.
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

/// Factory for a screen or window source with a deterministic fallback.
///
/// The shipped camera source uses a dedicated Nokhwa-backed factory. This
/// generic factory still supports deterministic camera fixtures for low-level
/// tests.
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
        if self.capture_kind == CaptureKind::Camera && is_nokhwa_camera_id(device_id) {
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
                shutdown_blocked: false,
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

/// Factory for the real camera source.
///
/// Camera sources are deliberately separate from [`SimulatedCaptureFactory`]
/// so a native camera ID can never silently fall back to a generated test
/// pattern. Every camera opened through the built-in source kind therefore
/// reaches [`crate::NokhwaCaptureDevice`].
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub struct NokhwaCaptureFactory {
    kind: Identifier,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl NokhwaCaptureFactory {
    /// Creates the Nokhwa-backed camera factory.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InvalidIdentifier`] only if the static source
    /// kind is invalid.
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(CAMERA_CAPTURE_SOURCE_KIND)
                .map_err(PluginError::InvalidIdentifier)?,
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl SourceFactory for NokhwaCaptureFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_format(settings)?;
        let device_id = match settings.get("device_id").map(str::trim) {
            Some(value) if !value.is_empty() => {
                if !is_nokhwa_camera_id(value) {
                    return Err(SourceError::invalid_setting(
                        "device_id",
                        "expected a Nokhwa camera ID",
                    ));
                }
                value
            }
            _ => "nokhwa-camera-0",
        };
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
            shutdown_blocked: false,
        };
        // Opening is asynchronous. A disconnected or busy camera remains a
        // valid project source and reports its Nokhwa failure on render.
        source.reopen();
        Ok(Box::new(source))
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

/// A camera rendered from a worker thread rather than from the compositor.
///
/// Opening a camera and pulling a frame from it are both driver calls that can
/// stall for tens of milliseconds. Doing either inline would hold up every
/// other source in the scene — a screen capture would stutter because a webcam
/// was slow — so the device lives on its own thread and the compositor only
/// ever reads its newest frame.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct NativeCameraSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    device_id: String,
    native_mode: Option<super::types::CameraMode>,
    /// `None` while the camera cannot be opened; see `failure` for the reason.
    device: Option<ThreadedCaptureDevice>,
    failure: Option<String>,
    /// Renders since the last attempt to reopen a camera that was unavailable.
    retry_countdown: u32,
    /// Set when the previous worker could not be joined yet. No replacement
    /// may open the same physical camera until that worker has finished.
    shutdown_blocked: bool,
}

/// Renders between attempts to reopen an unavailable camera.
///
/// Spawning a process on every frame would cost more than the capture itself,
/// so a camera that is busy or unplugged is retried about twice a second.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const CAMERA_RETRY_FRAMES: u32 = 15;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl NativeCameraSource {
    /// Starts a worker that opens the camera and keeps its newest frame ready.
    ///
    /// This returns immediately. A camera that is unplugged, busy, or missing
    /// therefore costs the scene nothing at all: the failure surfaces on a
    /// later render, and the retry path reopens it.
    fn reopen(&mut self) {
        self.retry_countdown = CAMERA_RETRY_FRAMES;
        self.failure = None;
        // The old worker is shut down before the new one is created. Assigning
        // over `self.device` would not do it: the replacement is constructed —
        // and its worker has already begun opening the camera — before the old
        // value is dropped, so both would be holding the same device and the
        // new one would very likely be told it is busy.
        if let Some(mut previous) = self.device.take() {
            if !previous.shutdown() {
                self.failure = Some(
                    "the previous camera worker is still shutting down; waiting before reopening"
                        .to_owned(),
                );
                self.shutdown_blocked = true;
                self.device = Some(previous);
                return;
            }
        }
        self.shutdown_blocked = false;
        let request = self.native_mode.map_or_else(
            || CaptureRequest::output(self.format),
            |mode| CaptureRequest::camera(self.format, mode),
        );
        let device_id = self.device_id.clone();
        let name = self.name.clone();
        self.device = Some(ThreadedCaptureDevice::open(
            request,
            &self.device_id,
            move || {
                crate::NokhwaCaptureDevice::from_device_id(&device_id, &name)
                    .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>)
            },
        ));
    }

    /// Returns the error to report while the camera is not delivering frames.
    fn unavailable(&self) -> SourceError {
        SourceError::Unavailable(
            self.failure
                .clone()
                .unwrap_or_else(|| format!("camera {} is unavailable", self.device_id)),
        )
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
            .filter(|value| is_nokhwa_camera_id(value))
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
        if self.shutdown_blocked {
            if self
                .device
                .as_ref()
                .is_some_and(|device| device.state() == CaptureLifecycleState::Lost)
            {
                self.reopen();
            } else {
                return Err(self.unavailable());
            }
        }
        match self.device.as_ref().map(ThreadedCaptureDevice::state) {
            // No worker at all: count down to the next attempt.
            None => {
                self.retry_countdown = self.retry_countdown.saturating_sub(1);
                if self.retry_countdown == 0 {
                    self.reopen();
                }
                Err(self.unavailable())
            }
            // Permission was refused. Reopening would only re-prompt, and at
            // one attempt a second that is a dialog the user cannot escape, so
            // this state is terminal: `update` is the way out of it, because
            // granting access is something the person has to do first.
            Some(CaptureLifecycleState::Denied) => {
                self.failure = self
                    .device
                    .as_ref()
                    .and_then(ThreadedCaptureDevice::failure)
                    .map(|error| error.to_string());
                Err(self.unavailable())
            }
            // A stopped worker keeps serving its last frame, which would freeze
            // the picture silently, so recovery is driven from the state rather
            // than from the poll result.
            Some(CaptureLifecycleState::Lost) => {
                self.failure = self
                    .device
                    .as_ref()
                    .and_then(ThreadedCaptureDevice::failure)
                    .map(|error| error.to_string());
                self.retry_countdown = self.retry_countdown.saturating_sub(1);
                if self.retry_countdown == 0 {
                    self.reopen();
                }
                Err(self.unavailable())
            }
            Some(_) => {
                let timestamp = request.timestamp();
                let Some(device) = self.device.as_mut() else {
                    return Err(self.unavailable());
                };
                match device.poll_frame(timestamp) {
                    Ok(frame) => {
                        if frame.is_some() {
                            self.failure = None;
                        }
                        Ok(frame)
                    }
                    Err(error) => {
                        self.failure = Some(error.to_string());
                        Err(SourceError::Unavailable(error.to_string()))
                    }
                }
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn is_nokhwa_camera_id(id: &str) -> bool {
    // `v4l2-videoN` is retained as a project-file migration spelling, but it
    // is still opened by Nokhwa and never by a separate V4L2 backend.
    id.starts_with("v4l2-") || id.starts_with("nokhwa-camera-")
}
