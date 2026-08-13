use std::collections::BTreeMap;

use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_util::Identifier;

use super::error::CaptureError;

/// The kind of video device represented by a capture descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CaptureKind {
    /// A deterministic in-process source used for tests and fallback behavior.
    TestPattern,
    /// A desktop/screen adapter or its portable fallback.
    Screen,
    /// A window adapter or its portable fallback.
    Window,
    /// A camera adapter or its portable fallback.
    Camera,
    /// A source delivered by a separately sandboxed Rust process.
    External,
}

/// A pixel layout that a camera can expose before the capture layer normalizes
/// it to the engine's RGBA8 frame contract.
///
/// This is intentionally separate from [`obs_rs_media::PixelFormat`]. Camera
/// drivers advertise transport formats such as MJPEG and YUYV, while the
/// media crate describes layouts that are already safe to hand to the
/// renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CameraPixelFormat {
    /// Motion-JPEG compressed camera frames.
    Mjpeg,
    /// Packed YUV 4:2:2 camera frames.
    Yuyv,
    /// 8-bit 4:2:0 bi-planar camera frames.
    Nv12,
    /// One luma byte per camera pixel.
    Gray,
    /// Packed RGB camera frames.
    Rgb,
    /// Packed BGR camera frames.
    Bgr,
}

impl CameraPixelFormat {
    /// Returns the stable value used by source settings and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mjpeg => "mjpeg",
            Self::Yuyv => "yuyv",
            Self::Nv12 => "nv12",
            Self::Gray => "gray",
            Self::Rgb => "rgb",
            Self::Bgr => "bgr",
        }
    }

    /// Parses a stable setting or backend name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mjpeg" | "mjpg" => Some(Self::Mjpeg),
            "yuyv" | "yuy2" => Some(Self::Yuyv),
            "nv12" => Some(Self::Nv12),
            "gray" | "grey" | "gray8" => Some(Self::Gray),
            "rgb" | "rawrgb" | "rgb8" => Some(Self::Rgb),
            "bgr" | "rawbgr" | "bgr8" => Some(Self::Bgr),
            _ => None,
        }
    }
}

impl std::fmt::Display for CameraPixelFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One native camera mode advertised by a capture backend.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CameraMode {
    pixel_format: CameraPixelFormat,
    width: u32,
    height: u32,
    frame_rate: FrameRate,
}

impl CameraMode {
    /// Creates a validated native camera mode.
    ///
    /// The same pixel budget as [`VideoFormat`] is applied here. A backend may
    /// advertise a larger mode, but the portable frame contract must not let a
    /// single capture request bypass the engine's memory bound.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Media`] when the dimensions or frame rate are
    /// outside the portable media bounds.
    pub fn new(
        pixel_format: CameraPixelFormat,
        width: u32,
        height: u32,
        frame_rate: FrameRate,
    ) -> Result<Self, CaptureError> {
        VideoFormat::new(width, height, frame_rate).map_err(CaptureError::Media)?;
        Ok(Self {
            pixel_format,
            width,
            height,
            frame_rate,
        })
    }

    /// Returns the native pixel layout.
    #[must_use]
    pub const fn pixel_format(self) -> CameraPixelFormat {
        self.pixel_format
    }

    /// Returns the native width.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Returns the native height.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Returns the native frame rate.
    #[must_use]
    pub const fn frame_rate(self) -> FrameRate {
        self.frame_rate
    }

    /// Converts the dimensions and rate to the normalized media format.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Media`] if the mode was constructed by an
    /// older serialized representation that violates the current pixel bound.
    pub fn video_format(self) -> Result<VideoFormat, CaptureError> {
        VideoFormat::new(self.width, self.height, self.frame_rate).map_err(CaptureError::Media)
    }
}

/// A stable target that a source may ask a capture provider to open.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CaptureTarget {
    id: Identifier,
    kind: CaptureKind,
}

impl CaptureTarget {
    /// Creates a target from a stable provider ID.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] for an invalid ID.
    pub fn new(id: &str, kind: CaptureKind) -> Result<Self, CaptureError> {
        Ok(Self {
            id: Identifier::new(id).map_err(|error| CaptureError::InvalidDevice {
                reason: error.to_string(),
            })?,
            kind,
        })
    }

    /// Returns the stable target ID.
    #[must_use]
    pub const fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the target kind.
    #[must_use]
    pub const fn kind(&self) -> CaptureKind {
        self.kind
    }
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

/// Connection state for a discovered capture target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureDeviceState {
    /// The target is currently present in the provider's discovery snapshot.
    Connected,
    /// The target was known but is currently unavailable, usually after a
    /// hot-unplug. Providers may retain this descriptor to make reconnects
    /// observable without changing the project's stable ID.
    Disconnected,
}

/// Safe capability snapshot reported by a capture adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CaptureBackendCapabilities {
    formats: Vec<VideoFormat>,
    camera_modes: Vec<CameraMode>,
    native_surfaces: bool,
    reconnectable: bool,
    hotplug: bool,
}

impl CaptureBackendCapabilities {
    /// Creates a deterministic, de-duplicated format snapshot.
    #[must_use]
    pub fn new(mut formats: Vec<VideoFormat>, native_surfaces: bool, reconnectable: bool) -> Self {
        formats.sort_unstable();
        formats.dedup();
        Self {
            formats,
            camera_modes: Vec::new(),
            native_surfaces,
            reconnectable,
            hotplug: false,
        }
    }

