use obs_rs_media::LatencyMetrics;

/// Counters for CPU compositor work and source behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompositorMetrics {
    pub(crate) render_calls: u64,
    pub(crate) source_requests: u64,
    pub(crate) source_frames: u64,
    pub(crate) empty_sources: u64,
    pub(crate) transformed_frames: u64,
    pub(crate) filtered_frames: u64,
    pub(crate) blended_layers: u64,
    pub(crate) capture_latency: LatencyMetrics,
}

impl CompositorMetrics {
    /// Returns the number of scene render calls.
    #[must_use]
    pub const fn render_calls(self) -> u64 {
        self.render_calls
    }

    /// Returns the number of source render requests.
    #[must_use]
    pub const fn source_requests(self) -> u64 {
        self.source_requests
    }

    /// Returns the number of source frames returned.
    #[must_use]
    pub const fn source_frames(self) -> u64 {
        self.source_frames
    }

    /// Returns the number of source requests that returned no frame.
    #[must_use]
    pub const fn empty_sources(self) -> u64 {
        self.empty_sources
    }

    /// Returns the number of frames sent through a non-identity transform.
    #[must_use]
    pub const fn transformed_frames(self) -> u64 {
        self.transformed_frames
    }

    /// Returns the number of in-place filter applications.
    #[must_use]
    pub const fn filtered_frames(self) -> u64 {
        self.filtered_frames
    }

    /// Returns the number of layer-over-layer blend operations.
    #[must_use]
    pub const fn blended_layers(self) -> u64 {
        self.blended_layers
    }

    /// Distribution of source capture/render call latency.
    #[must_use]
    pub const fn capture_latency(self) -> LatencyMetrics {
        self.capture_latency
    }
}
