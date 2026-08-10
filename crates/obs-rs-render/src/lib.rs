//! Portable render-backend contracts with a deterministic CPU fallback.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{collections::BTreeMap, fmt};

use obs_rs_media::{MediaError, RawVideoFrame, Timestamp, VideoFormat, VideoFrame};

/// Default byte budget used by the CPU backend constructor.
pub const DEFAULT_MAX_TEXTURE_BYTES: usize = 512 * 1024 * 1024;

/// Stable handle for one backend-owned texture resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextureId(u64);

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
    textures_created: u64,
    textures_destroyed: u64,
    uploads: u64,
    compositions: u64,
    readbacks: u64,
    context_losses: u64,
    recoveries: u64,
    allocated_bytes: usize,
    peak_allocated_bytes: usize,
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

/// Errors raised by render resources and composition.
#[derive(Debug, Eq, PartialEq)]
pub enum RenderError {
    /// A backend cannot be created with zero texture capacity.
    ZeroCapacity,
    /// A texture allocation would exceed the backend resource limit.
    TextureLimit { limit: usize },
    /// A texture allocation would exceed the aggregate byte budget.
    TextureByteLimit { limit: usize, requested: usize },
    /// Texture ID allocation overflowed.
    IdExhausted,
    /// A texture handle is not owned by the backend.
    UnknownTexture(TextureId),
    /// An operation requires a recovered context.
    ContextLost,
    /// A frame and texture use different formats.
    FormatMismatch {
        /// Format owned by the texture.
        expected: VideoFormat,
        /// Format supplied by the caller.
        actual: VideoFormat,
    },
    /// A texture has not received an uploaded frame.
    TextureNotReady(TextureId),
    /// A composition request contains no layers.
    EmptyComposition,
    /// A media invariant failed during CPU composition.
    Media(MediaError),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("render texture capacity must be non-zero"),
            Self::TextureLimit { limit } => {
                write!(formatter, "render texture limit reached: {limit}")
            }
            Self::TextureByteLimit { limit, requested } => write!(
                formatter,
                "render texture allocation of {requested} bytes exceeds {limit}-byte budget"
            ),
            Self::IdExhausted => formatter.write_str("render texture ID space is exhausted"),
            Self::UnknownTexture(id) => {
                write!(formatter, "render texture {} does not exist", id.value())
            }
            Self::ContextLost => formatter.write_str("render context is lost"),
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "render format {actual:?} does not match {expected:?}"
                )
            }
            Self::TextureNotReady(id) => {
                write!(formatter, "render texture {} has no frame", id.value())
            }
            Self::EmptyComposition => {
                formatter.write_str("render composition needs at least one layer")
            }
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RenderError {}

/// Safe operations required from a hardware or CPU render backend.
pub trait RenderBackend {
    /// Returns backend capabilities.
    fn capabilities(&self) -> RenderCapabilities;

    /// Returns the current context state.
    fn state(&self) -> RenderState;

    /// Returns resource and data-movement counters collected by the backend.
    #[must_use]
    fn metrics(&self) -> RenderMetrics {
        RenderMetrics::default()
    }

    /// Allocates an empty texture resource.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when the context is lost, the resource limit is
    /// reached, or ID allocation overflows.
    fn create_texture(&mut self, format: VideoFormat) -> Result<TextureId, RenderError>;

    /// Destroys a texture resource.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::UnknownTexture`] for an unknown handle.
    fn destroy_texture(&mut self, texture: TextureId) -> Result<(), RenderError>;

    /// Uploads one owned frame into a texture.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when the context, handle, or format is invalid.
    fn upload(&mut self, texture: TextureId, frame: &VideoFrame) -> Result<(), RenderError>;

    /// Converts and uploads a packed or planar frame through the media boundary.
    ///
    /// The default implementation keeps backend contracts in RGBA8 while
    /// allowing capture adapters to submit their native validated layout.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Media`] for a conversion failure or any error
    /// returned by [`Self::upload`].
    fn upload_raw(&mut self, texture: TextureId, frame: &RawVideoFrame) -> Result<(), RenderError> {
        let rgba = frame.clone().into_rgba8().map_err(RenderError::Media)?;
        self.upload(texture, &rgba)
    }