    /// Returns negotiated CPU frame formats in deterministic order.
    #[must_use]
    pub fn formats(&self) -> &[VideoFormat] {
        &self.formats
    }

    /// Adds a deterministic, de-duplicated list of native camera modes.
    #[must_use]
    pub fn with_camera_modes(mut self, mut modes: Vec<CameraMode>) -> Self {
        modes.sort_unstable();
        modes.dedup();
        self.camera_modes = modes;
        self
    }

    /// Returns the native camera modes, or an empty list when the backend does
    /// not expose camera capabilities for this target.
    #[must_use]
    pub fn camera_modes(&self) -> &[CameraMode] {
        &self.camera_modes
    }

    /// Returns whether a native camera mode is explicitly supported.
    #[must_use]
    pub fn supports_camera_mode(&self, mode: CameraMode) -> bool {
        self.camera_modes.is_empty() || self.camera_modes.binary_search(&mode).is_ok()
    }

    /// Sets whether this backend can reopen a target after hot-plug changes.
    #[must_use]
    pub const fn with_hotplug(mut self, hotplug: bool) -> Self {
        self.hotplug = hotplug;
        self
    }

    /// Returns whether the provider reports hot-plug state for this target.
    #[must_use]
    pub const fn hotplug(&self) -> bool {
        self.hotplug
    }

    /// Sets reconnect support while preserving the capability snapshot.
    #[must_use]
    pub const fn with_reconnectable(mut self, reconnectable: bool) -> Self {
        self.reconnectable = reconnectable;
        self
    }

    /// Returns whether the adapter can submit opaque native surfaces.
    #[must_use]
    pub const fn native_surfaces(&self) -> bool {
        self.native_surfaces
    }

    /// Returns whether a lost stream can be reopened with the stable ID.
    #[must_use]
    pub const fn reconnectable(&self) -> bool {
        self.reconnectable
    }

    /// Returns whether a requested CPU format is explicitly supported.
    #[must_use]
    pub fn supports(&self, format: VideoFormat) -> bool {
        self.formats.is_empty() || self.formats.binary_search(&format).is_ok()
    }
}

/// Backwards-compatible name for the capability snapshot used by existing
/// platform adapters.
pub type CaptureCapabilities = CaptureBackendCapabilities;

/// Immutable metadata used for discovery and diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureDeviceInfo {
    id: Identifier,
    name: String,
    kind: CaptureKind,
    permission: CapturePermission,
    state: CaptureDeviceState,
    capabilities: CaptureBackendCapabilities,
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
            state: CaptureDeviceState::Connected,
            capabilities: CaptureBackendCapabilities::default(),
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

    /// Returns the current connection state.
    #[must_use]
    pub const fn state(&self) -> CaptureDeviceState {
        self.state
    }

    /// Updates the permission state after a platform prompt or policy change.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.permission = permission;
    }

    /// Updates the connection state after a hot-plug event.
    pub const fn set_state(&mut self, state: CaptureDeviceState) {
        self.state = state;
    }

    /// Replaces the device capability snapshot.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: CaptureCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Returns safe format/surface/reconnect capabilities.
    #[must_use]
    pub const fn capabilities(&self) -> &CaptureBackendCapabilities {
        &self.capabilities
    }

    /// Returns the stable target represented by this descriptor.
    #[must_use]
    pub fn target(&self) -> CaptureTarget {
        CaptureTarget {
            id: self.id.clone(),
            kind: self.kind,
        }
    }
}

/// A camera discovery result with its native capability list attached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraDevice {
    info: CaptureDeviceInfo,
}

impl CameraDevice {
    /// Creates a camera descriptor from a stable ID, name, and native modes.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when `id` or `name` is invalid.
    pub fn new(id: &str, name: &str, modes: Vec<CameraMode>) -> Result<Self, CaptureError> {
        let capabilities = CaptureBackendCapabilities::default()
            .with_camera_modes(modes)
            .with_hotplug(true)
            .with_reconnectable(true);
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, CaptureKind::Camera)?
                .with_capabilities(capabilities),
        })
    }

    /// Returns the descriptor used by the general capture catalog.
    #[must_use]
    pub const fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    /// Returns the stable camera ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        self.info.id()
    }

    /// Returns the camera's display name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.info.name()
    }

    /// Returns the modes reported by the backend.
    #[must_use]
    pub fn modes(&self) -> &[CameraMode] {
        self.info.capabilities().camera_modes()
    }

    /// Moves the descriptor into the general catalog representation.
    #[must_use]
    pub fn into_info(self) -> CaptureDeviceInfo {
        self.info
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
    /// A known target changed connection state without changing its stable ID.
    StateChanged {
        /// Device whose connection state changed.
        id: Identifier,
        /// New connection state.
        state: CaptureDeviceState,
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
    ///
    /// Borrows the stored descriptors, so listing the catalog does not clone
    /// every device. Callers that need owned values can `.cloned().collect()`.
    #[must_use]
    pub fn devices(&self) -> impl ExactSizeIterator<Item = &CaptureDeviceInfo> {
        self.devices.values()
    }

    /// Returns the number of registered descriptors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Returns whether the catalog holds no descriptors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Looks up one descriptor.
    ///
    /// The lookup borrows `id` as a `str` key, so it neither allocates nor
    /// revalidates an [`Identifier`].
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CaptureDeviceInfo> {
        self.devices.get(id)
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
            CaptureEvent::StateChanged { id, state } => {
                let info = self
                    .devices
                    .get_mut(&id)
                    .ok_or_else(|| CaptureError::UnknownDevice(id.clone()))?;
                info.set_state(state);
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
