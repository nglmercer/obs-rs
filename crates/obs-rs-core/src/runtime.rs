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

const MAX_RUNTIME_SCENE_ITEM_ID_BYTES: usize = 4_096;

pub struct Runtime {
    pub(crate) registry: Registry,
    /// Keyed lookup only; never iterated in key order, so hashing is safe here
    /// and keeps `sources.get_mut` off the compositor's O(log n) path.
    pub(crate) sources: HashMap<SourceId, SourceInstance>,
    /// Keyed lookup only. Deterministic rendering comes from each scene's
    /// ordered source vector, not from scene-map iteration order.
    pub(crate) scenes: HashMap<Identifier, Scene>,
    /// How many scene items currently reference each source.
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
                filters: Vec::new(),
                last_frame: None,
                failure: None,
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

    /// Appends a scene-item reference to a source, including when the source is
    /// already present in that scene. The underlying capture device remains a
    /// single shared source instance; the returned index identifies the new
    /// scene-local transform slot.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the scene or source is unknown, or when
    /// the scene-item limit has been reached.
    pub fn attach_source_instance(
        &mut self,
        scene: &str,
        source: SourceId,
    ) -> Result<usize, RuntimeError> {
        let name = identifier(scene, "scene")?;
        let item_id = self.scenes.get(&name).map_or_else(
            || "item-0".to_owned(),
            |scene| format!("item-{}", scene.sources.len()),
        );
        self.attach_source_instance_with_id(scene, source, &item_id)
    }

    /// Appends a scene-item reference with a stable project-derived identity.
    /// The identity is independent of the scene's current draw order, so a
    /// reorder does not redirect a transient transform draft to another item.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::DuplicateSceneItem`] when the identity already
    /// exists in the scene, or [`RuntimeError::InvalidName`] when it is empty
    /// or exceeds the bounded path length.
    pub fn attach_source_instance_with_id(
        &mut self,
        scene: &str,
        source: SourceId,
        item_id: &str,
    ) -> Result<usize, RuntimeError> {
        let name = identifier(scene, "scene")?;
        validate_scene_item_id(item_id)?;
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
        if scene
            .item_ids
            .iter()
            .any(|existing| existing.as_ref() == item_id)
        {
            return Err(RuntimeError::DuplicateSceneItem(item_id.to_owned()));
        }
        let Some(index) = scene.attach_instance_with_id(source, item_id) else {
            return Err(RuntimeError::DuplicateSceneItem(item_id.to_owned()));
        };
        *self.scene_references.entry(source).or_insert(0) += 1;
        Ok(index)
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
        let Some(()) = scene.detach(source) else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        release_scene_reference(&mut self.scene_references, source);
        Ok(())
    }

