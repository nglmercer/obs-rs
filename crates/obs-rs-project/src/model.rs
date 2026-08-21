use obs_rs_config::Config;
use obs_rs_media::{FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::Identifier;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap, HashSet},
    sync::OnceLock,
};

use super::{error::ProjectError, validation::identifier};

/// The OBS filter group a source filter belongs to.
///
/// The project stores this classification instead of deriving it from a
/// renderer operation. That leaves room for audio/video filters and effect
/// filters to coexist, including plugin-provided kinds the reference renderer
/// does not know how to compile yet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceFilterCategory {
    /// Filters that operate on audio or on an asynchronous source stream.
    AudioVideo,
    /// Filters that operate on the rendered source image.
    #[default]
    Effect,
}

impl SourceFilterCategory {
    /// Returns the stable serialized category ID.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::AudioVideo => "audio_video",
            Self::Effect => "effect",
        }
    }

    /// Parses a serialized category ID.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "audio_video" => Some(Self::AudioVideo),
            "effect" => Some(Self::Effect),
            _ => None,
        }
    }
}

/// One named, ordered filter instance attached to a source.
///
/// This is deliberately a project value rather than a renderer enum. `kind`
/// identifies the filter implementation and `settings` belongs only to that
/// instance, so two filters of the same kind can have different names and
/// values without relying on a comma-separated UI string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFilterSpec {
    pub(crate) id: Identifier,
    pub(crate) name: String,
    pub(crate) kind: Identifier,
    pub(crate) category: SourceFilterCategory,
    pub(crate) enabled: bool,
    pub(crate) settings: Config,
}

impl SourceFilterSpec {
    /// Creates an enabled effect filter instance.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when an identifier, kind, or display name is
    /// invalid.
    pub fn new(id: &str, name: &str, kind: &str, settings: Config) -> Result<Self, ProjectError> {
        Self::with_category(id, name, kind, SourceFilterCategory::Effect, settings)
    }

    /// Creates an enabled filter instance with an explicit OBS filter group.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when an identifier, kind, or display name is
    /// invalid.
    pub fn with_category(
        id: &str,
        name: &str,
        kind: &str,
        category: SourceFilterCategory,
        settings: Config,
    ) -> Result<Self, ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "filter" });
        }
        Ok(Self {
            id: identifier(id, "filter id")?,
            name: name.to_owned(),
            kind: identifier(kind, "filter kind")?,
            category,
            enabled: true,
            settings,
        })
    }

    /// Returns the stable filter-instance ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the user-visible filter name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Replaces the filter display name after validating it.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] when the name is empty.
    pub fn set_name(&mut self, name: &str) -> Result<(), ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "filter" });
        }
        name.clone_into(&mut self.name);
        Ok(())
    }

    /// Returns the registered filter kind.
    #[must_use]
    pub fn kind(&self) -> &Identifier {
        &self.kind
    }

    /// Returns the OBS filter group.
    #[must_use]
    pub const fn category(&self) -> SourceFilterCategory {
        self.category
    }

    /// Returns whether this instance participates in the runtime chain.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Changes whether this instance participates in the runtime chain.
    pub const fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Returns this instance's independent settings document.
    #[must_use]
    pub const fn settings(&self) -> &Config {
        &self.settings
    }

    /// Replaces this instance's settings document.
    pub fn set_settings(&mut self, settings: Config) {
        self.settings = settings;
    }
}

/// A source definition stored in a profile-wide source registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpec {
    pub(crate) id: Identifier,
    pub(crate) kind: Identifier,
    pub(crate) name: String,
    pub(crate) settings: Config,
    pub(crate) filters: Vec<SourceFilterSpec>,
}

