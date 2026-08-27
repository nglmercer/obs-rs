use obs_rs_media::{FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::Identifier;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashSet},
    sync::OnceLock,
};

use super::{error::ProjectError, validation::identifier};

mod scene;

pub(crate) use scene::{group_at, group_mut_at, item_references_scene, MAX_GROUP_NESTING_DEPTH};
pub use scene::{
    FlattenedSceneItem, GroupSpec, SceneItemSpec, SceneSpec, SourceFilterCategory,
    SourceFilterSpec, SourceSpec,
};

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
    pub(crate) scene_order: Vec<Identifier>,
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
            scene_order: Vec::new(),
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
        self.scene_order.push(scene.id().clone());
        self.scenes.insert(scene.id().clone(), scene);
        Ok(())
    }

    /// Flattens one visible scene graph into source references for a runtime
    /// adapter, preserving draw order and composing axis-aligned nested
    /// transforms.
    ///
    /// Direct source transforms retain the full media transform model. Only a
    /// transform that crosses a nested-scene boundary is restricted to the
    /// axis-aligned scale/translation/opacity/mirroring subset; unsupported
    /// transforms fail explicitly instead of being approximated.
    ///
    /// # Errors
    ///
    /// Returns an unknown-scene/source, cycle, media, unsupported nested
    /// transform, or excessive-group-depth error when the graph cannot be
    /// represented safely.
    pub fn flatten_scene_items(
        &self,
        scene: &str,
    ) -> Result<Vec<FlattenedSceneItem>, ProjectError> {
        let scene_id = identifier(scene, "scene id")?;
        let mut output = Vec::new();
        self.flatten_scene_items_inner(
            &scene_id,
            FrameTransform::IDENTITY,
            &mut Vec::new(),
            &mut Vec::new(),
            0,
            &mut output,
        )?;
        Ok(output)
    }

    /// Returns whether another scene item references this scene.
    #[must_use]
    pub fn scene_in_use(&self, id: &Identifier) -> bool {
        self.scenes.values().any(|scene| {
            scene
                .items()
                .iter()
                .any(|item| item_references_scene(item, id))
        })
    }

    fn flatten_scene_items_inner(
        &self,
        scene_id: &Identifier,
        parent_transform: FrameTransform,
        stack: &mut Vec<Identifier>,
        path: &mut Vec<Identifier>,
        group_depth: usize,
        output: &mut Vec<FlattenedSceneItem>,
    ) -> Result<(), ProjectError> {
        if stack.iter().any(|current| current == scene_id) {
            return Err(ProjectError::CircularSceneReference(scene_id.clone()));
        }
        let scene = self
            .scene(scene_id)
            .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
        stack.push(scene_id.clone());
        self.flatten_items(
            scene.items(),
            parent_transform,
            stack,
            path,
            group_depth,
            output,
        )?;
        stack.pop();
        Ok(())
    }

    fn flatten_items(
        &self,
        items: &[SceneItemSpec],
        parent_transform: FrameTransform,
        stack: &mut Vec<Identifier>,
        path: &mut Vec<Identifier>,
        group_depth: usize,
        output: &mut Vec<FlattenedSceneItem>,
    ) -> Result<(), ProjectError> {
        for item in items.iter().filter(|item| item.visible()) {
            let transform = if parent_transform == FrameTransform::IDENTITY {
                item.transform()
            } else {
                item.transform()
                    .compose_axis_aligned(
                        parent_transform,
                        self.video_format.width(),
                        self.video_format.height(),
                    )
                    .map_err(|_| ProjectError::UnsupportedNestedSceneTransform(item.id().clone()))?
            };
            path.push(item.id().clone());
            let result = if let Some(child_scene) = item.scene_id() {
                if transform.is_cropped() || transform.is_rotated() {
                    return Err(ProjectError::UnsupportedNestedSceneTransform(
                        item.id().clone(),
                    ));
                }
                self.flatten_scene_items_inner(
                    child_scene,
                    transform,
                    stack,
                    path,
                    group_depth,
                    output,
                )
            } else if let Some(group) = item.group() {
                if transform.is_cropped() || transform.is_rotated() {
                    return Err(ProjectError::UnsupportedNestedSceneTransform(
                        item.id().clone(),
                    ));
                }
                if group_depth >= MAX_GROUP_NESTING_DEPTH {
                    return Err(ProjectError::GroupNestingTooDeep(MAX_GROUP_NESTING_DEPTH));
                }
                self.flatten_items(
                    group.items(),
                    transform,
                    stack,
                    path,
                    group_depth.saturating_add(1),
                    output,
                )
            } else {
                if !self.has_source(item.source_id()) {
                    path.pop();
                    return Err(ProjectError::UnknownSource(item.source_id().clone()));
                }
                output.push(FlattenedSceneItem {
                    item_id: path
                        .iter()
                        .map(Identifier::as_str)
                        .collect::<Vec<_>>()
                        .join("/"),
                    source_id: item.source_id().clone(),
                    transform,
                });
                Ok(())
            };
            path.pop();
            result?;
        }
        Ok(())
    }

    /// Removes a scene by ID and returns its definition.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownScene`] when the scene is absent.
    pub fn remove_scene(&mut self, id: &Identifier) -> Result<SceneSpec, ProjectError> {
        if self.scene_in_use(id) {
            return Err(ProjectError::SceneInUse(id.clone()));
        }
        let scene = self
            .scenes
            .remove(id)
            .ok_or_else(|| ProjectError::UnknownScene(id.clone()))?;
        self.scene_order.retain(|scene_id| scene_id != id);
        Ok(scene)
    }

    /// Moves one scene to an existing position in the profile's scene order.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownScene`] for an unknown scene ID or
    /// [`ProjectError::InvalidSceneOrder`] for an out-of-range position.
    pub fn move_scene(&mut self, id: &Identifier, target_index: usize) -> Result<(), ProjectError> {
        let current_index = self
            .scene_order
            .iter()
            .position(|scene_id| scene_id == id)
            .ok_or_else(|| ProjectError::UnknownScene(id.clone()))?;
        if target_index >= self.scene_order.len() {
            return Err(ProjectError::InvalidSceneOrder {
                index: target_index,
            });
        }
        let scene_id = self.scene_order.remove(current_index);
        self.scene_order.insert(target_index, scene_id);
        Ok(())
    }

    /// Replaces the persisted scene order after validating every scene ID.
    pub(crate) fn restore_scene_order(
        &mut self,
        order: Vec<Identifier>,
    ) -> Result<(), ProjectError> {
        if order.len() != self.scenes.len() {
            return Err(ProjectError::InvalidSceneOrder { index: order.len() });
        }
        let mut seen = HashSet::with_capacity(order.len());
        for (index, scene_id) in order.iter().enumerate() {
            if !self.scenes.contains_key(scene_id) || !seen.insert(scene_id.clone()) {
                return Err(ProjectError::InvalidSceneOrder { index });
            }
        }
        self.scene_order = order;
        Ok(())
    }

    /// Returns the persistent profile scene order as stable IDs.
    pub fn scene_order(&self) -> impl Iterator<Item = &Identifier> {
        self.scene_order.iter()
    }

    /// Returns scenes in their persistent user order.
    pub fn scenes(&self) -> impl Iterator<Item = &SceneSpec> {
        self.scene_order
            .iter()
            .filter_map(|scene_id| self.scenes.get(scene_id))
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
