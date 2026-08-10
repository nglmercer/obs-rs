//! Rust-native capture contracts and a deterministic CPU test backend.
//!
//! Platform capture implementations can depend on this contract later. The
//! portable engine only sees owned frames, capabilities, and typed lifecycle errors.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{collections::BTreeMap, fmt};

use obs_rs_config::Config;
use obs_rs_media::{FrameRate, MediaError, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

/// The kind of video device represented by a capture descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CaptureKind {
    /// A deterministic in-process source used for tests and fallback behavior.
    TestPattern,
    /// A future desktop/screen adapter.
    Screen,
    /// A future window adapter.
    Window,
    /// A future camera adapter.
    Camera,
}

/// Permission state reported by a capture provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapturePermission {
    /// The provider may start delivering frames.
    Granted,
    /// The user must approve access before the device can start.
    PromptRequired,
    /// The user or operating system denied access.
    Denied,
    /// The platform cannot provide permission handling for this device.
    Unavailable,
}

/// Immutable metadata used for discovery and diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureDeviceInfo {
    id: Identifier,
    name: String,
    kind: CaptureKind,
    permission: CapturePermission,
}

impl CaptureDeviceInfo {
    /// Creates a descriptor after validating the stable device ID.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] for an invalid ID or empty name.
    pub fn new(id: &str, name: &str, kind: CaptureKind) -> Result<Self, CaptureError> {
        if name.trim().is_empty() {
            return Err(CaptureError::InvalidDevice {
                reason: "device name is empty".to_owned(),
            });
        }
        Ok(Self {
            id: Identifier::new(id).map_err(|error| CaptureError::InvalidDevice {
                reason: error.to_string(),
            })?,
            name: name.to_owned(),
            kind,
            permission: CapturePermission::Granted,
        })
    }

    /// Returns the stable device ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the device kind.
    #[must_use]
    pub const fn kind(&self) -> CaptureKind {
        self.kind
    }

    /// Returns the current permission state.
    #[must_use]
    pub const fn permission(&self) -> CapturePermission {
        self.permission
    }

    /// Updates the permission state after a platform prompt or policy change.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.permission = permission;
    }
}

/// A discovery or permission event applied to a capture catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureEvent {
    /// A device became available.
    Added(CaptureDeviceInfo),
    /// A device disappeared.
    Removed(Identifier),
    /// A device's permission state changed.
    PermissionChanged {
        /// Device whose permission changed.
        id: Identifier,
        /// New permission state.
        permission: CapturePermission,
    },
}

/// A catalog of discovered capture descriptors.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CaptureCatalog {
    devices: BTreeMap<Identifier, CaptureDeviceInfo>,
}

impl CaptureCatalog {
    /// Creates an empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a descriptor, rejecting duplicate IDs.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::DuplicateDevice`] when the ID already exists.
    pub fn register(&mut self, info: CaptureDeviceInfo) -> Result<(), CaptureError> {
        if self.devices.contains_key(info.id()) {
            return Err(CaptureError::DuplicateDevice(info.id().clone()));
        }
        self.devices.insert(info.id().clone(), info);
        Ok(())
    }

    /// Returns descriptors in stable ID order.
    #[must_use]
    pub fn devices(&self) -> Vec<CaptureDeviceInfo> {
        self.devices.values().cloned().collect()
    }

    /// Looks up one descriptor.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CaptureDeviceInfo> {
        let id = Identifier::new(id).ok()?;
        self.devices.get(&id)
    }

    /// Applies one hot-plug or permission event atomically.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::DuplicateDevice`] for a repeated add or
    /// [`CaptureError::UnknownDevice`] for an event targeting a missing device.
    pub fn apply(&mut self, event: CaptureEvent) -> Result<(), CaptureError> {
        match event {
            CaptureEvent::Added(info) => self.register(info),
            CaptureEvent::Removed(id) => self
                .devices
                .remove(&id)
                .map(|_| ())
                .ok_or(CaptureError::UnknownDevice(id)),
            CaptureEvent::PermissionChanged { id, permission } => {
                let info = self
                    .devices
                    .get_mut(&id)
                    .ok_or_else(|| CaptureError::UnknownDevice(id.clone()))?;
                info.set_permission(permission);
                Ok(())
            }
        }
    }

    /// Replaces the complete catalog atomically from one discovery snapshot.
    ///
    /// Duplicate IDs are rejected before the current catalog is changed.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::DuplicateDevice`] when the snapshot repeats an ID.
    pub fn replace_all<I>(&mut self, devices: I) -> Result<(), CaptureError>
    where
        I: IntoIterator<Item = CaptureDeviceInfo>,
    {
        let mut replacement = BTreeMap::new();
        for info in devices {
            let id = info.id().clone();
            if replacement.insert(id.clone(), info).is_some() {
                return Err(CaptureError::DuplicateDevice(id));
            }
        }
        self.devices = replacement;
        Ok(())
    }
}

