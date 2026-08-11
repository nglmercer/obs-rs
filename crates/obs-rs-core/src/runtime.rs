use std::{collections::HashMap, sync::Arc};

use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform};
use obs_rs_plugin_api::{Plugin, PluginApiVersion, PluginManifest};
use obs_rs_util::Identifier;

use super::{
    error::{identifier, RuntimeError},
    ids::SourceId,
    limits::{RuntimeLimits, RuntimeUsage},
    metrics::CompositorMetrics,
    registry::{Registry, Scene, SourceInstance},
};

pub struct Runtime {
    pub(crate) registry: Registry,
    /// Keyed lookup only; never iterated in key order, so hashing is safe here
    /// and keeps `sources.get_mut` off the compositor's O(log n) path.
    pub(crate) sources: HashMap<SourceId, SourceInstance>,
    /// Keyed lookup only. Deterministic rendering comes from each scene's
    /// ordered source vector, not from scene-map iteration order.
    pub(crate) scenes: HashMap<Identifier, Scene>,
    /// How many scenes currently reference each source.
    ///
    /// Lets `destroy_source` answer "is this source still in use?" in constant
    /// time instead of scanning every scene's item list.
    pub(crate) scene_references: HashMap<SourceId, usize>,
    /// Running total of filters across every scene item.
    pub(crate) filter_count: usize,
    pub(crate) next_source_id: u64,
    pub(crate) metrics: CompositorMetrics,
    pub(crate) limits: RuntimeLimits,
}