    /// Composites texture layers in order into a target texture.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when a layer is missing, not uploaded, or has a
    /// format mismatch.
    fn composite(&mut self, target: TextureId, layers: &[TextureId]) -> Result<(), RenderError>;

    /// Reads a texture back into an owned CPU frame.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when readback is unsupported or the texture is not
    /// ready.
    fn readback(&mut self, texture: TextureId) -> Result<VideoFrame, RenderError>;

    /// Recovers a lost context and invalidates uploaded resource contents.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when recovery cannot be completed.
    fn recover(&mut self) -> Result<(), RenderError>;
}

struct Texture {
    format: VideoFormat,
    frame: Option<VideoFrame>,
}

/// Deterministic CPU implementation of [`RenderBackend`].
pub struct CpuRenderBackend {
    capabilities: RenderCapabilities,
    state: RenderState,
    textures: BTreeMap<TextureId, Texture>,
    next_texture: u64,
    allocated_bytes: usize,
    metrics: RenderMetrics,
}

impl CpuRenderBackend {
    /// Creates a CPU backend with a fixed texture-resource limit.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::ZeroCapacity`] when `max_textures` is zero.
    pub fn new(max_textures: usize) -> Result<Self, RenderError> {
        Self::with_limits(max_textures, DEFAULT_MAX_TEXTURE_BYTES)
    }

    /// Creates a CPU backend with texture-count and aggregate-byte limits.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::ZeroCapacity`] when either limit is zero.
    pub fn with_limits(max_textures: usize, max_texture_bytes: usize) -> Result<Self, RenderError> {
        if max_textures == 0 || max_texture_bytes == 0 {
            return Err(RenderError::ZeroCapacity);
        }
        Ok(Self {
            capabilities: RenderCapabilities::with_texture_bytes(
                false,
                true,
                max_textures,
                max_texture_bytes,
            ),
            state: RenderState::Ready,
            textures: BTreeMap::new(),
            next_texture: 1,
            allocated_bytes: 0,
            metrics: RenderMetrics::default(),
        })
    }

    /// Forces a simulated device/context loss.
    pub fn lose_context(&mut self) {
        self.state = RenderState::Lost;
        self.metrics.context_losses = self.metrics.context_losses.saturating_add(1);
    }

    /// Returns the current number of allocated texture handles.
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Returns aggregate RGBA storage currently allocated by textures.
    #[must_use]
    pub const fn allocated_bytes(&self) -> usize {
        self.allocated_bytes
    }

    /// Returns resource and data-movement counters collected by the backend.
    #[must_use]
    pub const fn metrics(&self) -> RenderMetrics {
        self.metrics
    }

    fn ensure_ready(&self) -> Result<(), RenderError> {
        if self.state == RenderState::Ready {
            Ok(())
        } else {
            Err(RenderError::ContextLost)
        }
    }

    fn texture(&self, id: TextureId) -> Result<&Texture, RenderError> {
        self.textures
            .get(&id)
            .ok_or(RenderError::UnknownTexture(id))
    }

    fn texture_mut(&mut self, id: TextureId) -> Result<&mut Texture, RenderError> {
        self.textures
            .get_mut(&id)
            .ok_or(RenderError::UnknownTexture(id))
    }
}

impl RenderBackend for CpuRenderBackend {
    fn capabilities(&self) -> RenderCapabilities {
        self.capabilities
    }

    fn state(&self) -> RenderState {
        self.state
    }

    fn metrics(&self) -> RenderMetrics {
        self.metrics
    }