/// Safe discovery boundary implemented by platform and fallback providers.
pub trait CaptureProvider {
    /// Returns one complete discovery snapshot.
    ///
    /// # Errors
    ///
    /// Returns a provider-specific [`CaptureError`] when discovery cannot produce
    /// a valid descriptor set.
    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError>;

    /// Replaces a catalog with the provider's latest snapshot.
    ///
    /// # Errors
    ///
    /// Propagates discovery or duplicate-ID validation errors.
    fn refresh(&self, catalog: &mut CaptureCatalog) -> Result<(), CaptureError> {
        catalog.replace_all(self.discover()?)
    }
}

/// A deterministic discovery provider for the portable CPU fallback devices.
///
/// This provider is intentionally not an operating-system adapter. It gives the
/// runtime and UI a complete discovery contract while real screen/window/camera
/// providers are added behind [`CaptureProvider`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SimulatedCaptureProvider;

impl SimulatedCaptureProvider {
    /// Creates the deterministic fallback provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CaptureProvider for SimulatedCaptureProvider {
    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        Ok(vec![
            CaptureDeviceInfo::new("test-pattern", "Test pattern", CaptureKind::TestPattern)?,
            CaptureDeviceInfo::new("screen-0", "Simulated screen", CaptureKind::Screen)?,
            CaptureDeviceInfo::new("window-0", "Simulated window", CaptureKind::Window)?,
            CaptureDeviceInfo::new("camera-0", "Simulated camera", CaptureKind::Camera)?,
        ])
    }
}

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
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CaptureError {}

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

/// A deterministic animated checkerboard capture device.
pub struct TestPatternDevice {
    info: CaptureDeviceInfo,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl TestPatternDevice {
    /// Creates a test device with stable metadata.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when the descriptor is invalid.
    pub fn new(id: &str, name: &str) -> Result<Self, CaptureError> {
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, CaptureKind::TestPattern)?,
            format: None,
            frame_index: 0,
        })
    }

    /// Returns the current frame counter.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the simulated permission state for lifecycle tests.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }
}

impl VideoCaptureDevice for TestPatternDevice {
    fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        if self.format.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        match self.info.permission() {
            CapturePermission::Granted => {}
            CapturePermission::PromptRequired => return Err(CaptureError::PermissionRequired),
            CapturePermission::Denied => return Err(CaptureError::PermissionDenied),
            CapturePermission::Unavailable => return Err(CaptureError::PermissionUnavailable),
        }
        self.format = Some(format);
        self.frame_index = 0;
        Ok(())
    }

    fn stop(&mut self) {
        self.format = None;
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let frame = simulated_frame(format, timestamp, self.frame_index, self.info.kind())?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}

/// A deterministic CPU fallback for screen, window, or camera capture.
pub struct SimulatedCaptureDevice {
    info: CaptureDeviceInfo,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl SimulatedCaptureDevice {
    /// Creates a simulated device for any supported capture kind.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when the descriptor is invalid.
    pub fn new(id: &str, name: &str, kind: CaptureKind) -> Result<Self, CaptureError> {
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, kind)?,
            format: None,
            frame_index: 0,
        })
    }

    /// Returns the current frame counter.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the simulated permission state for lifecycle tests.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }
}

impl VideoCaptureDevice for SimulatedCaptureDevice {
    fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        if self.format.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        match self.info.permission() {
            CapturePermission::Granted => {}
            CapturePermission::PromptRequired => return Err(CaptureError::PermissionRequired),
            CapturePermission::Denied => return Err(CaptureError::PermissionDenied),
            CapturePermission::Unavailable => return Err(CaptureError::PermissionUnavailable),
        }
        self.format = Some(format);
        self.frame_index = 0;
        Ok(())
    }

    fn stop(&mut self) {
        self.format = None;
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let frame = simulated_frame(format, timestamp, self.frame_index, self.info.kind())?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}

fn simulated_frame(
    format: VideoFormat,
    timestamp: Timestamp,
    frame_index: u64,
    kind: CaptureKind,
) -> Result<VideoFrame, CaptureError> {
    let width = usize::try_from(format.width())
        .map_err(|_| CaptureError::Media(MediaError::FrameTooLarge))?;
    let height = usize::try_from(format.height())
        .map_err(|_| CaptureError::Media(MediaError::FrameTooLarge))?;
    let mut pixels = vec![0_u8; format.rgba_bytes()];
    let phase = frame_index % 2;
    let variant = match kind {
        CaptureKind::TestPattern => 0,
        CaptureKind::Screen => 16,
        CaptureKind::Window => 32,
        CaptureKind::Camera => 48,
    };
    for y in 0..height {
        for x in 0..width {
            let tile = ((x / 16 + y / 16) as u64 + phase) % 2;
            let offset = (y * width + x) * 4;
            pixels[offset] = if tile == 0 {
                32_u8.saturating_add(variant)
            } else {
                224_u8.saturating_sub(variant)
            };
            pixels[offset + 1] = gradient_byte(x, width).saturating_add(variant / 2);
            pixels[offset + 2] = gradient_byte(y, height).saturating_add(variant / 3);
            pixels[offset + 3] = 255;
        }
    }
    VideoFrame::new(format, timestamp, pixels).map_err(CaptureError::Media)
}

