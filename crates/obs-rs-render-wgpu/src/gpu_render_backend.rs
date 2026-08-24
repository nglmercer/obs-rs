use super::{
    compositor::write_texture,
    helpers::{gpu_compositor, install_device_loss_handler, read_texture, request_device},
    nv12::nv12_converter,
};
use super::{
    FrameFilter, FrameTransform, GpuTexture, OpaqueFrameSurface, Ordering, RenderBackend,
    RenderCapabilities, RenderError, RenderMetrics, RenderState, SceneLayer, SurfaceImportMode,
    TextureId, Timestamp, VideoFormat, VideoFrame, WgpuRenderBackend,
};

impl RenderBackend for WgpuRenderBackend {
    fn capabilities(&self) -> RenderCapabilities {
        self.capabilities
    }

    fn state(&self) -> RenderState {
        if self.device_lost.load(Ordering::Acquire) {
            RenderState::Lost
        } else {
            self.state
        }
    }

    fn metrics(&self) -> RenderMetrics {
        self.metrics
    }

    fn surface_import_mode(&self, _provider: &str) -> SurfaceImportMode {
        SurfaceImportMode::CpuFallback
    }

    fn submit_surface(
        &mut self,
        _texture: TextureId,
        surface: &OpaqueFrameSurface,
    ) -> Result<(), RenderError> {
        Err(RenderError::SurfaceUnsupported {
            provider: surface.provider().to_owned(),
        })
    }