impl Runtime {
    /// Creates an empty runtime with no global state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            sources: HashMap::new(),
            scenes: HashMap::new(),
            scene_references: HashMap::new(),
            filter_count: 0,
            next_source_id: 1,
            metrics: CompositorMetrics::default(),
            limits: RuntimeLimits::default(),
        }
    }

    /// Creates an empty runtime with explicit resource limits.
    #[must_use]
    pub fn with_limits(limits: RuntimeLimits) -> Self {
        Self {
            registry: Registry::new(),
            sources: HashMap::new(),
            scenes: HashMap::new(),
            scene_references: HashMap::new(),
            filter_count: 0,
            next_source_id: 1,
            metrics: CompositorMetrics::default(),
            limits,
        }
    }

    /// Returns the active resource limits.
    #[must_use]
    pub const fn limits(&self) -> RuntimeLimits {
        self.limits
    }

    /// Returns a bounded resource-usage snapshot for diagnostics and UI status.
    #[must_use]
    pub fn usage(&self) -> RuntimeUsage {
        // Maintained incrementally by the filter mutators, so a usage snapshot
        // is O(1) rather than a walk of every scene item.
        let filters = self.filter_count;
        RuntimeUsage {
            plugins: self.registry.plugins.len(),
            source_kinds: self.registry.sources.len(),
            scenes: self.scenes.len(),
            sources: self.sources.len(),
            filters,
        }
    }

    /// Registers a plugin and all of its source factories atomically.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::DuplicatePlugin`] or
    /// [`RuntimeError::DuplicateSourceKind`] when registration would collide with
    /// existing runtime state.
    pub fn register_plugin(&mut self, plugin: &dyn Plugin) -> Result<(), RuntimeError> {
        // Validate against the borrowed manifest; it is only cloned once every
        // registration check has passed.
        let manifest = plugin.manifest();
        if self.registry.plugins.len() >= self.limits.max_plugins() {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "plugins",
                limit: self.limits.max_plugins(),
            });
        }
        let expected_api = PluginApiVersion::current();
        if manifest.api_version().major() != expected_api.major()
            || manifest.api_version().minor() > expected_api.minor()
        {
            return Err(RuntimeError::UnsupportedPluginApi {
                expected: expected_api,
                actual: manifest.api_version(),
            });
        }
        if self.registry.plugins.contains_key(manifest.id()) {
            return Err(RuntimeError::DuplicatePlugin(manifest.id().clone()));
        }

        let factories = plugin.source_factories();
        let source_kind_count = self
            .registry
            .sources
            .len()
            .checked_add(factories.len())
            .ok_or(RuntimeError::ResourceLimitExceeded {
                resource: "source kinds",
                limit: self.limits.max_source_kinds(),
            })?;
        if source_kind_count > self.limits.max_source_kinds() {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "source kinds",
                limit: self.limits.max_source_kinds(),
            });
        }
        for factory in factories {
            if self.registry.sources.contains_key(factory.kind()) {
                return Err(RuntimeError::DuplicateSourceKind(factory.kind().clone()));
            }
        }

        self.registry
            .plugins
            .insert(manifest.id().clone(), manifest.clone());
        for factory in factories {
            self.registry
                .sources
                .insert(factory.kind().clone(), Arc::clone(factory));
        }
        Ok(())
    }

    /// Returns the manifests of registered plugins in identifier order.
    ///
    /// Borrows the stored manifests; callers needing owned values can
    /// `.cloned().collect()`.
    #[must_use]
    pub fn plugins(&self) -> impl ExactSizeIterator<Item = &PluginManifest> {
        self.registry.plugins.values()
    }

    /// Returns the source kinds contributed by registered plugins, in
    /// identifier order.
    ///
    /// Borrows the stored identifiers rather than cloning each one.
    #[must_use]
    pub fn source_kinds(&self) -> impl ExactSizeIterator<Item = &Identifier> {
        self.registry.sources.keys()
    }

    /// Creates a named scene.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidIdentifier`] for an invalid name or
    /// [`RuntimeError::DuplicateScene`] when the name is already in use.
    pub fn create_scene(&mut self, name: &str) -> Result<(), RuntimeError> {
        if self.scenes.len() >= self.limits.max_scenes() {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "scenes",
                limit: self.limits.max_scenes(),
            });
        }
        let name = identifier(name, "scene")?;
        if self.scenes.contains_key(&name) {
            return Err(RuntimeError::DuplicateScene(name));
        }

        self.scenes.insert(name, Scene::new());
        Ok(())
    }

    /// Creates a source through a registered factory.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the kind or name is invalid, no factory exists,
    /// source settings are rejected, or the source ID space is exhausted.
    pub fn create_source(
        &mut self,
        kind: &str,
        name: &str,
        settings: &Config,
    ) -> Result<SourceId, RuntimeError> {
        if self.sources.len() >= self.limits.max_sources() {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "sources",
                limit: self.limits.max_sources(),
            });
        }
        if name.trim().is_empty() {
            return Err(RuntimeError::InvalidName { kind: "source" });
        }

        let kind = identifier(kind, "source kind")?;
        let factory = self
            .registry
            .sources
            .get(&kind)
            .ok_or_else(|| RuntimeError::UnknownSourceKind(kind.clone()))?;
        let source = factory
            .create(name, settings)
            .map_err(RuntimeError::Source)?;
        let id = SourceId(self.next_source_id);
        self.next_source_id = self
            .next_source_id
            .checked_add(1)
            .ok_or(RuntimeError::IdExhausted)?;

        self.sources.insert(
            id,
            SourceInstance {
                kind,
                name: name.to_owned(),
                source,
            },
        );
        Ok(id)
    }

    /// Attaches a source to the end of a scene's ordered item list.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the scene or source does not exist, or when the
    /// source is already attached to that scene.
    pub fn attach_source(&mut self, scene: &str, source: SourceId) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        if !self.sources.contains_key(&source) {
            return Err(RuntimeError::UnknownSource(source));
        }
        let limit = self.limits.max_sources_per_scene();
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        if scene.sources.len() >= limit {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "sources per scene",
                limit,
            });
        }
        if !scene.attach(source) {
            return Err(RuntimeError::SourceAlreadyAttached(source));
        }
        *self.scene_references.entry(source).or_insert(0) += 1;
        Ok(())
    }

    /// Removes a source from a scene while keeping the source instance alive.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the item is absent.
    pub fn detach_source(&mut self, scene: &str, source: SourceId) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        let Some(removed_filters) = scene.detach(source) else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        self.filter_count = self.filter_count.saturating_sub(removed_filters);
        release_scene_reference(&mut self.scene_references, source);
        Ok(())
    }

    /// Sets the transform and opacity for one scene item.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn set_source_transform(
        &mut self,
        scene: &str,
        source: SourceId,
        transform: FrameTransform,
    ) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        let Some(item) = scene.items.get_mut(&source) else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        item.transform = transform;
        Ok(())
    }

    /// Returns a scene item's current transform.
    #[must_use]
    pub fn source_transform(&self, scene: &str, source: SourceId) -> Option<FrameTransform> {
        Some(self.scenes.get(scene)?.items.get(&source)?.transform)
    }

    /// Adds a CPU filter to the end of one scene item's filter chain.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn add_source_filter(
        &mut self,
        scene: &str,
        source: SourceId,
        filter: FrameFilter,
    ) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let limit = self.limits.max_filters_per_item();
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        let Some(item) = scene.items.get_mut(&source) else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        if item.filters.len() >= limit {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "filters per scene item",
                limit,
            });
        }
        item.filters.push(filter);
        self.filter_count = self.filter_count.saturating_add(1);
        Ok(())
    }

    /// Removes every CPU filter from one scene item.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn clear_source_filters(
        &mut self,
        scene: &str,
        source: SourceId,
    ) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        let Some(item) = scene.items.get_mut(&source) else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        self.filter_count = self.filter_count.saturating_sub(item.filters.len());
        item.filters.clear();
        Ok(())
    }

    /// Returns one scene item's filter chain.
    ///
    /// Borrows the stored chain instead of cloning it for read-only access.
    #[must_use]
    pub fn source_filters(&self, scene: &str, source: SourceId) -> Option<&[FrameFilter]> {
        Some(
            self.scenes
                .get(scene)?
                .items
                .get(&source)?
                .filters
                .as_slice(),
        )
    }

    /// Destroys a source that is not attached to any scene.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownSource`] for an unknown ID or
    /// [`RuntimeError::SourceInUse`] while a scene still references the source.
    pub fn destroy_source(&mut self, source: SourceId) -> Result<(), RuntimeError> {
        if !self.sources.contains_key(&source) {
            return Err(RuntimeError::UnknownSource(source));
        }
        // Constant-time replacement for scanning every scene's item list.
        if self.scene_references.contains_key(&source) {
            return Err(RuntimeError::SourceInUse(source));
        }
        self.sources.remove(&source);
        Ok(())
    }

    /// Destroys a scene and all of its item references.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist.
    pub fn destroy_scene(&mut self, scene: &str) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(removed) = self.scenes.remove(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        self.filter_count = self.filter_count.saturating_sub(removed.filter_count());
        for source in removed.attached {
            release_scene_reference(&mut self.scene_references, source);
        }
        Ok(())
    }

    /// Applies new settings to a source instance.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownSource`] when the ID is not live or
    /// [`RuntimeError::Source`] when the source rejects the settings.
    pub fn update_source(
        &mut self,
        source: SourceId,
        settings: &Config,
    ) -> Result<(), RuntimeError> {
        let instance = self
            .sources
            .get_mut(&source)
            .ok_or(RuntimeError::UnknownSource(source))?;
        instance
            .source
            .update(settings)
            .map_err(RuntimeError::Source)
    }
}

/// Drops one scene's claim on `source`, forgetting the entry at zero.
fn release_scene_reference(references: &mut HashMap<SourceId, usize>, source: SourceId) {
    if let Some(count) = references.get_mut(&source) {
        *count -= 1;
        if *count == 0 {
            references.remove(&source);
        }
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