impl SourceSpec {
    /// Creates a source definition with no filters.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when an identifier or source name is invalid.
    pub fn new(id: &str, kind: &str, name: &str, settings: Config) -> Result<Self, ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "source" });
        }
        Ok(Self {
            id: identifier(id, "source id")?,
            kind: identifier(kind, "source kind")?,
            name: name.to_owned(),
            settings,
            filters: Vec::new(),
        })
    }

    /// Returns the stable project-local source ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the registered runtime source kind.
    #[must_use]
    pub fn kind(&self) -> &Identifier {
        &self.kind
    }

    /// Returns the user-facing source name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Replaces the source's display name after validating that it is non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] when `name` is empty or whitespace.
    pub fn set_name(&mut self, name: &str) -> Result<(), ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "source" });
        }
        name.clone_into(&mut self.name);
        Ok(())
    }

    /// Returns source settings.
    #[must_use]
    pub const fn settings(&self) -> &Config {
        &self.settings
    }

    /// Replaces source settings.
    pub fn set_settings(&mut self, settings: Config) {
        self.settings = settings;
    }

    /// Returns persistent filter instances in application order.
    #[must_use]
    pub fn filters(&self) -> &[SourceFilterSpec] {
        &self.filters
    }

    /// Appends a filter instance to the source's ordered filter chain.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateFilter`] when the filter ID is already
    /// present in this source.
    pub fn add_filter(&mut self, filter: SourceFilterSpec) -> Result<(), ProjectError> {
        if self
            .filters
            .iter()
            .any(|existing| existing.id() == filter.id())
        {
            return Err(ProjectError::DuplicateFilter(filter.id().clone()));
        }
        self.filters.push(filter);
        Ok(())
    }

    /// Finds a filter instance by ID.
    #[must_use]
    pub fn filter(&self, id: &Identifier) -> Option<&SourceFilterSpec> {
        self.filters.iter().find(|filter| filter.id() == id)
    }

    /// Finds a mutable filter instance by ID.
    pub fn filter_mut(&mut self, id: &Identifier) -> Option<&mut SourceFilterSpec> {
        self.filters.iter_mut().find(|filter| filter.id() == id)
    }

    /// Removes a filter instance by ID.
    pub fn remove_filter(&mut self, id: &Identifier) -> Option<SourceFilterSpec> {
        let index = self.filters.iter().position(|filter| filter.id() == id)?;
        Some(self.filters.remove(index))
    }

    /// Moves a filter instance to an existing order position.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownFilter`] for an unknown ID or
    /// [`ProjectError::InvalidFilterOrder`] for an out-of-range index.
    pub fn move_filter(
        &mut self,
        id: &Identifier,
        target_index: usize,
    ) -> Result<(), ProjectError> {
        let current_index = self
            .filters
            .iter()
            .position(|filter| filter.id() == id)
            .ok_or_else(|| ProjectError::UnknownFilter(id.clone()))?;
        if target_index >= self.filters.len() {
            return Err(ProjectError::InvalidFilterOrder {
                index: target_index,
            });
        }
        let filter = self.filters.remove(current_index);
        self.filters.insert(target_index, filter);
        Ok(())
    }
}

/// One scene-local reference to a source definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneItemSpec {
    pub(crate) id: Identifier,
    pub(crate) source_id: Identifier,
    pub(crate) transform: FrameTransform,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
}

impl SceneItemSpec {
    /// Creates an item reference with the default scene-item state.
    ///
    /// # Errors
    ///
    /// Returns an identifier validation error when either ID is empty or
    /// contains unsupported characters.
    pub fn new(id: &str, source_id: &str) -> Result<Self, ProjectError> {
        Ok(Self {
            id: identifier(id, "scene item id")?,
            source_id: identifier(source_id, "source id")?,
            transform: FrameTransform::IDENTITY,
            visible: true,
            locked: false,
        })
    }

    /// Creates the conventional item whose ID starts out equal to its source ID.
    ///
    /// # Errors
    ///
    /// Returns an identifier validation error when `source_id` is invalid.
    pub fn for_source(source_id: &str) -> Result<Self, ProjectError> {
        Self::new(source_id, source_id)
    }

