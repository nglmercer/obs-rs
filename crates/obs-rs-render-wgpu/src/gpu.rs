use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
};

use obs_rs_media::{
    FrameFilter, FrameTransform, PixelFormat, RawVideoFrame, Timestamp, VideoFormat, VideoFrame,
};
use obs_rs_render::{
    CpuRenderBackend, OpaqueFrameSurface, RenderBackend, RenderCapabilities, RenderError,
    RenderMetrics, RenderState, SceneLayer, SurfaceImportMode, TextureId,
};
use wgpu::util::DeviceExt;

mod parameters;

use parameters::layer_parameters;

struct GpuTexture {
    format: VideoFormat,
    texture: wgpu::Texture,
    uploaded: bool,
    timestamp: Timestamp,
}

const READBACK_IDLE: u8 = 0;
const READBACK_IN_FLIGHT: u8 = 1;
const READBACK_COMPLETE: u8 = 2;
const READBACK_FAILED: u8 = 3;
// One bounded slot per desktop/output consumer plus a small amount of
// overlap. The ring is shared by targets, so three slots would let four
// simultaneous consumers starve the first target even though each consumer is
// individually latest-frame-wins.
const READBACK_RING_CAPACITY: usize = 8;
const NV12_READBACK_RING_CAPACITY: usize = 3;

struct AsyncRgbaReadback {
    buffer: wgpu::Buffer,
    status: Arc<AtomicU8>,
    buffer_size: u64,
    unpadded_row: u32,
    padded_row: u32,
    texture: TextureId,
    format: VideoFormat,
    timestamp: Timestamp,
    sequence: u64,
}

struct Nv12Staging {
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    size: u64,
}

struct AsyncNv12Readback {
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    status: Arc<AtomicU8>,
    buffer_size: u64,
    byte_len: usize,
    texture: TextureId,
    format: VideoFormat,
    timestamp: Timestamp,
}

/// Persistent GPU resources for the encoder compatibility conversion.
///
/// The staging buffers grow only when a larger format is requested and are
/// reused thereafter. The synchronous map in `readback_nv12` is retained as
/// an explicit compatibility boundary; the expensive shader and pipeline
/// compilation is never part of the frame loop.
struct Nv12Converter {
    _shader: wgpu::ShaderModule,
    bind_group_layout: wgpu::BindGroupLayout,
    _pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::ComputePipeline,
    staging: Option<Nv12Staging>,
    async_staging: Vec<AsyncNv12Readback>,
}

/// Live counters for the bounded encoder conversion bridge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Nv12Metrics {
    conversions: u64,
    readbacks: u64,
    bytes_transferred: u64,
    staging_waits: u64,
    frames_dropped: u64,
}

impl Nv12Metrics {
    /// Number of completed GPU color conversions.
    #[must_use]
    pub const fn conversions(self) -> u64 {
        self.conversions
    }

    /// Number of completed NV12 payload readbacks.
    #[must_use]
    pub const fn readbacks(self) -> u64 {
        self.readbacks
    }

    /// Number of NV12 payload bytes transferred to the CPU.
    #[must_use]
    pub const fn bytes_transferred(self) -> u64 {
        self.bytes_transferred
    }

    /// Number of times the bounded staging ring had no available slot.
    #[must_use]
    pub const fn staging_waits(self) -> u64 {
        self.staging_waits
    }

    /// Number of conversions dropped because all staging slots were busy.
    #[must_use]
    pub const fn frames_dropped(self) -> u64 {
        self.frames_dropped
    }
}

/// Device details safe to expose in diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuAdapterCapabilities {
    name: String,
    backend: String,
    device_type: String,
    direct_surface_providers: Vec<&'static str>,
}

impl WgpuAdapterCapabilities {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    #[must_use]
    pub fn device_type(&self) -> &str {
        &self.device_type
    }

    #[must_use]
    pub fn direct_surface_providers(&self) -> &[&'static str] {
        &self.direct_surface_providers
    }
}

#[derive(Debug)]
pub struct WgpuBackendError(String);

impl fmt::Display for WgpuBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WgpuBackendError {}

