//! Source, filter, scene, and nested-group project values.

use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
};

use obs_rs_config::Config;
use obs_rs_media::{FrameTransform, StingerSpec, TransitionSpec};
use obs_rs_util::Identifier;

use super::super::{error::ProjectError, validation::identifier};

pub(crate) const MAX_GROUP_NESTING_DEPTH: usize = 64;

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

/// The thing one scene item draws.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SceneItemTarget {
    Source(Identifier),
    Scene(Identifier),
    Group(Box<GroupSpec>),
}

impl SceneItemTarget {
    fn id(&self) -> &Identifier {
        match self {
            Self::Source(id) | Self::Scene(id) => id,
            Self::Group(group) => group.id(),
        }
    }
}

/// One scene-local reference to a source definition or another scene.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneItemSpec {
    pub(crate) id: Identifier,
    target: SceneItemTarget,
    pub(crate) transform: FrameTransform,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
}

/// One source item after a nested-scene graph is flattened for a renderer.
///
/// The project remains the source of truth; runtime adapters map the stable
/// source ID to their own live source handle without rebuilding this state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlattenedSceneItem {
    pub(super) item_id: String,
    pub(super) source_id: Identifier,
    pub(super) transform: FrameTransform,
}

impl FlattenedSceneItem {
    /// Returns the stable path of the scene item that produced this runtime
    /// layer. Nested group and scene references use an outer-to-inner path.
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the profile-wide source definition ID.
    #[must_use]
    pub fn source_id(&self) -> &Identifier {
        &self.source_id
    }

    /// Returns the transform after nested axis-aligned composition.
    #[must_use]
    pub const fn transform(&self) -> FrameTransform {
        self.transform
    }
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
            target: SceneItemTarget::Source(identifier(source_id, "source id")?),
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

    /// Creates an item that draws another scene as a nested scene source.
    ///
    /// The target scene is validated when the item is added to a profile or
    /// loaded from a project document, because the target may be declared
    /// later in the profile's serialized scene order.
    ///
    /// # Errors
    ///
    /// Returns an identifier validation error when either ID is invalid.
    pub fn for_scene(id: &str, scene_id: &str) -> Result<Self, ProjectError> {
        Ok(Self {
            id: identifier(id, "scene item id")?,
            target: SceneItemTarget::Scene(identifier(scene_id, "scene id")?),
            transform: FrameTransform::IDENTITY,
            visible: true,
            locked: false,
        })
    }

    /// Creates a group item whose child list starts empty.
    ///
    /// Child sources, nested scenes, and nested groups can be appended through
    /// [`GroupSpec::add_item`] before the group item is added to a scene.
    ///
    /// # Errors
    ///
    /// Returns an identifier or name validation error when the group ID or
    /// display name is invalid.
    pub fn for_group(id: &str, name: &str) -> Result<Self, ProjectError> {
        let group = GroupSpec::new(id, name)?;
        Self::with_group(id, group)
    }

    pub(crate) fn with_group(id: &str, group: GroupSpec) -> Result<Self, ProjectError> {
        Ok(Self {
            id: identifier(id, "scene item id")?,
            target: SceneItemTarget::Group(Box::new(group)),
            transform: FrameTransform::IDENTITY,
            visible: true,
            locked: false,
        })
    }

    /// Returns the stable scene-item ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the target ID.
    ///
    /// For a source or nested-scene item this is the referenced ID; for a
    /// group it is the group's local ID. Call [`Self::scene_id`],
    /// [`Self::group`], or [`Self::is_source`] when the target kind matters.
    #[must_use]
    pub fn source_id(&self) -> &Identifier {
        self.target.id()
    }

    /// Returns the referenced nested scene ID, if this is a scene item.
    #[must_use]
    pub fn scene_id(&self) -> Option<&Identifier> {
        match &self.target {
            SceneItemTarget::Source(_) | SceneItemTarget::Group(_) => None,
            SceneItemTarget::Scene(id) => Some(id),
        }
    }

    /// Returns the nested group definition, if this item is a group.
    #[must_use]
    pub fn group(&self) -> Option<&GroupSpec> {
        match &self.target {
            SceneItemTarget::Group(group) => Some(group),
            SceneItemTarget::Source(_) | SceneItemTarget::Scene(_) => None,
        }
    }

    /// Returns mutable access to this group's child definition.
    pub fn group_mut(&mut self) -> Option<&mut GroupSpec> {
        match &mut self.target {
            SceneItemTarget::Group(group) => Some(group),
            SceneItemTarget::Source(_) | SceneItemTarget::Scene(_) => None,
        }
    }

    /// Returns whether this item references a profile source definition.
    #[must_use]
    pub const fn is_source(&self) -> bool {
        matches!(self.target, SceneItemTarget::Source(_))
    }

    /// Returns whether this item references another scene.
    #[must_use]
    pub const fn is_scene_reference(&self) -> bool {
        matches!(self.target, SceneItemTarget::Scene(_))
    }

    /// Returns whether this item owns a nested group.
    #[must_use]
    pub const fn is_group(&self) -> bool {
        matches!(self.target, SceneItemTarget::Group(_))
    }