    fn submit_layers(
        &mut self,
        target: TextureId,
        layers: &[SceneLayer<'_>],
    ) -> Result<(), RenderError> {
        self.ensure_ready()?;
        if layers.is_empty() {
            return Err(RenderError::EmptyComposition);
        }
        let mut prepared = Vec::with_capacity(layers.len());
        let mut timestamp = Timestamp::ZERO;
        for layer in layers {
            let frame = match layer.input() {
                obs_rs_render::LayerInput::Frame(frame) => frame,
                obs_rs_render::LayerInput::Surface(surface) => {
                    return Err(RenderError::SurfaceUnsupported {
                        provider: surface.provider().to_owned(),
                    });
                }
            };
            timestamp = frame.timestamp();
            let source_format = frame.format();
            self.metrics.record_upload_bytes(source_format.rgba_bytes());
            let texture = self.acquire_texture(source_format);
            write_texture(&self.queue, &texture, frame);
            prepared.push((
                texture,
                source_format,
                frame.timestamp(),
                layer.transform(),
                layer.filters(),
            ));
        }
        self.composite_textures(
            target,
            prepared
                .iter()
                .map(|(texture, source_format, timestamp, transform, filters)| {
                    (texture, *source_format, *timestamp, *transform, *filters)
                }),
        )?;
        for (texture, source_format, _, _, _) in prepared {
            self.recycle_texture(source_format, texture);
        }
        let target = self
            .textures
            .get_mut(&target)
            .ok_or(RenderError::UnknownTexture(target))?;
        target.uploaded = true;
        target.timestamp = timestamp;
        self.metrics.record_composition();
        Ok(())
    }

    fn create_texture(&mut self, format: VideoFormat) -> Result<TextureId, RenderError> {
        self.ensure_ready()?;
        let id = self.cpu.create_texture(format)?;
        self.textures.insert(
            id,
            GpuTexture {
                format,
                texture: Self::gpu_texture(format, &self.device),
                uploaded: false,
                timestamp: Timestamp::ZERO,
            },
        );
        self.metrics.record_texture_created(format.rgba_bytes());
        Ok(id)
    }

    fn destroy_texture(&mut self, texture: TextureId) -> Result<(), RenderError> {
        self.ensure_ready()?;
        let format = self
            .textures
            .get(&texture)
            .ok_or(RenderError::UnknownTexture(texture))?
            .format;
        self.cpu.destroy_texture(texture)?;
        self.textures
            .remove(&texture)
            .ok_or(RenderError::UnknownTexture(texture))?;
        self.metrics.record_texture_destroyed(format.rgba_bytes());
        Ok(())
    }

    fn upload(&mut self, texture: TextureId, frame: &VideoFrame) -> Result<(), RenderError> {
        self.ensure_ready()?;
        self.write_gpu(texture, frame)?;
        let gpu = self
            .textures
            .get_mut(&texture)
            .ok_or(RenderError::UnknownTexture(texture))?;
        gpu.uploaded = true;
        gpu.timestamp = frame.timestamp();
        self.metrics
            .record_upload_bytes(frame.format().rgba_bytes());
        Ok(())
    }

    fn upload_owned(&mut self, texture: TextureId, frame: VideoFrame) -> Result<(), RenderError> {
        self.ensure_ready()?;
        self.upload(texture, &frame)
    }

    fn composite(&mut self, target: TextureId, layers: &[TextureId]) -> Result<(), RenderError> {
        self.ensure_ready()?;
        if layers.is_empty() {
            return Err(RenderError::EmptyComposition);
        }
        let target_format = self
            .textures
            .get(&target)
            .ok_or(RenderError::UnknownTexture(target))?
            .format;
        let timestamp = self
            .textures
            .get(&layers[0])
            .ok_or(RenderError::UnknownTexture(layers[0]))?
            .timestamp;
        for layer in layers {
            let texture = self
                .textures
                .get(layer)
                .ok_or(RenderError::UnknownTexture(*layer))?;
            if texture.format != target_format {
                return Err(RenderError::FormatMismatch {
                    expected: target_format,
                    actual: texture.format,
                });
            }
            if !texture.uploaded {
                return Err(RenderError::TextureNotReady(*layer));
            }
        }
        // The target may not alias an input because render attachments cannot
        // simultaneously be sampled. Scene compositors allocate a distinct target.
        if layers.contains(&target) {
            return Err(RenderError::Backend {
                message: "GPU composition target aliases a source".to_owned(),
            });
        }
        self.composite_textures(
            target,
            layers.iter().map(|layer| {
                let texture = self.textures.get(layer).expect("validated layer");
                (
                    &texture.texture,
                    texture.format,
                    texture.timestamp,
                    FrameTransform::IDENTITY,
                    &[] as &[FrameFilter],
                )
            }),
        )?;
        let target = self
            .textures
            .get_mut(&target)
            .ok_or(RenderError::UnknownTexture(target))?;
        target.uploaded = true;
        target.timestamp = timestamp;
        self.metrics.record_composition();
        Ok(())
    }

    fn readback(&mut self, texture: TextureId) -> Result<VideoFrame, RenderError> {
        self.ensure_ready()?;
        let gpu = self
            .textures
            .get(&texture)
            .ok_or(RenderError::UnknownTexture(texture))?;
        if !gpu.uploaded {
            return Err(RenderError::TextureNotReady(texture));
        }
        let frame = read_texture(&self.device, &self.queue, gpu)?;
        self.metrics.record_readback_bytes(gpu.format.rgba_bytes());
        Ok(frame)
    }

    fn recover(&mut self) -> Result<(), RenderError> {
        let device_reported_loss = self.device_lost.load(Ordering::Acquire);
        if self.state == RenderState::Ready && !device_reported_loss {
            return Ok(());
        }
        if self.state == RenderState::Ready {
            self.metrics.record_context_loss();
        }
        let (device, queue) =
            request_device(&self.adapter).map_err(|error| RenderError::Backend {
                message: error.to_string(),
            })?;
        self.device = device;
        self.queue = queue;
        self.device_lost = install_device_loss_handler(&self.device);
        self.texture_pool.get_mut().clear();
        self.rgba_readbacks.clear();
        self.readback_sequence = 0;
        (
            self.bind_group_layout,
            self.replace_pipeline,
            self.composite_pipeline,
        ) = gpu_compositor(&self.device);
        self.nv12 = nv12_converter(&self.device);
        self.nv12_pipeline_builds = self.nv12_pipeline_builds.saturating_add(1);
        for texture in self.textures.values_mut() {
            texture.texture = Self::gpu_texture(texture.format, &self.device);
            texture.uploaded = false;
            texture.timestamp = Timestamp::ZERO;
        }
        self.cpu.recover()?;
        self.state = RenderState::Ready;
        self.metrics.record_recovery();
        Ok(())
    }
}