/// A `wgpu` device/texture backend retaining the CPU oracle for compatibility
/// readback and pixel-equivalence diagnostics.
pub struct WgpuRenderBackend {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    cpu: CpuRenderBackend,
    textures: HashMap<TextureId, GpuTexture>,
    texture_pool: RefCell<Vec<(VideoFormat, wgpu::Texture)>>,
    bind_group_layout: wgpu::BindGroupLayout,
    replace_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    nv12: Nv12Converter,
    nv12_pipeline_builds: u64,
    nv12_metrics: Nv12Metrics,
    rgba_readbacks: Vec<AsyncRgbaReadback>,
    readback_sequence: u64,
    metrics: RenderMetrics,
    capabilities: RenderCapabilities,
    adapter_capabilities: WgpuAdapterCapabilities,
    state: RenderState,
    device_lost: Arc<AtomicBool>,
}

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

    fn ensure_ready(&self) -> Result<(), RenderError> {
        if self.state == RenderState::Ready && !self.device_lost.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(RenderError::ContextLost)
        }
    }

    fn gpu_texture(format: VideoFormat, device: &wgpu::Device) -> wgpu::Texture {
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

    fn write_gpu(&self, texture: TextureId, frame: &VideoFrame) -> Result<(), RenderError> {
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

    fn acquire_texture(&self, format: VideoFormat) -> wgpu::Texture {
        let mut pool = self.texture_pool.borrow_mut();
        if let Some(index) = pool.iter().position(|(candidate, _)| *candidate == format) {
            return pool.swap_remove(index).1;
        }
        drop(pool);
        Self::gpu_texture(format, &self.device)
    }

    fn recycle_texture(&self, format: VideoFormat, texture: wgpu::Texture) {
        let mut pool = self.texture_pool.borrow_mut();
        if pool.len() < self.capabilities.max_textures().min(32) {
            pool.push((format, texture));
        }
    }
}

fn write_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, frame: &VideoFrame) {
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        frame.pixels(),
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(frame.format().width() * 4),
            rows_per_image: Some(frame.format().height()),
        },
        wgpu::Extent3d {
            width: frame.format().width(),
            height: frame.format().height(),
            depth_or_array_layers: 1,
        },
    );
}

impl WgpuRenderBackend {
    #[allow(
        clippy::too_many_lines,
        reason = "one encoder owns both ping-pong textures and every ordered layer pass"
    )]
    fn composite_textures<'a, I>(&self, target: TextureId, sources: I) -> Result<(), RenderError>
    where
        I: IntoIterator<
            Item = (
                &'a wgpu::Texture,
                VideoFormat,
                Timestamp,
                FrameTransform,
                &'a [FrameFilter],
            ),
        >,
        I::IntoIter: ExactSizeIterator,
    {
        let sources = sources.into_iter();
        if sources.len() == 0 {
            return Err(RenderError::EmptyComposition);
        }
        let source_count = sources.len();
        let target_texture = self
            .textures
            .get(&target)
            .ok_or(RenderError::UnknownTexture(target))?;
        let scratch_a = self.acquire_texture(target_texture.format);
        let scratch_b = self.acquire_texture(target_texture.format);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("obs-rs-gpu-composite"),
            });
        for (index, (source, source_format, timestamp, transform, filters)) in sources.enumerate() {
            let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
            let background = if index.is_multiple_of(2) {
                if index == 0 {
                    source
                } else {
                    &scratch_b
                }
            } else {
                &scratch_a
            };
            let background_view = background.create_view(&wgpu::TextureViewDescriptor::default());
            let destination = if index.is_multiple_of(2) {
                &scratch_a
            } else {
                &scratch_b
            };
            let destination_view = destination.create_view(&wgpu::TextureViewDescriptor::default());
            let parameters = layer_parameters(
                source_format,
                target_texture.format,
                timestamp,
                transform,
                filters,
            );
            let parameter_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("obs-rs-layer-parameters"),
                        contents: &parameters,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("obs-rs-layer"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&background_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: parameter_buffer.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("obs-rs-layer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &destination_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(if index == 0 {
                &self.replace_pipeline
            } else {
                &self.composite_pipeline
            });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let completed = if source_count.is_multiple_of(2) {
            &scratch_b
        } else {
            &scratch_a
        };
        encoder.copy_texture_to_texture(
            wgpu::ImageCopyTexture {
                texture: completed,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyTexture {
                texture: &target_texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: target_texture.format.width(),
                height: target_texture.format.height(),
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        self.recycle_texture(target_texture.format, scratch_a);
        self.recycle_texture(target_texture.format, scratch_b);
        Ok(())
    }
}

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

const NV12_SHADER: &str = include_str!("shaders/nv12.wgsl");

impl Nv12Converter {
    fn ensure_staging(&mut self, device: &wgpu::Device, size: u64) {
        if self
            .staging
            .as_ref()
            .is_some_and(|staging| staging.size >= size)
        {
            return;
        }
        self.staging = Some(Nv12Staging {
            output: nv12_output_buffer(device, size),
            readback: nv12_readback_buffer(device, size),
            size,
        });
    }
}

fn nv12_output_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("obs-rs-nv12-output-ring"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn nv12_readback_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("obs-rs-nv12-readback-ring"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

fn async_nv12_readback(
    device: &wgpu::Device,
    buffer_size: u64,
    byte_len: usize,
    texture: TextureId,
    format: VideoFormat,
    timestamp: Timestamp,
) -> AsyncNv12Readback {
    AsyncNv12Readback {
        output: nv12_output_buffer(device, buffer_size),
        readback: nv12_readback_buffer(device, buffer_size),
        status: Arc::new(AtomicU8::new(READBACK_IDLE)),
        buffer_size,
        byte_len,
        texture,
        format,
        timestamp,
    }
}

fn nv12_buffer_size(format: VideoFormat) -> Result<(usize, u64), RenderError> {
    let byte_len = PixelFormat::Nv12
        .bytes_for(format)
        .map_err(RenderError::Media)?;
    let word_count = byte_len.div_ceil(4);
    let buffer_size = u64::try_from(word_count)
        .unwrap_or(u64::MAX / 4)
        .saturating_mul(4);
    Ok((byte_len, buffer_size))
}

fn nv12_converter(device: &wgpu::Device) -> Nv12Converter {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("obs-rs-rgba-to-nv12"),
        source: wgpu::ShaderSource::Wgsl(NV12_SHADER.into()),
    });
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("obs-rs-nv12-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("obs-rs-nv12-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("obs-rs-nv12-pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: "main",
        compilation_options: wgpu::PipelineCompilationOptions::default(),
    });
    Nv12Converter {
        _shader: shader,
        bind_group_layout,
        _pipeline_layout: pipeline_layout,
        pipeline,
        staging: None,
        async_staging: Vec::new(),
    }
}

fn request_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), WgpuBackendError> {
    pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("obs-rs-wgpu-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
        },
        None,
    ))
    .map_err(|error| WgpuBackendError(error.to_string()))
}

