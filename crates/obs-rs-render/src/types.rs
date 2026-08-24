use obs_rs_media::VideoFormat;

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

/// The consumer that owns a render target's latency and size budget.
///
/// Keeping the role beside the format prevents a GUI viewport from being
/// treated as an alias for the program canvas. A backend may share resources
/// between roles, but the decision remains explicit at the submission boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RenderTargetRole {
    /// The scene selected for the live program/output path.
    Program,
    /// The bounded desktop view of the live program feed.
    ///
    /// This is deliberately separate from [`Self::Program`]: the program
    /// canvas may be 4K even when the studio window only needs a 1000px view.
    ProgramPreview,
    /// A bounded desktop preview viewport.
    Preview,
    /// A projector or multiview consumer.
    Projector,
    /// One bounded tile in the multiview compositor.
    MultiviewTile,
    /// A target whose pixels are being converted for an encoder.
    Encoder,
}

/// A render target request with an explicit consumer role and media format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RenderTarget {
    role: RenderTargetRole,
    format: VideoFormat,
}

impl RenderTarget {
    /// Creates a target request.
    #[must_use]
    pub const fn new(role: RenderTargetRole, format: VideoFormat) -> Self {
        Self { role, format }
    }

    /// Returns the consumer role.
    #[must_use]
    pub const fn role(self) -> RenderTargetRole {
        self.role
    }

    /// Returns the target media format.
    #[must_use]
    pub const fn format(self) -> VideoFormat {
        self.format
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
    pub(crate) uploaded_bytes: u64,
    pub(crate) compositions: u64,
    pub(crate) readbacks: u64,
    pub(crate) readback_bytes: u64,
    pub(crate) color_conversions: u64,
    pub(crate) context_losses: u64,
    pub(crate) recoveries: u64,
    pub(crate) allocated_bytes: usize,
    pub(crate) peak_allocated_bytes: usize,
}

impl RenderMetrics {
    /// Records allocation of one backend texture.
    pub fn record_texture_created(&mut self, bytes: usize) {
        self.textures_created = self.textures_created.saturating_add(1);
        self.allocated_bytes = self.allocated_bytes.saturating_add(bytes);
        self.peak_allocated_bytes = self.peak_allocated_bytes.max(self.allocated_bytes);
    }

    /// Records destruction of one backend texture.
    pub fn record_texture_destroyed(&mut self, bytes: usize) {
        self.textures_destroyed = self.textures_destroyed.saturating_add(1);
        self.allocated_bytes = self.allocated_bytes.saturating_sub(bytes);
    }

    /// Records one host-to-backend upload.
    pub fn record_upload(&mut self) {
        self.record_upload_bytes(0);
    }

    /// Records one host-to-backend upload and its payload size.
    pub fn record_upload_bytes(&mut self, bytes: usize) {
        self.uploads = self.uploads.saturating_add(1);
        self.uploaded_bytes = self
            .uploaded_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    /// Records one completed backend composition.
    pub fn record_composition(&mut self) {
        self.compositions = self.compositions.saturating_add(1);
    }

    /// Records one explicit backend-to-host readback.
    pub fn record_readback(&mut self) {
        self.record_readback_bytes(0);
    }

    /// Records one explicit backend-to-host readback and its payload size.
    pub fn record_readback_bytes(&mut self, bytes: usize) {
        self.readbacks = self.readbacks.saturating_add(1);
        self.readback_bytes = self
            .readback_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    /// Records one device-side pixel-format conversion.
    pub fn record_color_conversion(&mut self) {
        self.color_conversions = self.color_conversions.saturating_add(1);
    }

    /// Records a context/device loss notification.
    pub fn record_context_loss(&mut self) {
        self.context_losses = self.context_losses.saturating_add(1);
    }

    /// Records successful context/device recovery.
    pub fn record_recovery(&mut self) {
        self.recoveries = self.recoveries.saturating_add(1);
    }

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

    /// Returns the cumulative host-to-backend payload size.
    #[must_use]
    pub const fn uploaded_bytes(self) -> u64 {
        self.uploaded_bytes
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

    /// Returns the cumulative backend-to-host payload size.
    #[must_use]
    pub const fn readback_bytes(self) -> u64 {
        self.readback_bytes
    }

    /// Returns the number of completed device-side color conversions.
    #[must_use]
    pub const fn color_conversions(self) -> u64 {
        self.color_conversions
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