    fn create_texture(&mut self, format: VideoFormat) -> Result<TextureId, RenderError> {
        self.ensure_ready()?;
        if self.textures.len() >= self.capabilities.max_textures() {
            return Err(RenderError::TextureLimit {
                limit: self.capabilities.max_textures(),
            });
        }
        let requested = format.rgba_bytes();
        let new_total =
            self.allocated_bytes
                .checked_add(requested)
                .ok_or(RenderError::TextureByteLimit {
                    limit: self.capabilities.max_texture_bytes(),
                    requested,
                })?;
        if new_total > self.capabilities.max_texture_bytes() {
            return Err(RenderError::TextureByteLimit {
                limit: self.capabilities.max_texture_bytes(),
                requested,
            });
        }
        let id = TextureId(self.next_texture);
        self.next_texture = self
            .next_texture
            .checked_add(1)
            .ok_or(RenderError::IdExhausted)?;
        self.textures.insert(
            id,
            Texture {
                format,
                frame: None,
            },
        );
        self.allocated_bytes = new_total;
        self.metrics.textures_created = self.metrics.textures_created.saturating_add(1);
        self.metrics.allocated_bytes = new_total;
        self.metrics.peak_allocated_bytes = self.metrics.peak_allocated_bytes.max(new_total);
        Ok(id)
    }

    fn destroy_texture(&mut self, texture: TextureId) -> Result<(), RenderError> {
        self.ensure_ready()?;
        let removed = self
            .textures
            .remove(&texture)
            .ok_or(RenderError::UnknownTexture(texture))?;
        self.allocated_bytes = self
            .allocated_bytes
            .saturating_sub(removed.format.rgba_bytes());
        self.metrics.textures_destroyed = self.metrics.textures_destroyed.saturating_add(1);
        self.metrics.allocated_bytes = self.allocated_bytes;
        Ok(())
    }

    fn upload(&mut self, texture: TextureId, frame: &VideoFrame) -> Result<(), RenderError> {
        self.ensure_ready()?;
        let target = self.texture_mut(texture)?;
        if target.format != frame.format() {
            return Err(RenderError::FormatMismatch {
                expected: target.format,
                actual: frame.format(),
            });
        }
        target.frame = Some(frame.clone());
        self.metrics.uploads = self.metrics.uploads.saturating_add(1);
        Ok(())
    }

    fn composite(&mut self, target: TextureId, layers: &[TextureId]) -> Result<(), RenderError> {
        self.ensure_ready()?;
        if layers.is_empty() {
            return Err(RenderError::EmptyComposition);
        }
        let target_format = self.texture(target)?.format;
        let mut frames = Vec::with_capacity(layers.len());
        for layer in layers {
            let texture = self.texture(*layer)?;
            if texture.format != target_format {
                return Err(RenderError::FormatMismatch {
                    expected: target_format,
                    actual: texture.format,
                });
            }
            frames.push(
                texture
                    .frame
                    .clone()
                    .ok_or(RenderError::TextureNotReady(*layer))?,
            );
        }
        let timestamp = frames
            .first()
            .map_or(Timestamp::ZERO, VideoFrame::timestamp);
        let mut result = VideoFrame::solid(target_format, timestamp, [0, 0, 0, 0]);
        for frame in frames {
            result.blend_over(&frame).map_err(RenderError::Media)?;
        }
        self.texture_mut(target)?.frame = Some(result);
        self.metrics.compositions = self.metrics.compositions.saturating_add(1);
        Ok(())
    }

    fn readback(&mut self, texture: TextureId) -> Result<VideoFrame, RenderError> {
        self.ensure_ready()?;
        let frame = self
            .texture(texture)?
            .frame
            .clone()
            .ok_or(RenderError::TextureNotReady(texture))?;
        self.metrics.readbacks = self.metrics.readbacks.saturating_add(1);
        Ok(frame)
    }

