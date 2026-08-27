use super::{Arc, AtomicBool, GpuTexture, Ordering, RenderError, VideoFrame, WgpuBackendError};

pub(super) fn request_device(
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

pub(super) fn install_device_loss_handler(device: &wgpu::Device) -> Arc<AtomicBool> {
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
pub(super) fn gpu_compositor(
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

pub(super) fn read_texture(
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

pub(super) fn readback_buffer(device: &wgpu::Device, buffer_size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("obs-rs-readback-ring"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}
