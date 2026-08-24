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

#[path = "gpu_backend.rs"]
mod backend;
#[path = "gpu_compositor.rs"]
mod compositor;
#[path = "gpu_helpers.rs"]
mod helpers;
#[path = "gpu_nv12.rs"]
mod nv12;
#[path = "gpu_render_backend.rs"]
mod render_backend;
