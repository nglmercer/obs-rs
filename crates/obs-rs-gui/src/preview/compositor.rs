//! GPU and CPU preview composition target ownership.

use std::collections::HashMap;
use std::error::Error;

use obs_rs_media::VideoFormat;
use obs_rs_render::{RenderBackend, RenderTarget, RenderTargetRole, TextureId};
use obs_rs_render_wgpu::WgpuRenderBackend;

struct GpuTarget {
    target: RenderTarget,
    texture: TextureId,
}

pub(super) struct WgpuCompositor {
    pub(super) backend: Box<WgpuRenderBackend>,
    targets: HashMap<RenderTargetRole, GpuTarget>,
}

impl WgpuCompositor {
    pub(super) fn target(&mut self, target: RenderTarget) -> Result<TextureId, Box<dyn Error>> {
        if let Some(existing) = self.targets.get(&target.role()) {
            if existing.target.format() == target.format() {
                return Ok(existing.texture);
            }
        }
        if let Some(previous) = self.targets.remove(&target.role()) {
            self.backend.destroy_texture(previous.texture)?;
        }
        let texture = self.backend.create_texture(target.format())?;
        self.targets
            .insert(target.role(), GpuTarget { target, texture });
        Ok(texture)
    }
}

pub(super) enum PreviewCompositor {
    Wgpu(WgpuCompositor),
    Cpu { reason: Option<String> },
}

impl PreviewCompositor {
    pub(super) fn new(format: VideoFormat) -> Self {
        let texture_budget = format.rgba_bytes().saturating_mul(12);
        match WgpuRenderBackend::new(12, texture_budget) {
            Ok(backend) => Self::Wgpu(WgpuCompositor {
                backend: Box::new(backend),
                targets: HashMap::new(),
            }),
            Err(error) => Self::Cpu {
                reason: Some(error.to_string()),
            },
        }
    }
}