    fn recover(&mut self) -> Result<(), RenderError> {
        self.state = RenderState::Ready;
        for texture in self.textures.values_mut() {
            texture.frame = None;
        }
        self.metrics.recoveries = self.metrics.recoveries.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_media::FrameRate;

    fn format() -> VideoFormat {
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format")
    }

    #[test]
    fn cpu_backend_uploads_composes_and_reads_back() {
        let format = format();
        let mut backend = CpuRenderBackend::new(3).expect("backend");
        let background = backend.create_texture(format).expect("background");
        let foreground = backend.create_texture(format).expect("foreground");
        let target = backend.create_texture(format).expect("target");
        backend
            .upload(
                background,
                &VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 255, 255]),
            )
            .expect("background upload");
        backend
            .upload(
                foreground,
                &VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 128]),
            )
            .expect("foreground upload");
        backend
            .composite(target, &[background, foreground])
            .expect("composition");

        let frame = backend.readback(target).expect("readback");
        assert_eq!(frame.pixel(0, 0), Some([128, 0, 127, 255]));
        assert!(!backend.capabilities().accelerated());
        assert!(backend.capabilities().readback());
        let metrics = backend.metrics();
        assert_eq!(metrics.textures_created(), 3);
        assert_eq!(metrics.textures_destroyed(), 0);
        assert_eq!(metrics.uploads(), 2);
        assert_eq!(metrics.compositions(), 1);
        assert_eq!(metrics.readbacks(), 1);
        assert_eq!(metrics.allocated_bytes(), format.rgba_bytes() * 3);
        assert_eq!(metrics.peak_allocated_bytes(), format.rgba_bytes() * 3);
    }

    #[test]
    fn context_loss_requires_recovery_and_invalidates_contents() {
        let format = format();
        let mut backend = CpuRenderBackend::new(1).expect("backend");
        let texture = backend.create_texture(format).expect("texture");
        backend
            .upload(
                texture,
                &VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255]),
            )
            .expect("upload");
        backend.lose_context();
        assert_eq!(backend.state(), RenderState::Lost);
        assert_eq!(backend.readback(texture), Err(RenderError::ContextLost));
        backend.recover().expect("recover");
        assert_eq!(backend.state(), RenderState::Ready);
        assert_eq!(
            backend.readback(texture),
            Err(RenderError::TextureNotReady(texture))
        );
        let metrics = backend.metrics();
        assert_eq!(metrics.context_losses(), 1);
        assert_eq!(metrics.recoveries(), 1);
        assert_eq!(metrics.readbacks(), 0);
    }

    #[test]
    fn backend_rejects_limits_formats_and_empty_layers() {
        let format = format();
        assert!(matches!(
            CpuRenderBackend::new(0),
            Err(RenderError::ZeroCapacity)
        ));
        let mut backend = CpuRenderBackend::new(1).expect("backend");
        let texture = backend.create_texture(format).expect("texture");
        assert_eq!(
            backend.create_texture(format),
            Err(RenderError::TextureLimit { limit: 1 })
        );
        assert_eq!(
            backend.composite(texture, &[]),
            Err(RenderError::EmptyComposition)
        );
        let other = VideoFormat::new(1, 1, format.frame_rate()).expect("other format");
        assert!(matches!(
            backend.upload(
                texture,
                &VideoFrame::solid(other, Timestamp::ZERO, [0, 0, 0, 255])
            ),
            Err(RenderError::FormatMismatch { .. })
        ));
    }

    #[test]
    fn backend_accounts_texture_bytes_and_accepts_raw_uploads() {
        let format = format();
        let mut backend = CpuRenderBackend::with_limits(2, format.rgba_bytes()).expect("backend");
        let texture = backend.create_texture(format).expect("texture");
        assert_eq!(backend.allocated_bytes(), format.rgba_bytes());
        assert_eq!(
            backend.create_texture(format),
            Err(RenderError::TextureByteLimit {
                limit: format.rgba_bytes(),
                requested: format.rgba_bytes()
            })
        );

        let raw = RawVideoFrame::new(
            format,
            obs_rs_media::PixelFormat::Bgra8,
            Timestamp::ZERO,
            vec![3, 2, 1, 255, 7, 6, 5, 255],
        )
        .expect("raw frame");
        backend.upload_raw(texture, &raw).expect("raw upload");
        assert_eq!(
            backend.readback(texture).expect("readback").pixels(),
            &[1, 2, 3, 255, 5, 6, 7, 255]
        );
        backend.destroy_texture(texture).expect("destroy");
        assert_eq!(backend.allocated_bytes(), 0);
        let metrics = backend.metrics();
        assert_eq!(metrics.textures_created(), 1);
        assert_eq!(metrics.textures_destroyed(), 1);
        assert_eq!(metrics.uploads(), 1);
        assert_eq!(metrics.readbacks(), 1);
        assert_eq!(metrics.allocated_bytes(), 0);
        assert_eq!(metrics.peak_allocated_bytes(), format.rgba_bytes());
    }
}