    /// Returns the stable scene-item ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the referenced source ID.
    #[must_use]
    pub fn source_id(&self) -> &Identifier {
        &self.source_id
    }

    /// Returns the scene-item transform.
    #[must_use]
    pub const fn transform(&self) -> FrameTransform {
        self.transform
    }

    /// Sets the scene-item transform.
    pub const fn set_transform(&mut self, transform: FrameTransform) {
        self.transform = transform;
    }

    /// Returns whether this item participates in scene composition.
    #[must_use]
    pub const fn visible(&self) -> bool {
        self.visible
    }

    /// Sets whether this item participates in scene composition.
    pub const fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Returns whether this item is protected from desktop editing.
    #[must_use]
    pub const fn locked(&self) -> bool {
        self.locked
    }

    /// Sets whether this item is protected from desktop editing.
    pub const fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }
}

/// An ordered scene collection entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneSpec {
    pub(crate) id: Identifier,
    pub(crate) name: String,
    /// Composition order, which is part of the scene's meaning.
    pub(crate) items: Vec<SceneItemSpec>,
    /// O(1) item lookup and membership mirror. Values are indices into the
    /// ordered `items` vector, so keyed mutations do not scan the scene.
    pub(crate) item_ids: HashMap<Identifier, usize>,
}

impl SceneSpec {
    /// Creates an empty scene definition.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] for an empty scene name or an
    /// identifier validation error for an invalid ID.
    pub fn new(id: &str, name: &str) -> Result<Self, ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "scene" });
        }
        Ok(Self {
            id: identifier(id, "scene id")?,
            name: name.to_owned(),
            items: Vec::new(),
            item_ids: HashMap::new(),
        })
    }

    /// Returns the stable scene ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the user-facing scene name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns scene items in compositor order.
    #[must_use]
    pub fn items(&self) -> &[SceneItemSpec] {
        &self.items
    }

    /// Compatibility alias for callers that only need ordered scene rows.
    #[must_use]
    pub fn sources(&self) -> &[SceneItemSpec] {
        self.items()
    }

    /// Replaces the scene's display name after validating that it is non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] when `name` is empty or whitespace.
    pub fn set_name(&mut self, name: &str) -> Result<(), ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "scene" });
        }
        name.clone_into(&mut self.name);
        Ok(())
    }

    /// Appends a scene item while rejecting duplicate item IDs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSceneItem`] when the item ID is already
    /// present in this scene.
    pub fn add_item(&mut self, item: SceneItemSpec) -> Result<(), ProjectError> {
        if self.item_ids.contains_key(item.id()) {
            return Err(ProjectError::DuplicateSceneItem(item.id().clone()));
        }
        let index = self.items.len();
        self.item_ids.insert(item.id().clone(), index);
        self.items.push(item);
        Ok(())
    }

    /// Appends several scene items, rejecting duplicates atomically.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSceneItem`] when an incoming item or
    /// an existing item has the same ID.
    pub fn add_items(
        &mut self,
        items: impl IntoIterator<Item = SceneItemSpec>,
    ) -> Result<(), ProjectError> {
        let incoming: Vec<SceneItemSpec> = items.into_iter().collect();
        let mut candidate = HashSet::with_capacity(incoming.len());
        for item in &incoming {
            if self.item_ids.contains_key(item.id()) || !candidate.insert(item.id()) {
                return Err(ProjectError::DuplicateSceneItem(item.id().clone()));
            }
        }
        self.items.reserve(incoming.len());
        for item in incoming {
            let index = self.items.len();
            self.item_ids.insert(item.id().clone(), index);
            self.items.push(item);
        }
        Ok(())
    }

    /// Returns whether this scene contains an item with `id`.
    #[must_use]
    pub fn has_item<Q>(&self, id: &Q) -> bool
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.item_ids.contains_key(id)
    }

    /// Returns whether an item references the source ID.
    #[must_use]
    pub fn has_source<Q>(&self, source_id: &Q) -> bool
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.items
            .iter()
            .any(|item| item.source_id.borrow() == source_id)
    }

    /// Finds a mutable scene item by project-local ID.
    pub fn item_mut(&mut self, id: &Identifier) -> Option<&mut SceneItemSpec> {
        let index = *self.item_ids.get(id)?;
        self.items.get_mut(index)
    }

    /// Finds an immutable scene item by project-local ID.
    #[must_use]
    pub fn item<Q>(&self, id: &Q) -> Option<&SceneItemSpec>
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        let index = *self.item_ids.get(id)?;
        self.items.get(index)
    }

    /// Removes one scene item and returns it.
    pub fn remove_item(&mut self, id: &Identifier) -> Option<SceneItemSpec> {
        let index = self.item_ids.remove(id)?;
        let item = self.items.remove(index);
        for (index, item) in self.items.iter().enumerate().skip(index) {
            if let Some(entry) = self.item_ids.get_mut(item.id()) {
                *entry = index;
            }
        }
        Some(item)
    }

    /// Moves one scene item to an existing order position.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownSceneItem`] for an unknown ID or
    /// [`ProjectError::InvalidSceneItemOrder`] for an out-of-range index.
    pub fn move_item(&mut self, id: &Identifier, target_index: usize) -> Result<(), ProjectError> {
        let current_index = self
            .item_ids
            .get(id)
            .copied()
            .ok_or_else(|| ProjectError::UnknownSceneItem(id.clone()))?;
        if target_index >= self.items.len() {
            return Err(ProjectError::InvalidSceneItemOrder {
                index: target_index,
            });
        }
        let item = self.items.remove(current_index);
        self.items.insert(target_index, item);
        let first = current_index.min(target_index);
        for (index, item) in self.items.iter().enumerate().skip(first) {
            if let Some(entry) = self.item_ids.get_mut(item.id()) {
                *entry = index;
            }
        }
        Ok(())
    }
}
/// Preferred renderer for one project profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderBackendPreference {
    /// Deterministic portable renderer and compatibility default.
    #[default]
    Cpu,
    /// Optional accelerated renderer, with explicit CPU fallback if unavailable.
    Wgpu,
}

