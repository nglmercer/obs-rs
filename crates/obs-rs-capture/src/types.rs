use std::collections::BTreeMap;

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
