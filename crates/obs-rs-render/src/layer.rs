use obs_rs_media::{FrameFilter, FrameTransform, Timestamp, VideoFormat, VideoFrame};

/// Opaque reference to a native capture surface retained by its producer.
///
/// The numeric token is meaningful only to the named producer/backend bridge;
/// no OS handle is exposed to the portable engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OpaqueFrameSurface {
    provider: String,
    token: u64,
    format: VideoFormat,
    timestamp: Timestamp,
}

impl OpaqueFrameSurface {
    /// Creates an opaque surface token.
    ///
    /// # Errors
    ///
    /// Returns `None` for an empty provider name or zero token.
    #[must_use]
    pub fn new(
        provider: &str,
        token: u64,
        format: VideoFormat,
        timestamp: Timestamp,
    ) -> Option<Self> {
        if provider.trim().is_empty() || token == 0 {
            return None;
        }
        Some(Self {
            provider: provider.to_owned(),
            token,
            format,
            timestamp,
        })
    }

    /// Returns the producer/backend bridge name, never an OS handle.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Returns the producer-local opaque token.
    #[must_use]
    pub const fn token(&self) -> u64 {
        self.token
    }

    /// Returns the surface video format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the surface presentation timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

/// Frame input carried by one scene-layer submission.
#[derive(Clone, Copy, Debug)]
pub enum LayerInput<'a> {
    /// Portable RGBA frame, uploaded or processed by the selected backend.
    Frame(&'a VideoFrame),
    /// Opaque native surface eligible for zero-copy import.
    Surface(&'a OpaqueFrameSurface),
}

impl LayerInput<'_> {
    /// Returns the input format without exposing native details.
    #[must_use]
    pub const fn format(self) -> VideoFormat {
        match self {
            Self::Frame(frame) => frame.format(),
            Self::Surface(surface) => surface.format(),
        }
    }
}

/// One ordered scene layer with reusable transform/filter metadata.
#[derive(Clone, Copy, Debug)]
pub struct SceneLayer<'a> {
    input: LayerInput<'a>,
    transform: FrameTransform,
    filters: &'a [FrameFilter],
}

impl<'a> SceneLayer<'a> {
    /// Creates a CPU-frame layer.
    #[must_use]
    pub const fn frame(
        frame: &'a VideoFrame,
        transform: FrameTransform,
        filters: &'a [FrameFilter],
    ) -> Self {
        Self {
            input: LayerInput::Frame(frame),
            transform,
            filters,
        }
    }

    /// Creates an opaque native-surface layer.
    #[must_use]
    pub const fn surface(
        surface: &'a OpaqueFrameSurface,
        transform: FrameTransform,
        filters: &'a [FrameFilter],
    ) -> Self {
        Self {
            input: LayerInput::Surface(surface),
            transform,
            filters,
        }
    }

    /// Returns the layer input.
    #[must_use]
    pub const fn input(self) -> LayerInput<'a> {
        self.input
    }

    /// Returns transform metadata.
    #[must_use]
    pub const fn transform(self) -> FrameTransform {
        self.transform
    }

    /// Returns the ordered filter chain.
    #[must_use]
    pub const fn filters(self) -> &'a [FrameFilter] {
        self.filters
    }
}

/// How a backend will consume one native surface provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceImportMode {
    /// Native surface remains on the accelerated path without CPU readback.
    Direct,
    /// Producer must submit an RGBA frame through the CPU upload path.
    CpuFallback,
    /// Neither direct import nor a compatible fallback is available.
    Unsupported,
}