impl RenderBackendPreference {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Wgpu => "wgpu",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "cpu" => Some(Self::Cpu),
            "wgpu" => Some(Self::Wgpu),
            _ => None,
        }
    }
}

/// One named profile containing a source registry, video settings, and scenes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    pub(crate) id: Identifier,
    pub(crate) name: String,
    pub(crate) video_format: VideoFormat,
    pub(crate) render_backend: RenderBackendPreference,
    pub(crate) output_kind: OutputProfileKind,
    pub(crate) sources: BTreeMap<Identifier, SourceSpec>,
    pub(crate) scenes: BTreeMap<Identifier, SceneSpec>,
}

impl Profile {
    /// Creates an empty profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when the profile ID or name is invalid.
    pub fn new(id: &str, name: &str, video_format: VideoFormat) -> Result<Self, ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "profile" });
        }
        Ok(Self {
            id: identifier(id, "profile id")?,
            name: name.to_owned(),
            video_format,
            render_backend: RenderBackendPreference::Cpu,
            output_kind: OutputProfileKind::ReferencePacket,
            sources: BTreeMap::new(),
            scenes: BTreeMap::new(),
        })
    }

    /// Returns the stable profile ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the profile display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the configured video format.
    #[must_use]
    pub const fn video_format(&self) -> VideoFormat {
        self.video_format
    }

    /// Replaces the canvas resolution and frame rate used to render this profile.
    pub const fn set_video_format(&mut self, video_format: VideoFormat) {
        self.video_format = video_format;
    }

    /// Returns the requested renderer. Availability is negotiated at runtime.
    #[must_use]
    pub const fn render_backend(&self) -> RenderBackendPreference {
        self.render_backend
    }

    /// Selects a renderer without changing the deterministic fallback policy.
    pub const fn set_render_backend(&mut self, preference: RenderBackendPreference) {
        self.render_backend = preference;
    }

    /// Returns the exact requested output profile.
    #[must_use]
    pub const fn output_profile(&self) -> OutputProfileKind {
        self.output_kind
    }

    /// Selects an exact output profile. Runtime negotiation may report it unavailable.
    pub const fn set_output_profile(&mut self, profile: OutputProfileKind) {
        self.output_kind = profile;
    }

    /// Adds a source to the profile-wide registry.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSource`] when the source ID is already
    /// registered.
    pub fn add_source(&mut self, source: SourceSpec) -> Result<(), ProjectError> {
        if self.sources.contains_key(source.id()) {
            return Err(ProjectError::DuplicateSource(source.id().clone()));
        }
        self.sources.insert(source.id().clone(), source);
        Ok(())
    }

    /// Adds several sources, rejecting duplicates atomically.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSource`] when an incoming source or an
    /// existing source has the same ID.
    pub fn add_sources(
        &mut self,
        sources: impl IntoIterator<Item = SourceSpec>,
    ) -> Result<(), ProjectError> {
        let incoming: Vec<SourceSpec> = sources.into_iter().collect();
        let mut candidate = HashSet::with_capacity(incoming.len());
        for source in &incoming {
            if self.sources.contains_key(source.id()) || !candidate.insert(source.id()) {
                return Err(ProjectError::DuplicateSource(source.id().clone()));
            }
        }
        for source in incoming {
            self.sources.insert(source.id().clone(), source);
        }
        Ok(())
    }

    /// Returns sources in deterministic ID order.
    pub fn sources(&self) -> impl Iterator<Item = &SourceSpec> {
        self.sources.values()
    }

    /// Returns whether the profile registry contains `id`.
    #[must_use]
    pub fn has_source<Q>(&self, id: &Q) -> bool
    where
        Identifier: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.sources.contains_key(id)
    }

    /// Returns a source by ID.
    #[must_use]
    pub fn source<Q>(&self, id: &Q) -> Option<&SourceSpec>
    where
        Identifier: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.sources.get(id)
    }

    /// Returns a mutable source by ID.
    pub fn source_mut(&mut self, id: &Identifier) -> Option<&mut SourceSpec> {
        self.sources.get_mut(id)
    }

    /// Returns whether any scene item references this source.
    #[must_use]
    pub fn source_in_use(&self, id: &Identifier) -> bool {
        self.scenes.values().any(|scene| scene.has_source(id))
    }

    /// Removes an unreferenced source from the registry.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::SourceInUse`] when a scene still references the
    /// source or [`ProjectError::UnknownSource`] when the ID is absent.
    pub fn remove_source(&mut self, id: &Identifier) -> Result<SourceSpec, ProjectError> {
        if self.source_in_use(id) {
            return Err(ProjectError::SourceInUse(id.clone()));
        }
        self.sources
            .remove(id)
            .ok_or_else(|| ProjectError::UnknownSource(id.clone()))
    }

    /// Adds a scene while rejecting duplicate IDs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateScene`] when the ID is already present.
    pub fn add_scene(&mut self, scene: SceneSpec) -> Result<(), ProjectError> {
        if self.scenes.contains_key(scene.id()) {
            return Err(ProjectError::DuplicateScene(scene.id().clone()));
        }
        self.scenes.insert(scene.id().clone(), scene);
        Ok(())
    }

    /// Removes a scene by ID and returns its definition.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownScene`] when the scene is absent.
    pub fn remove_scene(&mut self, id: &Identifier) -> Result<SceneSpec, ProjectError> {
        self.scenes
            .remove(id)
            .ok_or_else(|| ProjectError::UnknownScene(id.clone()))
    }

    /// Returns scenes in deterministic ID order.
    pub fn scenes(&self) -> impl Iterator<Item = &SceneSpec> {
        self.scenes.values()
    }

    /// Returns a scene by ID.
    ///
    /// Scenes are stored in a keyed map, so this is a direct lookup rather than
    /// a walk of the scene list. The key may be an [`Identifier`] or a plain
    /// `&str`, so callers holding borrowed text need not build one.
    #[must_use]
    pub fn scene<Q>(&self, id: &Q) -> Option<&SceneSpec>
    where
        Identifier: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.scenes.get(id)
    }

    /// Returns a mutable scene by ID.
    pub fn scene_mut(&mut self, id: &Identifier) -> Option<&mut SceneSpec> {
        self.scenes.get_mut(id)
    }
}
/// A complete project document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    pub(crate) title: String,
    pub(crate) active_profile: Identifier,
    pub(crate) profiles: BTreeMap<Identifier, Profile>,
}

