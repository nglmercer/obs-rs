use super::{
    Arc, AsyncNv12Readback, AtomicU8, Nv12Converter, Nv12Staging, PixelFormat, RenderError,
    TextureId, Timestamp, VideoFormat, READBACK_IDLE,
};

const NV12_SHADER: &str = include_str!("shaders/nv12.wgsl");

impl Nv12Converter {
    pub(super) fn ensure_staging(&mut self, device: &wgpu::Device, size: u64) {
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

pub(super) fn async_nv12_readback(
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

pub(super) fn nv12_buffer_size(format: VideoFormat) -> Result<(usize, u64), RenderError> {
    let byte_len = PixelFormat::Nv12
        .bytes_for(format)
        .map_err(RenderError::Media)?;
    let word_count = byte_len.div_ceil(4);
    let buffer_size = u64::try_from(word_count)
        .unwrap_or(u64::MAX / 4)
        .saturating_mul(4);
    Ok((byte_len, buffer_size))
}

pub(super) fn nv12_converter(device: &wgpu::Device) -> Nv12Converter {
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
