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
    pub(crate) failed_sources: u64,
    pub(crate) contract_violations: u64,
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

    /// Returns the number of source renders that failed and were skipped.
    ///
    /// A failing source is isolated rather than fatal, so this counter is how a
    /// broken camera in an otherwise healthy scene becomes visible.
    #[must_use]
    pub const fn failed_sources(self) -> u64 {
        self.failed_sources
    }

    /// Returns how many source failures were contract violations.
    ///
    /// A subset of [`Self::failed_sources`]. A device that is unavailable is an
    /// ordinary fact of live capture; a source that rejects the format it was
    /// configured for, or its own settings, is a bug in the engine or in that
    /// source. Both are isolated so the scene keeps rendering, and this counter
    /// is what keeps the second kind from hiding among the first.
    #[must_use]
    pub const fn contract_violations(self) -> u64 {
        self.contract_violations
    }

    /// Distribution of source capture/render call latency.
    #[must_use]
    pub const fn capture_latency(self) -> LatencyMetrics {
        self.capture_latency
    }
}
