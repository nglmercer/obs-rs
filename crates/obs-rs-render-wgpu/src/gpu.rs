use std::{collections::HashMap, fmt};

use obs_rs_media::{FrameFilter, FrameTransform, Timestamp, VideoFormat, VideoFrame};
use obs_rs_render::{
    CpuRenderBackend, OpaqueFrameSurface, RenderBackend, RenderCapabilities, RenderError,
    RenderMetrics, RenderState, SceneLayer, SurfaceImportMode, TextureId,
};
use wgpu::util::DeviceExt;

struct GpuTexture {
    format: VideoFormat,
    texture: wgpu::Texture,
    uploaded: bool,
    timestamp: Timestamp,
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
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    replace_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    metrics: RenderMetrics,
    capabilities: RenderCapabilities,
    adapter_capabilities: WgpuAdapterCapabilities,
    state: RenderState,
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
        let (bind_group_layout, sampler, replace_pipeline, composite_pipeline) =
            gpu_compositor(&device);
        let info = adapter.get_info();
        let cpu = CpuRenderBackend::with_limits(max_textures, max_texture_bytes)
            .map_err(|error| WgpuBackendError(error.to_string()))?;
        Ok(Self {
            adapter,
            device,
            queue,
            cpu,
            textures: HashMap::new(),
            bind_group_layout,
            sampler,
            replace_pipeline,
            composite_pipeline,
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
        })
    }

    #[must_use]
    pub const fn adapter_capabilities(&self) -> &WgpuAdapterCapabilities {
        &self.adapter_capabilities
    }

    /// Marks resources lost for deterministic recovery testing.
    pub fn lose_device(&mut self) {
        self.state = RenderState::Lost;
        self.cpu.lose_context();
        self.metrics.record_context_loss();
    }

    fn ensure_ready(&self) -> Result<(), RenderError> {
        if self.state == RenderState::Ready {
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
    fn composite_textures(
        &self,
        target: TextureId,
        sources: &[(&wgpu::Texture, FrameTransform, &[FrameFilter])],
    ) -> Result<(), RenderError> {
        if sources.is_empty() {
            return Err(RenderError::EmptyComposition);
        }
        let target_texture = self
            .textures
            .get(&target)
            .ok_or(RenderError::UnknownTexture(target))?;
        let target_view = target_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("obs-rs-gpu-composite"),
            });
        for (index, (source, transform, filters)) in sources.iter().enumerate() {
            let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
            let parameters = layer_parameters(target_texture.format, *transform, filters);
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
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
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
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if index == 0 {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
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
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }
}

impl RenderBackend for WgpuRenderBackend {
    fn capabilities(&self) -> RenderCapabilities {
        self.capabilities
    }

    fn state(&self) -> RenderState {
        self.state
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
        let target_format = self
            .textures
            .get(&target)
            .ok_or(RenderError::UnknownTexture(target))?
            .format;
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
            if frame.format() != target_format {
                return Err(RenderError::FormatMismatch {
                    expected: target_format,
                    actual: frame.format(),
                });
            }
            timestamp = frame.timestamp();
            let texture = Self::gpu_texture(target_format, &self.device);
            write_texture(&self.queue, &texture, frame);
            prepared.push((texture, layer.transform(), layer.filters()));
        }
        let sources = prepared
            .iter()
            .map(|(texture, transform, filters)| (texture, *transform, *filters))
            .collect::<Vec<_>>();
        self.composite_textures(target, &sources)?;
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
        self.metrics.record_upload();
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
        let sources = layers
            .iter()
            .map(|layer| {
                (
                    &self.textures.get(layer).expect("validated layer").texture,
                    FrameTransform::IDENTITY,
                    &[] as &[FrameFilter],
                )
            })
            .collect::<Vec<_>>();
        self.composite_textures(target, &sources)?;
        drop(sources);
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
        self.metrics.record_readback();
        Ok(frame)
    }

    fn recover(&mut self) -> Result<(), RenderError> {
        if self.state == RenderState::Ready {
            return Ok(());
        }
        let (device, queue) =
            request_device(&self.adapter).map_err(|error| RenderError::Backend {
                message: error.to_string(),
            })?;
        self.device = device;
        self.queue = queue;
        (
            self.bind_group_layout,
            self.sampler,
            self.replace_pipeline,
            self.composite_pipeline,
        ) = gpu_compositor(&self.device);
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

#[allow(clippy::too_many_lines, reason = "the complete WGSL oracle is kept beside its binding layout")]
fn gpu_compositor(
    device: &wgpu::Device,
) -> (
    wgpu::BindGroupLayout,
    wgpu::Sampler,
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
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("obs-rs-nearest-sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..wgpu::SamplerDescriptor::default()
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("obs-rs-composite-shader"),
        source: wgpu::ShaderSource::Wgsl(
            r"
@group(0) @binding(0) var layer_texture: texture_2d<f32>;
@group(0) @binding(1) var layer_sampler: sampler;
struct Parameters { values: array<i32> };
@group(0) @binding(2) var<storage, read> parameters: Parameters;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    var output: VertexOutput;
    let position = positions[vertex];
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = vec2<f32>((position.x + 1.0) * 0.5, (1.0 - position.y) * 0.5);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let x = i32(input.position.x);
    let y = i32(input.position.y);
    let width = parameters.values[0];
    let height = parameters.values[1];
    let local_x = x - parameters.values[4];
    let local_y = y - parameters.values[5];
    if (local_x < 0 || local_y < 0) {
        return vec4<f32>(0.0);
    }
    let crop_left = parameters.values[9];
    let crop_top = parameters.values[10];
    let visible_right = width - parameters.values[11];
    let visible_bottom = height - parameters.values[12];
    var source_x = crop_left + local_x * 1000 / parameters.values[2];
    var source_y = crop_top + local_y * 1000 / parameters.values[3];
    if (source_x < crop_left || source_x >= visible_right ||
        source_y < crop_top || source_y >= visible_bottom) {
        return vec4<f32>(0.0);
    }
    if (parameters.values[6] != 0) {
        source_x = crop_left + visible_right - 1 - source_x;
    }
    if (parameters.values[7] != 0) {
        source_y = crop_top + visible_bottom - 1 - source_y;
    }
    let sampled = textureLoad(layer_texture, vec2<i32>(source_x, source_y), 0);
    var pixel = vec4<i32>(floor(sampled * 255.0 + vec4<f32>(0.5)));
    pixel.a = pixel.a * parameters.values[8] / 255;
    let filter_count = parameters.values[13];
    var filter_index = 0;
    loop {
        if (filter_index >= filter_count) { break; }
        let kind = parameters.values[14 + filter_index * 2];
        let value = parameters.values[15 + filter_index * 2];
        if (kind == 0) {
            let luma = (pixel.r * 77 + pixel.g * 150 + pixel.b * 29) / 256;
            pixel.r = luma;
            pixel.g = luma;
            pixel.b = luma;
        } else if (kind == 1) {
            let multiplier = value + 1000;
            pixel.r = clamp(pixel.r * multiplier / 1000, 0, 255);
            pixel.g = clamp(pixel.g * multiplier / 1000, 0, 255);
            pixel.b = clamp(pixel.b * multiplier / 1000, 0, 255);
        } else {
            pixel.a = pixel.a * value / 255;
        }
        filter_index = filter_index + 1;
    }
    return vec4<f32>(pixel) / 255.0;
}
"
            .into(),
        ),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("obs-rs-composite-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let create_pipeline = |label, blend| {
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
                entry_point: "fs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        })
    };
    let replace = create_pipeline("obs-rs-replace-pipeline", None);
    let composite = create_pipeline(
        "obs-rs-composite-pipeline",
        Some(wgpu::BlendState::ALPHA_BLENDING),
    );
    (layout, sampler, replace, composite)
}

fn layer_parameters(
    format: VideoFormat,
    transform: FrameTransform,
    filters: &[FrameFilter],
) -> Vec<u8> {
    let mut values = Vec::with_capacity(14 + filters.len() * 2);
    values.extend([
        i32::try_from(format.width()).unwrap_or(i32::MAX),
        i32::try_from(format.height()).unwrap_or(i32::MAX),
        i32::try_from(transform.scale_x_milli()).unwrap_or(i32::MAX),
        i32::try_from(transform.scale_y_milli()).unwrap_or(i32::MAX),
        transform.translate_x(),
        transform.translate_y(),
        i32::from(transform.flip_x()),
        i32::from(transform.flip_y()),
        i32::from(transform.opacity()),
        i32::try_from(transform.crop_left()).unwrap_or(i32::MAX),
        i32::try_from(transform.crop_top()).unwrap_or(i32::MAX),
        i32::try_from(transform.crop_right()).unwrap_or(i32::MAX),
        i32::try_from(transform.crop_bottom()).unwrap_or(i32::MAX),
        i32::try_from(filters.len()).unwrap_or(i32::MAX),
    ]);
    for filter in filters {
        match *filter {
            FrameFilter::Grayscale => values.extend([0, 0]),
            FrameFilter::Brightness { milli } => values.extend([1, i32::from(milli)]),
            FrameFilter::Opacity(opacity) => values.extend([2, i32::from(opacity)]),
        }
    }
    values.into_iter().flat_map(i32::to_le_bytes).collect()
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