    pub(crate) fn set_source_id(&mut self, source_id: Identifier) {
        self.target = SceneItemTarget::Source(source_id);
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

/// A scene-local ordered group of source, scene, or group items.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupSpec {
    pub(crate) id: Identifier,
    pub(crate) name: String,
    pub(crate) items: Vec<SceneItemSpec>,
    pub(crate) item_ids: HashMap<Identifier, usize>,
}

impl GroupSpec {
    /// Creates an empty group.
    ///
    /// # Errors
    ///
    /// Returns an identifier or name validation error when the group ID or
    /// display name is invalid.
    pub fn new(id: &str, name: &str) -> Result<Self, ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "group" });
        }
        Ok(Self {
            id: identifier(id, "group id")?,
            name: name.to_owned(),
            items: Vec::new(),
            item_ids: HashMap::new(),
        })
    }

    /// Returns the stable group ID.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the group display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns child items in compositor order.
    #[must_use]
    pub fn items(&self) -> &[SceneItemSpec] {
        &self.items
    }

    pub(crate) fn items_mut(&mut self) -> &mut [SceneItemSpec] {
        &mut self.items
    }

    /// Replaces the group display name after validating that it is non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] when `name` is empty or whitespace.
    pub fn set_name(&mut self, name: &str) -> Result<(), ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "group" });
        }
        name.clone_into(&mut self.name);
        Ok(())
    }

    /// Appends a child item while rejecting duplicate IDs within the group.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSceneItem`] for a repeated child ID.
    pub fn add_item(&mut self, item: SceneItemSpec) -> Result<(), ProjectError> {
        if self.item_ids.contains_key(item.id()) {
            return Err(ProjectError::DuplicateSceneItem(item.id().clone()));
        }
        let index = self.items.len();
        self.item_ids.insert(item.id().clone(), index);
        self.items.push(item);
        Ok(())
    }

    /// Returns whether a child item with `id` exists.
    #[must_use]
    pub fn has_item<Q>(&self, id: &Q) -> bool
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.item_ids.contains_key(id)
    }

    /// Finds a mutable child item by its group-local ID.
    pub(crate) fn item_mut(&mut self, id: &Identifier) -> Option<&mut SceneItemSpec> {
        let index = *self.item_ids.get(id)?;
        self.items.get_mut(index)
    }

    /// Removes one child item and returns it.
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

    /// Moves one child item to an existing order position.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownSceneItem`] for an unknown child ID or
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

    /// Returns whether any descendant references the source ID.
    #[must_use]
    pub fn has_source<Q>(&self, source_id: &Q) -> bool
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.items.iter().any(|item| {
            (item.is_source() && item.source_id().borrow() == source_id)
                || item
                    .group()
                    .is_some_and(|group| group.has_source(source_id))
        })
    }
}

pub(crate) fn group_mut_at<'a>(
    items: &'a mut [SceneItemSpec],
    path: &[Identifier],
) -> Option<&'a mut GroupSpec> {
    let (head, tail) = path.split_first()?;
    let item = items.iter_mut().find(|item| item.id() == head)?;
    let group = item.group_mut()?;
    if tail.is_empty() {
        Some(group)
    } else {
        group_mut_at(group.items_mut(), tail)
    }
}

pub(crate) fn group_at<'a>(
    items: &'a [SceneItemSpec],
    path: &[Identifier],
) -> Option<&'a GroupSpec> {
    let (head, tail) = path.split_first()?;
    let item = items.iter().find(|item| item.id() == head)?;
    let group = item.group()?;
    if tail.is_empty() {
        Some(group)
    } else {
        group_at(group.items(), tail)
    }
}

pub(crate) fn item_references_scene(item: &SceneItemSpec, scene_id: &Identifier) -> bool {
    item.scene_id().is_some_and(|target| target == scene_id)
        || item.group().is_some_and(|group| {
            group
                .items()
                .iter()
                .any(|child| item_references_scene(child, scene_id))
        })
}

/// An ordered scene collection entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneSpec {
    pub(crate) id: Identifier,
    pub(crate) name: String,
    /// Optional transition policy applied when this scene is taken to program.
    /// `None` inherits the desktop's current transition selection.
    pub(crate) transition_override: Option<TransitionSpec>,
    /// Optional persistent Stinger resource used by the scene transition.
    /// The decoded clip is resolved outside project state on a worker.
    pub(crate) stinger_override: Option<StingerSpec>,
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
            transition_override: None,
            stinger_override: None,
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

    /// Returns the optional transition policy used when this scene is taken.
    #[must_use]
    pub const fn transition_override(&self) -> Option<TransitionSpec> {
        self.transition_override
    }

    /// Replaces the optional transition policy used when this scene is taken.
    pub fn set_transition_override(&mut self, transition: Option<TransitionSpec>) {
        self.transition_override = transition;
    }

    /// Returns the optional persistent Stinger resource used by this scene.
    #[must_use]
    pub fn stinger_override(&self) -> Option<&StingerSpec> {
        self.stinger_override.as_ref()
    }

    /// Replaces the optional persistent Stinger resource used by this scene.
    pub fn set_stinger_override(&mut self, stinger: Option<StingerSpec>) {
        self.stinger_override = stinger;
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
        self.items.iter().any(|item| {
            (item.is_source() && item.source_id().borrow() == source_id)
                || item
                    .group()
                    .is_some_and(|group| group.has_source(source_id))
        })
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
