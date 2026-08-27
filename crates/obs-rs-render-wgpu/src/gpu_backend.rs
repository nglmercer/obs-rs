use super::{
    compositor::write_texture,
    helpers::{gpu_compositor, install_device_loss_handler, readback_buffer, request_device},
    nv12::{async_nv12_readback, nv12_buffer_size, nv12_converter},
};
use super::{
    Arc, AsyncRgbaReadback, AtomicU8, CpuRenderBackend, HashMap, Nv12Metrics, Ordering,
    PixelFormat, RawVideoFrame, RefCell, RenderCapabilities, RenderError, RenderMetrics,
    RenderState, TextureId, Timestamp, VideoFormat, VideoFrame, WgpuAdapterCapabilities,
    WgpuBackendError, WgpuRenderBackend, NV12_READBACK_RING_CAPACITY, READBACK_COMPLETE,
    READBACK_FAILED, READBACK_IDLE, READBACK_IN_FLIGHT, READBACK_RING_CAPACITY,
};

impl WgpuRenderBackend {
    /// Selects a high-performance adapter and creates bounded texture pools.
    ///
    /// # Errors
    ///
    /// Returns a typed initialization error when no adapter/device is available.
    pub fn new(max_textures: usize, max_texture_bytes: usize) -> Result<Self, WgpuBackendError> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok_or_else(|| WgpuBackendError("no compatible wgpu adapter".to_owned()))?;
        let (device, queue) = request_device(&adapter)?;
        let device_lost = install_device_loss_handler(&device);
        let (bind_group_layout, replace_pipeline, composite_pipeline) = gpu_compositor(&device);
        let nv12 = nv12_converter(&device);
        let info = adapter.get_info();
        let cpu = CpuRenderBackend::with_limits(max_textures, max_texture_bytes)
            .map_err(|error| WgpuBackendError(error.to_string()))?;
        Ok(Self {
            adapter,
            device,
            queue,
            cpu,
            textures: HashMap::new(),
            texture_pool: RefCell::new(Vec::new()),
            bind_group_layout,
            replace_pipeline,
            composite_pipeline,
            nv12,
            nv12_pipeline_builds: 1,
            nv12_metrics: Nv12Metrics::default(),
            rgba_readbacks: Vec::new(),
            readback_sequence: 0,
            metrics: RenderMetrics::default(),
            capabilities: RenderCapabilities::with_texture_bytes(
                true,
                true,
                max_textures,
                max_texture_bytes,
            ),
            adapter_capabilities: WgpuAdapterCapabilities {
                name: info.name,
                backend: format!("{:?}", info.backend),
                device_type: format!("{:?}", info.device_type),
                // Generic wgpu 0.20 has no portable external-memory import API.
                direct_surface_providers: Vec::new(),
            },
            state: RenderState::Ready,
            device_lost,
        })
    }

    #[must_use]
    pub const fn adapter_capabilities(&self) -> &WgpuAdapterCapabilities {
        &self.adapter_capabilities
    }

    /// Waits for submitted GPU work to finish for benchmarking/diagnostics.
    /// Live render and capture callbacks should never call this method.
    pub fn wait_idle(&self) {
        self.device.poll(wgpu::Maintain::Wait);
    }

    /// Returns how many times the reusable NV12 pipeline has been built.
    ///
    /// This is primarily useful for diagnostics and regression tests: normal
    /// frame conversion should leave the value unchanged.
    #[must_use]
    pub const fn nv12_pipeline_builds(&self) -> u64 {
        self.nv12_pipeline_builds
    }

    /// Returns live counters for the encoder conversion bridge.
    #[must_use]
    pub const fn nv12_metrics(&self) -> Nv12Metrics {
        self.nv12_metrics
    }

    /// Schedules a bounded RGBA readback without waiting for the GPU.
    ///
    /// The returned boolean is false when all bounded staging slots are still
    /// in flight. Callers should keep their previous presentation in that
    /// case; realtime preview prefers one dropped frame to a render-worker
    /// stall.
    ///
    /// # Errors
    ///
    /// Returns an error when the device is lost, the texture is unknown, or
    /// the texture has not received an upload.
    #[allow(
        clippy::too_many_lines,
        reason = "the bounded submission owns validation, copy, and map setup"
    )]
    pub fn submit_readback(&mut self, texture_id: TextureId) -> Result<bool, RenderError> {
        self.ensure_ready()?;
        let (format, timestamp) = self
            .textures
            .get(&texture_id)
            .ok_or(RenderError::UnknownTexture(texture_id))
            .map(|texture| (texture.format, texture.timestamp))?;
        if !self
            .textures
            .get(&texture_id)
            .is_some_and(|texture| texture.uploaded)
        {
            return Err(RenderError::TextureNotReady(texture_id));
        }
        let unpadded_row = format.width().saturating_mul(4);
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(alignment) * alignment;
        let buffer_size = u64::from(padded_row) * u64::from(format.height());
        let Some(index) = self
            .rgba_readbacks
            .iter()
            .position(|slot| slot.status.load(Ordering::Acquire) == READBACK_IDLE)
            .or_else(|| {
                (self.rgba_readbacks.len() < READBACK_RING_CAPACITY)
                    .then_some(self.rgba_readbacks.len())
            })
        else {
            return Ok(false);
        };
        if index == self.rgba_readbacks.len() {
            self.rgba_readbacks.push(AsyncRgbaReadback {
                buffer: readback_buffer(&self.device, buffer_size),
                status: Arc::new(AtomicU8::new(READBACK_IDLE)),
                buffer_size,
                unpadded_row,
                padded_row,
                texture: texture_id,
                format,
                timestamp,
                sequence: 0,
            });
        } else if self.rgba_readbacks[index].buffer_size != buffer_size {
            self.rgba_readbacks[index] = AsyncRgbaReadback {
                buffer: readback_buffer(&self.device, buffer_size),
                status: Arc::new(AtomicU8::new(READBACK_IDLE)),
                buffer_size,
                unpadded_row,
                padded_row,
                texture: texture_id,
                format,
                timestamp,
                sequence: 0,
            };
        }
        let slot = &mut self.rgba_readbacks[index];
        slot.status.store(READBACK_IN_FLIGHT, Ordering::Release);
        slot.unpadded_row = unpadded_row;
        slot.padded_row = padded_row;
        slot.texture = texture_id;
        slot.format = format;
        slot.timestamp = timestamp;
        slot.sequence = self.readback_sequence;
        self.readback_sequence = self.readback_sequence.wrapping_add(1);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("obs-rs-async-readback-copy"),
            });
        let texture = self
            .textures
            .get(&texture_id)
            .ok_or(RenderError::UnknownTexture(texture_id))?;
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &slot.buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(format.height()),
                },
            },
            wgpu::Extent3d {
                width: format.width(),
                height: format.height(),
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let status = Arc::clone(&slot.status);
        slot.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                status.store(
                    if result.is_ok() {
                        READBACK_COMPLETE
                    } else {
                        READBACK_FAILED
                    },
                    Ordering::Release,
                );
            });
        Ok(true)
    }

    /// Polls completed asynchronous RGBA readbacks without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when the device is lost or a completed frame violates
    /// the validated media buffer contract.
    pub fn poll_readbacks(&mut self) -> Result<Vec<(TextureId, VideoFrame)>, RenderError> {
        self.ensure_ready()?;
        self.device.poll(wgpu::Maintain::Poll);
        let mut completed = Vec::new();
        for slot in &mut self.rgba_readbacks {
            match slot.status.load(Ordering::Acquire) {
                READBACK_COMPLETE => {
                    let mapped = slot.buffer.slice(..).get_mapped_range();
                    let mut pixels = Vec::with_capacity(slot.format.rgba_bytes());
                    for row in mapped.chunks_exact(slot.padded_row as usize) {
                        pixels.extend_from_slice(&row[..slot.unpadded_row as usize]);
                    }
                    drop(mapped);
                    slot.buffer.unmap();
                    slot.status.store(READBACK_IDLE, Ordering::Release);
                    let frame = VideoFrame::new(slot.format, slot.timestamp, pixels)
                        .map_err(RenderError::Media)?;
                    self.metrics.record_readback_bytes(slot.format.rgba_bytes());
                    completed.push((slot.texture, frame));
                }
                READBACK_FAILED => {
                    slot.status.store(READBACK_IDLE, Ordering::Release);
                }
                _ => {}
            }
        }
        completed.sort_by_key(|(texture, _)| texture.value());
        Ok(completed)
    }

    /// Reports whether a texture already owns an in-flight staging slot.
    #[must_use]
    pub fn readback_pending(&self, texture_id: TextureId) -> bool {
        self.rgba_readbacks.iter().any(|slot| {
            slot.texture == texture_id && slot.status.load(Ordering::Acquire) == READBACK_IN_FLIGHT
        })
    }

    fn nv12_texture_info(
        &self,
        texture_id: TextureId,
    ) -> Result<(VideoFormat, Timestamp, wgpu::TextureView), RenderError> {
        let texture = self
            .textures
            .get(&texture_id)
            .ok_or(RenderError::UnknownTexture(texture_id))?;
        if !texture.uploaded {
            return Err(RenderError::TextureNotReady(texture_id));
        }
        Ok((
            texture.format,
            texture.timestamp,
            texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        ))
    }

    /// Schedules a bounded asynchronous RGBA-to-NV12 conversion.
    ///
    /// The compute pipeline and staging buffers are retained by the backend;
    /// this call only records a dispatch/copy and maps an available ring slot.
    /// A false result means all bounded encoder slots are still in flight.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown/unready texture, invalid 4:2:0
    /// dimensions, or a lost device.
    pub fn submit_nv12_readback(&mut self, texture_id: TextureId) -> Result<bool, RenderError> {
        self.ensure_ready()?;
        let (format, timestamp, view) = self.nv12_texture_info(texture_id)?;
        let (byte_len, buffer_size) = nv12_buffer_size(format)?;
        let Some(index) = self
            .nv12
            .async_staging
            .iter()
            .position(|slot| slot.status.load(Ordering::Acquire) == READBACK_IDLE)
            .or_else(|| {
                (self.nv12.async_staging.len() < NV12_READBACK_RING_CAPACITY)
                    .then_some(self.nv12.async_staging.len())
            })
        else {
            self.nv12_metrics.staging_waits = self.nv12_metrics.staging_waits.saturating_add(1);
            self.nv12_metrics.frames_dropped = self.nv12_metrics.frames_dropped.saturating_add(1);
            return Ok(false);
        };
        if index == self.nv12.async_staging.len() {
            self.nv12.async_staging.push(async_nv12_readback(
                &self.device,
                buffer_size,
                byte_len,
                texture_id,
                format,
                timestamp,
            ));
        } else if self.nv12.async_staging[index].buffer_size < buffer_size {
            self.nv12.async_staging[index] = async_nv12_readback(
                &self.device,
                buffer_size,
                byte_len,
                texture_id,
                format,
                timestamp,
            );
        }
        let slot = &mut self.nv12.async_staging[index];
        slot.status.store(READBACK_IN_FLIGHT, Ordering::Release);
        slot.byte_len = byte_len;
        slot.texture = texture_id;
        slot.format = format;
        slot.timestamp = timestamp;
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("obs-rs-nv12-async-bind-group"),
            layout: &self.nv12.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: slot.output.as_entire_binding(),
                },
            ],
        });
        let word_count = byte_len.div_ceil(4);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("obs-rs-nv12-async-conversion"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("obs-rs-nv12-async-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.nv12.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = u32::try_from(word_count.div_ceil(64)).unwrap_or(u32::MAX);
            pass.dispatch_workgroups(groups.min(65_535), groups.div_ceil(65_535), 1);
        }
        encoder.copy_buffer_to_buffer(&slot.output, 0, &slot.readback, 0, buffer_size);
        self.queue.submit(Some(encoder.finish()));
        let status = Arc::clone(&slot.status);
        slot.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                status.store(
                    if result.is_ok() {
                        READBACK_COMPLETE
                    } else {
                        READBACK_FAILED
                    },
                    Ordering::Release,
                );
            });
        Ok(true)
    }

    /// Polls completed asynchronous NV12 conversions without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when the device is lost or a completed payload fails
    /// the validated raw-frame contract.
    pub fn poll_nv12_readbacks(&mut self) -> Result<Vec<(TextureId, RawVideoFrame)>, RenderError> {
        self.ensure_ready()?;
        self.device.poll(wgpu::Maintain::Poll);
        let mut completed = Vec::new();
        for slot in &mut self.nv12.async_staging {
            match slot.status.load(Ordering::Acquire) {
                READBACK_COMPLETE => {
                    let mapped = slot.readback.slice(..).get_mapped_range();
                    let bytes = mapped[..slot.byte_len].to_vec();
                    drop(mapped);
                    slot.readback.unmap();
                    slot.status.store(READBACK_IDLE, Ordering::Release);
                    let frame =
                        RawVideoFrame::new(slot.format, PixelFormat::Nv12, slot.timestamp, bytes)
                            .map_err(RenderError::Media)?;
                    self.metrics.record_color_conversion();
                    self.metrics.record_readback_bytes(slot.byte_len);
                    self.nv12_metrics.conversions = self.nv12_metrics.conversions.saturating_add(1);
                    self.nv12_metrics.readbacks = self.nv12_metrics.readbacks.saturating_add(1);
                    self.nv12_metrics.bytes_transferred = self
                        .nv12_metrics
                        .bytes_transferred
                        .saturating_add(u64::try_from(slot.byte_len).unwrap_or(u64::MAX));
                    completed.push((slot.texture, frame));
                }
                READBACK_FAILED => {
                    slot.status.store(READBACK_IDLE, Ordering::Release);
                }
                _ => {}
            }
        }
        Ok(completed)
    }

    /// Reports whether an encoder conversion is already in flight for a
    /// target texture.
    #[must_use]
    pub fn nv12_readback_pending(&self, texture_id: TextureId) -> bool {
        self.nv12.async_staging.iter().any(|slot| {
            slot.texture == texture_id && slot.status.load(Ordering::Acquire) == READBACK_IN_FLIGHT
        })
    }

    /// Converts one composed RGBA texture to packed NV12 on the GPU and reads
    /// back only the encoder-oriented 4:2:0 payload.
    ///
    /// This is the compatibility bridge for encoders that cannot yet import a
    /// WGPU texture directly. Color math remains on the GPU and the transfer is
    /// 62.5% smaller than an RGBA readback.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown/unready texture, odd 4:2:0 dimensions,
    /// device loss, or a failed GPU mapping operation.
    #[allow(
        clippy::too_many_lines,
        reason = "GPU conversion owns dispatch and the explicit compatibility readback"
    )]
    pub fn readback_nv12(&mut self, texture_id: TextureId) -> Result<RawVideoFrame, RenderError> {
        self.ensure_ready()?;
        let (format, timestamp, view) = {
            let texture = self
                .textures
                .get(&texture_id)
                .ok_or(RenderError::UnknownTexture(texture_id))?;
            if !texture.uploaded {
                return Err(RenderError::TextureNotReady(texture_id));
            }
            (
                texture.format,
                texture.timestamp,
                texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            )
        };
        let byte_len = PixelFormat::Nv12
            .bytes_for(format)
            .map_err(RenderError::Media)?;
        let word_count = byte_len.div_ceil(4);
        let buffer_size = u64::try_from(word_count)
            .unwrap_or(u64::MAX / 4)
            .saturating_mul(4);
        self.nv12.ensure_staging(&self.device, buffer_size);
        let Some(staging) = self.nv12.staging.as_ref() else {
            return Err(RenderError::Backend {
                message: "NV12 staging allocation was not initialized".to_owned(),
            });
        };
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("obs-rs-nv12-bind-group"),
            layout: &self.nv12.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: staging.output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("obs-rs-nv12-conversion"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("obs-rs-nv12-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.nv12.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = u32::try_from(word_count.div_ceil(64)).unwrap_or(u32::MAX);
            let groups_x = groups.min(65_535);
            let groups_y = groups.div_ceil(65_535);
            pass.dispatch_workgroups(groups_x, groups_y, 1);
        }
        encoder.copy_buffer_to_buffer(&staging.output, 0, &staging.readback, 0, buffer_size);
        self.queue.submit(Some(encoder.finish()));
        let slice = staging.readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        self.nv12_metrics.staging_waits = self.nv12_metrics.staging_waits.saturating_add(1);
        receiver
            .recv()
            .map_err(|error| RenderError::Backend {
                message: format!("NV12 readback callback failed: {error}"),
            })?
            .map_err(|error| RenderError::Backend {
                message: format!("NV12 readback mapping failed: {error}"),
            })?;
        let mapped = slice.get_mapped_range();
        let bytes = mapped[..byte_len].to_vec();
        drop(mapped);
        staging.readback.unmap();
        let frame = RawVideoFrame::new(format, PixelFormat::Nv12, timestamp, bytes)
            .map_err(RenderError::Media)?;
        self.metrics.record_color_conversion();
        self.metrics.record_readback_bytes(byte_len);
        self.nv12_metrics.conversions = self.nv12_metrics.conversions.saturating_add(1);
        self.nv12_metrics.readbacks = self.nv12_metrics.readbacks.saturating_add(1);
        self.nv12_metrics.bytes_transferred = self
            .nv12_metrics
            .bytes_transferred
            .saturating_add(u64::try_from(byte_len).unwrap_or(u64::MAX));
        Ok(frame)
    }

    /// Returns reusable textures currently retained by the bounded pool.
    #[must_use]
    pub fn pooled_texture_count(&self) -> usize {
        self.texture_pool.borrow().len()
    }

    /// Estimates target plus pooled RGBA texture memory visible to this backend.
    #[must_use]
    pub fn estimated_gpu_bytes(&self) -> usize {
        let targets = self.textures.values().fold(0_usize, |bytes, texture| {
            bytes.saturating_add(texture.format.rgba_bytes())
        });
        self.texture_pool
            .borrow()
            .iter()
            .fold(targets, |bytes, (format, _)| {
                bytes.saturating_add(format.rgba_bytes())
            })
    }

    /// Marks resources lost for deterministic recovery testing.
    pub fn lose_device(&mut self) {
        self.state = RenderState::Lost;
        self.device_lost.store(true, Ordering::Release);
        self.cpu.lose_context();
        self.metrics.record_context_loss();
    }

    pub(super) fn ensure_ready(&self) -> Result<(), RenderError> {
        if self.state == RenderState::Ready && !self.device_lost.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(RenderError::ContextLost)
        }
    }

    pub(super) fn gpu_texture(format: VideoFormat, device: &wgpu::Device) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obs-rs-frame"),
            size: wgpu::Extent3d {
                width: format.width(),
                height: format.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    pub(super) fn write_gpu(
        &self,
        texture: TextureId,
        frame: &VideoFrame,
    ) -> Result<(), RenderError> {
        let gpu = self
            .textures
            .get(&texture)
            .ok_or(RenderError::UnknownTexture(texture))?;
        if gpu.format != frame.format() {
            return Err(RenderError::FormatMismatch {
                expected: gpu.format,
                actual: frame.format(),
            });
        }
        write_texture(&self.queue, &gpu.texture, frame);
        Ok(())
    }

    pub(super) fn acquire_texture(&self, format: VideoFormat) -> wgpu::Texture {
        let mut pool = self.texture_pool.borrow_mut();
        if let Some(index) = pool.iter().position(|(candidate, _)| *candidate == format) {
            return pool.swap_remove(index).1;
        }
        drop(pool);
        Self::gpu_texture(format, &self.device)
    }

    pub(super) fn recycle_texture(&self, format: VideoFormat, texture: wgpu::Texture) {
        let mut pool = self.texture_pool.borrow_mut();
        if pool.len() < self.capabilities.max_textures().min(32) {
            pool.push((format, texture));
        }
    }
}