/// Stable source kind for the built-in capture adapter.
pub const TEST_PATTERN_SOURCE_KIND: &str = "test_pattern";
/// Stable source kind for the simulated screen fallback.
pub const SCREEN_CAPTURE_SOURCE_KIND: &str = "screen_capture";
/// Stable source kind for the simulated window fallback.
pub const WINDOW_CAPTURE_SOURCE_KIND: &str = "window_capture";
/// Stable source kind for the simulated camera fallback.
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

/// Factory for a simulated screen, window, or camera source.
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
        let mut device = SimulatedCaptureDevice::new(self.kind.as_str(), name, self.capture_kind)
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
        let mut device =
            SimulatedCaptureDevice::new(self.kind.as_str(), &self.name, self.capture_kind)
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

fn parse_format(settings: &Config) -> Result<VideoFormat, SourceError> {
    let width = parse_u32(settings, "width")?;
    let height = parse_u32(settings, "height")?;
    let numerator = parse_u32_with_default(settings, "fps_numerator", 30)?;
    let denominator = parse_u32_with_default(settings, "fps_denominator", 1)?;
    let frame_rate = FrameRate::new(numerator, denominator)
        .map_err(|error| SourceError::invalid_setting("fps", error.to_string()))?;
    VideoFormat::new(width, height, frame_rate)
        .map_err(|error| SourceError::invalid_setting("format", error.to_string()))
}

fn parse_u32(settings: &Config, key: &str) -> Result<u32, SourceError> {
    let value = settings
        .get(key)
        .ok_or_else(|| SourceError::invalid_setting(key, "setting is required"))?;
    value
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting(key, error.to_string()))
}

fn parse_u32_with_default(settings: &Config, key: &str, default: u32) -> Result<u32, SourceError> {
    let Some(value) = settings.get(key) else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting(key, error.to_string()))
}

