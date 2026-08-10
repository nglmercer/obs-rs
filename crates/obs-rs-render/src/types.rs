/// Stable handle for one backend-owned texture resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextureId(pub(crate) u64);

impl TextureId {
    /// Returns the numeric ID for diagnostics.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Backend lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderState {
    /// Resources can be created and used.
    Ready,
    /// The rendering context is lost and must be recovered.
    Lost,
}

/// Capabilities exposed without leaking backend-specific handles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderCapabilities {
    accelerated: bool,
    readback: bool,
    max_textures: usize,
    max_texture_bytes: usize,
}

impl RenderCapabilities {
    /// Creates a capability record.
    #[must_use]
    pub const fn new(accelerated: bool, readback: bool, max_textures: usize) -> Self {
        Self {
            accelerated,
            readback,
            max_textures,
            max_texture_bytes: usize::MAX,
        }
    }

    /// Creates capabilities with an explicit aggregate texture byte budget.
    #[must_use]
    pub const fn with_texture_bytes(
        accelerated: bool,
        readback: bool,
        max_textures: usize,
        max_texture_bytes: usize,
    ) -> Self {
        Self {
            accelerated,
            readback,
            max_textures,
            max_texture_bytes,
        }
    }

    /// Returns whether this backend uses an accelerated device.
    #[must_use]
    pub const fn accelerated(self) -> bool {
        self.accelerated
    }

    /// Returns whether CPU readback is supported.
    #[must_use]
    pub const fn readback(self) -> bool {
        self.readback
    }

    /// Returns the maximum number of simultaneously allocated textures.
    #[must_use]
    pub const fn max_textures(self) -> usize {
        self.max_textures
    }

    /// Returns the aggregate byte budget for allocated texture storage.
    #[must_use]
    pub const fn max_texture_bytes(self) -> usize {
        self.max_texture_bytes
    }
}

/// Counters for render-resource lifetime and data movement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderMetrics {
    pub(crate) textures_created: u64,
    pub(crate) textures_destroyed: u64,
    pub(crate) uploads: u64,
    pub(crate) compositions: u64,
    pub(crate) readbacks: u64,
    pub(crate) context_losses: u64,
    pub(crate) recoveries: u64,
    pub(crate) allocated_bytes: usize,
    pub(crate) peak_allocated_bytes: usize,
}

impl RenderMetrics {
    /// Returns the number of successfully created textures.
    #[must_use]
    pub const fn textures_created(self) -> u64 {
        self.textures_created
    }

    /// Returns the number of successfully destroyed textures.
    #[must_use]
    pub const fn textures_destroyed(self) -> u64 {
        self.textures_destroyed
    }

    /// Returns the number of successful frame uploads.
    #[must_use]
    pub const fn uploads(self) -> u64 {
        self.uploads
    }

    /// Returns the number of successful ordered compositions.
    #[must_use]
    pub const fn compositions(self) -> u64 {
        self.compositions
    }

    /// Returns the number of successful readbacks.
    #[must_use]
    pub const fn readbacks(self) -> u64 {
        self.readbacks
    }

    /// Returns the number of simulated context-loss events.
    #[must_use]
    pub const fn context_losses(self) -> u64 {
        self.context_losses
    }

    /// Returns the number of successful context recoveries.
    #[must_use]
    pub const fn recoveries(self) -> u64 {
        self.recoveries
    }

    /// Returns current aggregate texture storage in bytes.
    #[must_use]
    pub const fn allocated_bytes(self) -> usize {
        self.allocated_bytes
    }

    /// Returns the high-water mark of aggregate texture storage in bytes.
    #[must_use]
    pub const fn peak_allocated_bytes(self) -> usize {
        self.peak_allocated_bytes
    }
}
