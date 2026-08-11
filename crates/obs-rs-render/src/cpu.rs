use std::collections::HashMap;

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    backend::RenderBackend,
    error::RenderError,
    types::{RenderCapabilities, RenderMetrics, RenderState, TextureId},
    DEFAULT_MAX_TEXTURE_BYTES,
};
struct Texture {
    format: VideoFormat,
    frame: Option<VideoFrame>,
}

/// Deterministic CPU implementation of [`RenderBackend`].
pub struct CpuRenderBackend {
    capabilities: RenderCapabilities,
    state: RenderState,
    /// Keyed lookup only: the catalog is never iterated in key order, so a hash
    /// map gives O(1) access without costing determinism.
    textures: HashMap<TextureId, Texture>,
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
            textures: HashMap::new(),
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
        self.upload_owned(texture, frame.clone())
    }

    fn upload_owned(&mut self, texture: TextureId, frame: VideoFrame) -> Result<(), RenderError> {
        self.ensure_ready()?;
        let target = self.texture_mut(texture)?;
        if target.format != frame.format() {
            return Err(RenderError::FormatMismatch {
                expected: target.format,
                actual: frame.format(),
            });
        }
        target.frame = Some(frame);
        self.metrics.uploads = self.metrics.uploads.saturating_add(1);
        Ok(())
    }

    fn composite(&mut self, target: TextureId, layers: &[TextureId]) -> Result<(), RenderError> {
        self.ensure_ready()?;
        if layers.is_empty() {
            return Err(RenderError::EmptyComposition);
        }
        let target_format = self.texture(target)?.format;

        // Validate every layer up front so a rejected composition leaves the
        // backend untouched, then blend from borrowed frames. The target is only
        // written at the end, so a layer that aliases the target still reads its
        // pre-composition contents without needing a defensive copy.
        for layer in layers {
            let texture = self.texture(*layer)?;
            if texture.format != target_format {
                return Err(RenderError::FormatMismatch {
                    expected: target_format,
                    actual: texture.format,
                });
            }
            if texture.frame.is_none() {
                return Err(RenderError::TextureNotReady(*layer));
            }
        }

        let timestamp = layers
            .first()
            .and_then(|layer| self.textures.get(layer))
            .and_then(|texture| texture.frame.as_ref())
            .map_or(Timestamp::ZERO, VideoFrame::timestamp);
        let can_reuse_first = !layers[1..].contains(&target)
            && !layers[1..].contains(layers.first().expect("composition is non-empty"));
        let mut result = if can_reuse_first {
            let first = *layers.first().expect("composition is non-empty");
            let mut frame = self
                .texture_mut(first)?
                .frame
                .take()
                .ok_or(RenderError::TextureNotReady(first))?;
            frame.clear_transparent_rgb();
            frame
        } else {
            VideoFrame::solid(target_format, timestamp, [0, 0, 0, 0])
        };
        let start = usize::from(can_reuse_first);
        for layer in layers.iter().skip(start) {
            let frame = self
                .textures
                .get(layer)
                .and_then(|texture| texture.frame.as_ref())
                .ok_or(RenderError::TextureNotReady(*layer))?;
            result.blend_over(frame).map_err(RenderError::Media)?;
        }
        self.texture_mut(target)?.frame = Some(result);
        self.metrics.compositions = self.metrics.compositions.saturating_add(1);
        Ok(())
    }

    fn readback(&mut self, texture: TextureId) -> Result<VideoFrame, RenderError> {
        self.ensure_ready()?;
        let frame = self
            .texture_mut(texture)?
            .frame
            .take()
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
