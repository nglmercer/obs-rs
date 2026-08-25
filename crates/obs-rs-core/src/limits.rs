/// Bounded runtime resources used to contain faulty or untrusted extensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeLimits {
    plugins: usize,
    source_kinds: usize,
    scenes: usize,
    sources: usize,
    sources_per_scene: usize,
    filters_per_source: usize,
}

impl RuntimeLimits {
    /// Creates explicit limits for runtime-owned resources.
    #[must_use]
    pub const fn new(
        max_plugins: usize,
        max_source_kinds: usize,
        max_scenes: usize,
        max_sources: usize,
        max_sources_per_scene: usize,
        max_filters_per_source: usize,
    ) -> Self {
        Self {
            plugins: max_plugins,
            source_kinds: max_source_kinds,
            scenes: max_scenes,
            sources: max_sources,
            sources_per_scene: max_sources_per_scene,
            filters_per_source: max_filters_per_source,
        }
    }

    /// Returns the maximum registered plugin count.
    #[must_use]
    pub const fn max_plugins(self) -> usize {
        self.plugins
    }

    /// Returns the maximum registered source-kind count.
    #[must_use]
    pub const fn max_source_kinds(self) -> usize {
        self.source_kinds
    }

    /// Returns the maximum plugin-dock count derived from the plugin quota.
    ///
    /// Keeping this derived preserves the six-argument constructor while still
    /// bounding extension metadata independently from scene and source state.
    #[must_use]
    pub const fn max_docks(self) -> usize {
        self.plugins
            .saturating_mul(obs_rs_plugin_api::MAX_PLUGIN_DOCKS)
    }

    /// Returns the maximum scene count.
    #[must_use]
    pub const fn max_scenes(self) -> usize {
        self.scenes
    }

    /// Returns the maximum source-instance count.
    #[must_use]
    pub const fn max_sources(self) -> usize {
        self.sources
    }

    /// Returns the maximum scene items in one scene.
    #[must_use]
    pub const fn max_sources_per_scene(self) -> usize {
        self.sources_per_scene
    }

    /// Returns the maximum filters on one shared source definition.
    #[must_use]
    pub const fn max_filters_per_source(self) -> usize {
        self.filters_per_source
    }

    /// Returns the maximum filters on one source.
    ///
    /// Kept as a source-compatible alias for callers compiled against the
    /// pre-registry runtime API.
    #[must_use]
    #[deprecated(note = "use max_filters_per_source")]
    pub const fn max_filters_per_item(self) -> usize {
        self.max_filters_per_source()
    }
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self::new(64, 256, 1_024, 4_096, 1_024, 64)
    }
}

/// Current runtime-owned resource usage for diagnostics and quota dashboards.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeUsage {
    pub(crate) plugins: usize,
    pub(crate) source_kinds: usize,
    pub(crate) docks: usize,
    pub(crate) scenes: usize,
    pub(crate) sources: usize,
    pub(crate) filters: usize,
}

impl RuntimeUsage {
    /// Returns the number of registered plugins.
    #[must_use]
    pub const fn plugins(self) -> usize {
        self.plugins
    }

    /// Returns the number of registered source kinds.
    #[must_use]
    pub const fn source_kinds(self) -> usize {
        self.source_kinds
    }

    /// Returns the number of registered plugin dock descriptors.
    #[must_use]
    pub const fn docks(self) -> usize {
        self.docks
    }

    /// Returns the number of named scenes.
    #[must_use]
    pub const fn scenes(self) -> usize {
        self.scenes
    }

    /// Returns the number of runtime-owned source instances.
    #[must_use]
    pub const fn sources(self) -> usize {
        self.sources
    }

    /// Returns the total number of filters across registered source definitions.
    #[must_use]
    pub const fn filters(self) -> usize {
        self.filters
    }
}