fn gradient_byte(value: usize, size: usize) -> u8 {
    let value = u64::try_from(value).unwrap_or(u64::MAX);
    let size = u64::try_from(size.max(1)).unwrap_or(u64::MAX);
    let scaled = value.saturating_mul(255) / size;
    u8::try_from(scaled.min(u64::from(u8::MAX))).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> VideoFormat {
        VideoFormat::new(32, 16, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
    }

    #[test]
    fn catalog_is_deterministic_and_rejects_duplicates() {
        let first =
            CaptureDeviceInfo::new("screen_b", "B", CaptureKind::Screen).expect("valid info");
        let second =
            CaptureDeviceInfo::new("screen_a", "A", CaptureKind::Screen).expect("valid info");
        let mut catalog = CaptureCatalog::new();
        catalog.register(first.clone()).expect("first device");
        catalog.register(second).expect("second device");
        assert_eq!(
            catalog.devices()[0],
            catalog.get("screen_a").cloned().expect("lookup")
        );
        assert_eq!(
            catalog.register(first),
            Err(CaptureError::DuplicateDevice(
                Identifier::new("screen_b").expect("valid id")
            ))
        );
    }

    #[test]
    fn catalog_applies_hotplug_and_permission_events() {
        let info =
            CaptureDeviceInfo::new("camera", "Camera", CaptureKind::Camera).expect("device info");
        let id = info.id().clone();
        let mut catalog = CaptureCatalog::new();
        catalog.apply(CaptureEvent::Added(info)).expect("add event");
        catalog
            .apply(CaptureEvent::PermissionChanged {
                id: id.clone(),
                permission: CapturePermission::PromptRequired,
            })
            .expect("permission event");
        assert_eq!(
            catalog.get("camera").expect("camera").permission(),
            CapturePermission::PromptRequired
        );
        catalog
            .apply(CaptureEvent::Removed(id.clone()))
            .expect("remove event");
        assert!(catalog.get("camera").is_none());
        assert_eq!(
            catalog.apply(CaptureEvent::Removed(id.clone())),
            Err(CaptureError::UnknownDevice(id))
        );
    }

    #[test]
    fn provider_refreshes_catalog_atomically_and_deterministically() {
        let provider = SimulatedCaptureProvider::new();
        let mut catalog = CaptureCatalog::new();
        catalog
            .register(
                CaptureDeviceInfo::new("old", "Old device", CaptureKind::Screen)
                    .expect("old device"),
            )
            .expect("register old device");
        provider.refresh(&mut catalog).expect("refresh catalog");

        let devices = catalog.devices();
        assert_eq!(devices.len(), 4);
        assert_eq!(devices[0].id().as_str(), "camera-0");
        assert_eq!(devices[1].id().as_str(), "screen-0");
        assert_eq!(devices[2].id().as_str(), "test-pattern");
        assert_eq!(devices[3].id().as_str(), "window-0");
        assert!(catalog.get("old").is_none());
    }

    #[test]
    fn catalog_snapshot_rejects_duplicates_without_partial_replacement() {
        let mut catalog = CaptureCatalog::new();
        catalog
            .register(
                CaptureDeviceInfo::new("stable", "Stable", CaptureKind::Screen)
                    .expect("stable device"),
            )
            .expect("register stable");
        let duplicate = CaptureDeviceInfo::new("duplicate", "One", CaptureKind::Screen)
            .expect("first duplicate");
        let duplicate_again = CaptureDeviceInfo::new("duplicate", "Two", CaptureKind::Window)
            .expect("second duplicate");

        assert_eq!(
            catalog.replace_all(vec![duplicate, duplicate_again]),
            Err(CaptureError::DuplicateDevice(
                Identifier::new("duplicate").expect("valid ID")
            ))
        );
        assert!(catalog.get("stable").is_some());
        assert!(catalog.get("duplicate").is_none());
    }

    #[test]
    fn test_device_has_start_stop_and_animated_frames() {
        let mut device = TestPatternDevice::new("pattern", "Pattern").expect("device");
        assert_eq!(
            device.next_frame(Timestamp::ZERO),
            Err(CaptureError::NotRunning)
        );
        device.set_permission(CapturePermission::Denied);
        assert_eq!(device.start(format()), Err(CaptureError::PermissionDenied));
        device.set_permission(CapturePermission::Granted);
        device.start(format()).expect("start device");
        assert!(device.is_running());
        assert_eq!(device.start(format()), Err(CaptureError::AlreadyRunning));
        let first = device
            .next_frame(Timestamp::ZERO)
            .expect("first frame")
            .expect("frame exists");
        let second = device
            .next_frame(Timestamp::from_millis(33))
            .expect("second frame")
            .expect("frame exists");
        assert_ne!(first.pixels(), second.pixels());
        assert_eq!(device.frame_index(), 2);
        device.stop();
        assert!(!device.is_running());
    }

    #[test]
    fn simulated_platform_kinds_share_the_cpu_lifecycle_contract() {
        for kind in [
            CaptureKind::Screen,
            CaptureKind::Window,
            CaptureKind::Camera,
        ] {
            let id = match kind {
                CaptureKind::Screen => "screen",
                CaptureKind::Window => "window",
                CaptureKind::Camera => "camera",
                CaptureKind::TestPattern => "pattern",
            };
            let mut device = SimulatedCaptureDevice::new(id, id, kind).expect("device");
            device.start(format()).expect("start");
            let frame = device
                .next_frame(Timestamp::from_millis(33))
                .expect("frame result")
                .expect("frame");
            assert_eq!(frame.format(), format());
            assert_eq!(frame.timestamp(), Timestamp::from_millis(33));
            assert_eq!(device.info().kind(), kind);
            device.stop();
            assert!(!device.is_running());
        }
    }

    #[test]
    fn source_factory_exposes_test_pattern_frames() {
        let factory = TestPatternFactory::new().expect("factory");
        let mut settings = Config::new();
        settings.set("width", "32").expect("width");
        settings.set("height", "16").expect("height");
        let mut source = factory.create("capture", &settings).expect("source");
        let frame = source
            .render(&VideoRequest::new(Timestamp::ZERO, format()))
            .expect("render")
            .expect("frame");
        assert_eq!(frame.format(), format());
    }

    #[test]
    fn simulated_factories_expose_screen_window_and_camera_sources() {
        let settings = {
            let mut settings = Config::new();
            settings.set("width", "32").expect("width");
            settings.set("height", "16").expect("height");
            settings
        };
        for (kind, capture_kind) in [
            (SCREEN_CAPTURE_SOURCE_KIND, CaptureKind::Screen),
            (WINDOW_CAPTURE_SOURCE_KIND, CaptureKind::Window),
            (CAMERA_CAPTURE_SOURCE_KIND, CaptureKind::Camera),
        ] {
            let factory = SimulatedCaptureFactory::new(kind, capture_kind).expect("factory");
            let mut source = factory.create("capture", &settings).expect("source");
            let frame = source
                .render(&VideoRequest::new(Timestamp::ZERO, format()))
                .expect("render")
                .expect("frame");
            assert_eq!(frame.format(), format());
        }
    }
}
