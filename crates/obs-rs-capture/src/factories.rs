use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

#[cfg(target_os = "linux")]
use super::v4l2::V4l2CaptureDevice;
use super::{
    device::VideoCaptureDevice,
    settings::parse_format,
    simulated::{SimulatedCaptureDevice, TestPatternDevice},
    types::CaptureKind,
};

pub const TEST_PATTERN_SOURCE_KIND: &str = "test_pattern";
/// Stable source kind for a screen capture source with a portable fallback.
pub const SCREEN_CAPTURE_SOURCE_KIND: &str = "screen_capture";
/// Stable source kind for the direct Linux X11 screen adapter with an x11grab fallback.
#[cfg(target_os = "linux")]
pub const X11_SCREEN_CAPTURE_SOURCE_KIND: &str = "x11_screen_capture";
/// Stable source kind for a window capture source with a portable fallback.
pub const WINDOW_CAPTURE_SOURCE_KIND: &str = "window_capture";
/// Stable source kind for a camera capture source with a V4L2/portable backend.
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
        #[cfg(target_os = "linux")]
        if self.capture_kind == CaptureKind::Camera && device_id.starts_with("v4l2-") {
            let mut device = V4l2CaptureDevice::from_device_id(device_id, name)
                .map_err(|error| SourceError::Unavailable(error.to_string()))?;
            device
                .start(format)
                .map_err(|error| SourceError::Unavailable(error.to_string()))?;
            return Ok(Box::new(V4l2CaptureSource {
                kind: self.kind.clone(),
                name: name.to_owned(),
                format,
                device,
            }));
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

#[cfg(target_os = "linux")]
struct V4l2CaptureSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    device: V4l2CaptureDevice,
}

#[cfg(target_os = "linux")]
impl Source for V4l2CaptureSource {
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
            .filter(|value| value.starts_with("v4l2-"))
            .ok_or_else(|| {
                SourceError::invalid_setting("device_id", "expected a V4L2 camera ID")
            })?;
        let mut device = V4l2CaptureDevice::from_device_id(device_id, &self.name)
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
