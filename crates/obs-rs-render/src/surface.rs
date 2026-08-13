use obs_rs_media::{PixelFormat, Timestamp, VideoFormat, VideoFrame};

use crate::TextureId;

/// One plane of a backend-owned GPU frame.
///
/// `texture` is a process-local render-backend token, not an operating-system
/// texture or memory handle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GpuPlaneHandle {
    texture: TextureId,
    width: u32,
    height: u32,
}

impl GpuPlaneHandle {
    /// Creates a portable plane descriptor for a backend-owned texture.
    #[must_use]
    pub const fn new(texture: TextureId, width: u32, height: u32) -> Self {
        Self {
            texture,
            width,
            height,
        }
    }

    /// Returns the backend-local texture token.
    #[must_use]
    pub const fn texture(self) -> TextureId {
        self.texture
    }

    /// Returns the plane width in samples.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Returns the plane height in samples.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

/// Portable reference to backend-owned GPU video planes.
///
/// The handle deliberately carries no Vulkan, Metal, D3D, DMA-BUF, or other
/// platform handle. An adapter that created the tokens is solely responsible
/// for resolving them and for keeping the resources alive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuFrameHandle {
    provider: String,
    format: VideoFormat,
    pixel_format: PixelFormat,
    timestamp: Timestamp,
    planes: Vec<GpuPlaneHandle>,
}

impl GpuFrameHandle {
    /// Creates a GPU-frame descriptor after validating its provider and planes.
    #[must_use]
    pub fn new(
        provider: &str,
        format: VideoFormat,
        pixel_format: PixelFormat,
        timestamp: Timestamp,
        planes: Vec<GpuPlaneHandle>,
    ) -> Option<Self> {
        if provider.trim().is_empty()
            || planes.is_empty()
            || planes
                .iter()
                .any(|plane| plane.width == 0 || plane.height == 0)
        {
            return None;
        }
        Some(Self {
            provider: provider.to_owned(),
            format,
            pixel_format,
            timestamp,
            planes,
        })
    }

    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    #[must_use]
    pub const fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    #[must_use]
    pub fn planes(&self) -> &[GpuPlaneHandle] {
        &self.planes
    }
}

/// A video frame that may remain on its producer's GPU or use the CPU fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VideoSurface {
    Cpu(VideoFrame),
    Gpu(GpuFrameHandle),
}

impl VideoSurface {
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        match self {
            Self::Cpu(frame) => frame.format(),
            Self::Gpu(frame) => frame.format(),
        }
    }

    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        match self {
            Self::Cpu(frame) => frame.timestamp(),
            Self::Gpu(frame) => frame.timestamp(),
        }
    }
}
