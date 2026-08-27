//! GPU and CPU preview composition target ownership.

use std::collections::HashMap;
use std::error::Error;

use obs_rs_media::{RawVideoFrame, VideoFormat, VideoFrame};
use obs_rs_render::{RenderBackend, RenderTarget, RenderTargetRole, TextureId};
use obs_rs_render_wgpu::WgpuRenderBackend;

struct GpuTarget {
    target: RenderTarget,
    texture: TextureId,
}

// Keep one completed compatibility frame for each normal desktop consumer
// while retaining a hard bound for projector-heavy sessions.
const COMPLETED_READBACK_CAPACITY: usize = 8;
const COMPLETED_NV12_CAPACITY: usize = 2;

pub(super) struct WgpuCompositor {
    pub(super) backend: Box<WgpuRenderBackend>,
    targets: HashMap<RenderTargetRole, GpuTarget>,
    completed_readbacks: HashMap<TextureId, VideoFrame>,
    completed_nv12: HashMap<TextureId, RawVideoFrame>,
}

impl WgpuCompositor {
    pub(super) fn target(&mut self, target: RenderTarget) -> Result<TextureId, Box<dyn Error>> {
        if let Some(existing) = self.targets.get(&target.role()) {
            if existing.target.format() == target.format() {
                return Ok(existing.texture);
            }
        }
        if let Some(previous) = self.targets.remove(&target.role()) {
            self.completed_readbacks.remove(&previous.texture);
            self.completed_nv12.remove(&previous.texture);
            self.backend.destroy_texture(previous.texture)?;
        }
        let texture = self.backend.create_texture(target.format())?;
        self.targets
            .insert(target.role(), GpuTarget { target, texture });
        Ok(texture)
    }

    /// Presents the newest completed compatibility readback, then queues one
    /// replacement if this target does not already have a staging request.
    ///
    /// Polling is deliberately nonblocking. A caller that receives `None`
    /// keeps its previous GUI image instead of waiting for the GPU.
    pub(super) fn readback_async(
        &mut self,
        texture: TextureId,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.poll_async_readbacks()?;
        let frame = self.completed_readbacks.remove(&texture);
        if !self.backend.readback_pending(texture) {
            let _ = self.backend.submit_readback(texture)?;
        }
        Ok(frame)
    }

    pub(super) fn poll_async_readbacks(&mut self) -> Result<(), Box<dyn Error>> {
        for (completed_texture, frame) in self.backend.poll_readbacks()? {
            if self.completed_readbacks.len() >= COMPLETED_READBACK_CAPACITY
                && !self.completed_readbacks.contains_key(&completed_texture)
            {
                if let Some(oldest) = self.completed_readbacks.keys().next().copied() {
                    self.completed_readbacks.remove(&oldest);
                }
            }
            self.completed_readbacks.insert(completed_texture, frame);
        }
        Ok(())
    }

    pub(super) fn take_async_readback(&mut self, texture: TextureId) -> Option<VideoFrame> {
        self.completed_readbacks.remove(&texture)
    }

    /// Polls the bounded encoder conversion bridge and presents its newest
    /// completed frame without waiting for the GPU.
    pub(super) fn readback_nv12_async(
        &mut self,
        texture: TextureId,
    ) -> Result<Option<RawVideoFrame>, Box<dyn Error>> {
        for (completed_texture, frame) in self.backend.poll_nv12_readbacks()? {
            if self.completed_nv12.len() >= COMPLETED_NV12_CAPACITY
                && !self.completed_nv12.contains_key(&completed_texture)
            {
                if let Some(oldest) = self.completed_nv12.keys().next().copied() {
                    self.completed_nv12.remove(&oldest);
                }
            }
            self.completed_nv12.insert(completed_texture, frame);
        }
        let frame = self.completed_nv12.remove(&texture);
        if !self.backend.nv12_readback_pending(texture) {
            let _ = self.backend.submit_nv12_readback(texture)?;
        }
        Ok(frame)
    }

    pub(super) fn existing_target(&self, target: RenderTarget) -> Option<TextureId> {
        self.targets
            .get(&target.role())
            .filter(|existing| existing.target.format() == target.format())
            .map(|existing| existing.texture)
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
                completed_readbacks: HashMap::new(),
                completed_nv12: HashMap::new(),
            }),
            Err(error) => Self::Cpu {
                reason: Some(error.to_string()),
            },
        }
    }

    pub(super) fn deferred_readback(&self) -> bool {
        matches!(self, Self::Wgpu(_))
    }

    pub(super) fn diagnostics(&self) -> (String, String) {
        match self {
            Self::Wgpu(compositor) => (
                compositor.backend.adapter_capabilities().name().to_owned(),
                compositor.backend.adapter_capabilities().backend().to_owned(),
            ),
            Self::Cpu { reason } => (
                "CPU fallback".to_owned(),
                reason
                    .clone()
                    .unwrap_or_else(|| "CPU compositor".to_owned()),
            ),
        }
    }
}