    /// Removes every scene-item reference while keeping the scene and all
    /// shared source instances alive.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist.
    pub fn clear_scene_sources(&mut self, scene: &str) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let sources = {
            let Some(scene) = self.scenes.get_mut(&name) else {
                return Err(RuntimeError::UnknownScene(name));
            };
            let sources = std::mem::take(&mut scene.sources);
            scene.items.clear();
            scene.attached.clear();
            scene.item_ids.clear();
            sources
        };
        for source in sources {
            release_scene_reference(&mut self.scene_references, source);
        }
        Ok(())
    }

    /// Sets the transform and opacity for one scene item.
    ///
    /// # Errors
    ///
    /// The filter chain is source-owned, so it is shared by every attached
    /// scene item that references this runtime source.
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
        let Some(item) = scene.items.iter_mut().find(|item| item.source == source) else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        item.transform = transform;
        Ok(())
    }

    /// Returns a scene item's current transform.
    #[must_use]
    pub fn source_transform(&self, scene: &str, source: SourceId) -> Option<FrameTransform> {
        self.scenes
            .get(scene)?
            .items
            .iter()
            .find(|item| item.source == source)
            .map(|item| item.transform)
    }

    /// Sets the transform for one ordered scene-item reference.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SceneItemOutOfBounds`] for an invalid item index.
    pub fn set_scene_item_transform(
        &mut self,
        scene: &str,
        item_index: usize,
        transform: FrameTransform,
    ) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        let Some(item) = scene.items.get_mut(item_index) else {
            return Err(RuntimeError::SceneItemOutOfBounds { index: item_index });
        };
        item.transform = transform;
        Ok(())
    }

    /// Returns the transform for one ordered scene-item reference.
    #[must_use]
    pub fn scene_item_transform(&self, scene: &str, item_index: usize) -> Option<FrameTransform> {
        self.scenes
            .get(scene)?
            .items
            .get(item_index)
            .map(|item| item.transform)
    }

    /// Returns stable scene-item identities in current draw order.
    #[must_use]
    pub fn scene_item_ids(&self, scene: &str) -> Option<Vec<String>> {
        Some(
            self.scenes
                .get(scene)?
                .items
                .iter()
                .map(|item| item.item_id.as_ref().to_owned())
                .collect(),
        )
    }

    /// Sets a transform by stable scene-item identity.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SceneItemNotAttached`] when the identity is not
    /// present in the requested scene.
    pub fn set_scene_item_transform_by_id(
        &mut self,
        scene: &str,
        item_id: &str,
        transform: FrameTransform,
    ) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(scene) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        let Some(item) = scene
            .items
            .iter_mut()
            .find(|item| item.item_id.as_ref() == item_id)
        else {
            return Err(RuntimeError::SceneItemNotAttached(item_id.to_owned()));
        };
        item.transform = transform;
        Ok(())
    }

    /// Returns a stable scene-item transform by identity.
    #[must_use]
    pub fn scene_item_transform_by_id(&self, scene: &str, item_id: &str) -> Option<FrameTransform> {
        self.scenes
            .get(scene)?
            .items
            .iter()
            .find(|item| item.item_id.as_ref() == item_id)
            .map(|item| item.transform)
    }

    /// Adds a CPU filter to the shared source filter chain.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownSource`] when the source does not exist.
    pub fn add_source_filter(
        &mut self,
        source: SourceId,
        filter: FrameFilter,
    ) -> Result<(), RuntimeError> {
        let limit = self.limits.max_filters_per_source();
        let instance = self
            .sources
            .get_mut(&source)
            .ok_or(RuntimeError::UnknownSource(source))?;
        if instance.filters.len() >= limit {
            return Err(RuntimeError::ResourceLimitExceeded {
                resource: "filters per source",
                limit,
            });
        }
        instance.filters.push(filter);
        self.filter_count = self.filter_count.saturating_add(1);
        Ok(())
    }

    /// Removes every CPU filter from one shared source.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn clear_source_filters(&mut self, source: SourceId) -> Result<(), RuntimeError> {
        let instance = self
            .sources
            .get_mut(&source)
            .ok_or(RuntimeError::UnknownSource(source))?;
        self.filter_count = self.filter_count.saturating_sub(instance.filters.len());
        instance.filters.clear();
        Ok(())
    }

    /// Returns one shared source's filter chain.
    ///
    /// Borrows the stored chain instead of cloning it for read-only access.
    #[must_use]
    pub fn source_filters(&self, source: SourceId) -> Option<&[FrameFilter]> {
        Some(self.sources.get(&source)?.filters.as_slice())
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
        if let Some(instance) = self.sources.get(&source) {
            self.filter_count = self.filter_count.saturating_sub(instance.filters.len());
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
        for source in removed.sources {
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
        // New settings can change the frame shape, so the retained frame is
        // dropped rather than composited at the old size.
        instance.last_frame = None;
        instance.failure = None;
        instance
            .source
            .update(settings)
            .map_err(RuntimeError::Source)
    }

    /// Renames a source instance without recreating it.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownSource`] when the ID is not live or
    /// [`RuntimeError::InvalidName`] for an empty name.
    pub fn rename_source(&mut self, source: SourceId, name: &str) -> Result<(), RuntimeError> {
        if name.trim().is_empty() {
            return Err(RuntimeError::InvalidName { kind: "source" });
        }
        let instance = self
            .sources
            .get_mut(&source)
            .ok_or(RuntimeError::UnknownSource(source))?;
        name.clone_into(&mut instance.name);
        Ok(())
    }

    /// Reorders one scene's composition order to exactly `order`.
    ///
    /// This is the incremental counterpart of tearing a scene down and
    /// rebuilding it: reordering, showing, and hiding items are scene-graph
    /// edits and must not restart the capture devices underneath them.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist and
    /// [`RuntimeError::UnknownSource`] when `order` names a source that is not
    /// attached to it.
    pub fn set_scene_order(&mut self, scene: &str, order: &[SourceId]) -> Result<(), RuntimeError> {
        let name = identifier(scene, "scene")?;
        let Some(state) = self.scenes.get_mut(&name) else {
            return Err(RuntimeError::UnknownScene(name));
        };
        for source in order {
            if !state.attached.contains(source) {
                return Err(RuntimeError::SourceNotAttached(*source));
            }
        }
        // A partial order would silently drop the items it omits, so the caller
        // has to name every attached item exactly once.
        if order.len() != state.sources.len() {
            return Err(RuntimeError::InvalidName {
                kind: "scene order",
            });
        }
        let mut expected = HashMap::new();
        for source in &state.sources {
            *expected.entry(*source).or_insert(0usize) += 1;
        }
        let mut requested = HashMap::new();
        for source in order {
            *requested.entry(*source).or_insert(0usize) += 1;
        }
        if expected != requested {
            return Err(RuntimeError::InvalidName {
                kind: "scene order",
            });
        }
        let mut remaining = state.items.clone();
        let mut next_items = Vec::with_capacity(order.len());
        for source in order {
            let Some(index) = remaining.iter().position(|item| item.source == *source) else {
                return Err(RuntimeError::InvalidName {
                    kind: "scene order",
                });
            };
            next_items.push(remaining.remove(index));
        }
        state.sources = order.to_vec();
        state.items = next_items;
        Ok(())
    }

    /// Returns the source IDs that exist in this runtime, in no order.
    #[must_use]
    pub fn source_ids(&self) -> Vec<SourceId> {
        self.sources.keys().copied().collect()
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

fn validate_scene_item_id(item_id: &str) -> Result<(), RuntimeError> {
    if item_id.is_empty() || item_id.len() > MAX_RUNTIME_SCENE_ITEM_ID_BYTES {
        return Err(RuntimeError::InvalidName {
            kind: "scene item id",
        });
    }
    Ok(())
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