impl Project {
    /// Creates a project with no profiles.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] for an empty title.
    pub fn new(title: &str) -> Result<Self, ProjectError> {
        if title.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "project" });
        }
        Ok(Self {
            title: title.to_owned(),
            active_profile: default_profile_id().clone(),
            profiles: BTreeMap::new(),
        })
    }

    /// Returns the project title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the active profile ID.
    #[must_use]
    pub fn active_profile(&self) -> &Identifier {
        &self.active_profile
    }

    /// Adds a profile while rejecting duplicate IDs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateProfile`] when the ID is already present.
    pub fn add_profile(&mut self, profile: Profile) -> Result<(), ProjectError> {
        if self.profiles.contains_key(profile.id()) {
            return Err(ProjectError::DuplicateProfile(profile.id().clone()));
        }
        if self.profiles.is_empty() {
            self.active_profile = profile.id().clone();
        }
        self.profiles.insert(profile.id().clone(), profile);
        Ok(())
    }

    /// Selects the active profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownProfile`] when the ID is absent.
    pub fn set_active_profile(&mut self, id: &str) -> Result<(), ProjectError> {
        let id = identifier(id, "profile id")?;
        if !self.profiles.contains_key(&id) {
            return Err(ProjectError::UnknownProfile(id));
        }
        self.active_profile = id;
        Ok(())
    }

    /// Returns profiles in deterministic ID order.
    pub fn profiles(&self) -> impl Iterator<Item = &Profile> {
        self.profiles.values()
    }

    /// Returns a profile by ID, keyed by [`Identifier`] or `&str`.
    #[must_use]
    pub fn profile<Q>(&self, id: &Q) -> Option<&Profile>
    where
        Identifier: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.profiles.get(id)
    }

    /// Returns the profile named by [`Project::active_profile`].
    ///
    /// Profiles are stored in a keyed map, so resolving the active profile is a
    /// direct lookup; callers used to rescan the whole profile list, often
    /// several times per UI command.
    #[must_use]
    pub fn active_profile_spec(&self) -> Option<&Profile> {
        self.profiles.get(&self.active_profile)
    }

    /// Returns a mutable profile by ID.
    pub fn profile_mut(&mut self, id: &Identifier) -> Option<&mut Profile> {
        self.profiles.get_mut(id)
    }
}

/// Returns the shared `"default"` profile identifier.
///
/// Validated and allocated once for the process instead of on every
/// [`Project::new`]; callers clone the cached value.
fn default_profile_id() -> &'static Identifier {
    static DEFAULT_PROFILE_ID: OnceLock<Identifier> = OnceLock::new();
    DEFAULT_PROFILE_ID.get_or_init(|| {
        Identifier::new("default")
            .unwrap_or_else(|_| unreachable!("\"default\" is a valid identifier"))
    })
}