fn install_device_loss_handler(device: &wgpu::Device) -> Arc<AtomicBool> {
    let lost = Arc::new(AtomicBool::new(false));
    let callback_flag = Arc::clone(&lost);
    device.set_device_lost_callback(move |_reason, _message| {
        callback_flag.store(true, Ordering::Release);
    });
    lost
}

#[allow(
    clippy::too_many_lines,
    reason = "the complete WGSL oracle is kept beside its binding layout"
)]
fn gpu_compositor(
    device: &wgpu::Device,
) -> (
    wgpu::BindGroupLayout,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("obs-rs-composite-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("obs-rs-composite-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/compositor.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("obs-rs-composite-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let create_pipeline = |label, entry_point| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        })
    };
    let replace = create_pipeline("obs-rs-replace-pipeline", "fs_replace");
    let composite = create_pipeline("obs-rs-composite-pipeline", "fs_composite");
    (layout, replace, composite)
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &GpuTexture,
) -> Result<VideoFrame, RenderError> {
    let unpadded_row = texture.format.width() * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = unpadded_row.div_ceil(alignment) * alignment;
    let buffer_size = u64::from(padded_row) * u64::from(texture.format.height());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("obs-rs-readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("obs-rs-readback-copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(texture.format.height()),
            },
        },
        wgpu::Extent3d {
            width: texture.format.width(),
            height: texture.format.height(),
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::Maintain::Wait);
    receiver
        .recv()
        .map_err(|error| RenderError::Backend {
            message: format!("GPU readback callback failed: {error}"),
        })?
        .map_err(|error| RenderError::Backend {
            message: format!("GPU readback mapping failed: {error}"),
        })?;
    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity(texture.format.rgba_bytes());
    for row in mapped.chunks_exact(padded_row as usize) {
        pixels.extend_from_slice(&row[..unpadded_row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    VideoFrame::new(texture.format, texture.timestamp, pixels).map_err(RenderError::Media)
}

fn readback_buffer(device: &wgpu::Device, buffer_size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("obs-rs-readback-ring"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}
