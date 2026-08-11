use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::Identifier;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap, HashSet},
    sync::OnceLock,
};

use super::{error::ProjectError, validation::identifier};
/// A source definition stored in a scene collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpec {
    pub(crate) id: Identifier,
    pub(crate) kind: Identifier,
    pub(crate) name: String,
    pub(crate) settings: Config,
    pub(crate) transform: FrameTransform,
    pub(crate) filters: Vec<FrameFilter>,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
}

impl SourceSpec {
    /// Creates a source definition with an identity transform and no filters.
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
            transform: FrameTransform::IDENTITY,
            filters: Vec::new(),
            visible: true,
            locked: false,
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

    /// Returns source settings.
    #[must_use]
    pub const fn settings(&self) -> &Config {
        &self.settings
    }

    /// Replaces source settings.
    pub fn set_settings(&mut self, settings: Config) {
        self.settings = settings;
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

    /// Returns filters in application order.
    #[must_use]
    pub fn filters(&self) -> &[FrameFilter] {
        &self.filters
    }

    /// Appends a filter to the source's ordered filter chain.
    pub fn add_filter(&mut self, filter: FrameFilter) {
        self.filters.push(filter);
    }

    /// Returns whether this source participates in scene composition.
    #[must_use]
    pub const fn visible(&self) -> bool {
        self.visible
    }

    /// Sets whether this source participates in scene composition.
    pub const fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Returns whether the source is protected from editing in a desktop UI.
    #[must_use]
    pub const fn locked(&self) -> bool {
        self.locked
    }

    /// Sets whether the source is protected from editing in a desktop UI.
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
    pub(crate) sources: Vec<SourceSpec>,
    /// O(1) source lookup and membership mirror. Values are indices into the
    /// ordered `sources` vector, so keyed mutations do not scan the scene.
    pub(crate) source_ids: HashMap<Identifier, usize>,
}

impl SceneSpec {
    /// Creates an empty scene definition.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when the scene ID or name is invalid.
    pub fn new(id: &str, name: &str) -> Result<Self, ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "scene" });
        }
        Ok(Self {
            id: identifier(id, "scene id")?,
            name: name.to_owned(),
            sources: Vec::new(),
            source_ids: HashMap::new(),
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

    /// Returns sources in compositor order.
    #[must_use]
    pub fn sources(&self) -> &[SourceSpec] {
        &self.sources
    }

    /// Replaces the scene's display name after validating that it is non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidName`] when the new name is empty.
    pub fn set_name(&mut self, name: &str) -> Result<(), ProjectError> {
        if name.trim().is_empty() {
            return Err(ProjectError::InvalidName { kind: "scene" });
        }
        name.clone_into(&mut self.name);
        Ok(())
    }

    /// Appends a source definition while rejecting duplicate IDs.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSource`] when the ID is already present.
    pub fn add_source(&mut self, source: SourceSpec) -> Result<(), ProjectError> {
        if self.source_ids.contains_key(source.id()) {
            return Err(ProjectError::DuplicateSource(source.id().clone()));
        }
        let index = self.sources.len();
        self.source_ids.insert(source.id().clone(), index);
        self.sources.push(source);
        Ok(())
    }

    /// Appends several source definitions, rejecting duplicates atomically.
    ///
    /// Every ID is checked before anything is inserted, so a rejected batch
    /// leaves the scene unchanged. Adding N sources costs O(N) rather than the
    /// O(N^2) that N separate [`SceneSpec::add_source`] calls would.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateSource`] when an ID is already present
    /// or is repeated within `sources`.
    pub fn add_sources(
        &mut self,
        sources: impl IntoIterator<Item = SourceSpec>,
    ) -> Result<(), ProjectError> {
        let incoming: Vec<SourceSpec> = sources.into_iter().collect();
        let mut candidate = HashSet::with_capacity(incoming.len());
        for source in &incoming {
            if self.source_ids.contains_key(source.id()) || !candidate.insert(source.id()) {
                return Err(ProjectError::DuplicateSource(source.id().clone()));
            }
        }
        self.sources.reserve(incoming.len());
        for source in incoming {
            let index = self.sources.len();
            self.source_ids.insert(source.id().clone(), index);
            self.sources.push(source);
        }
        Ok(())
    }

    /// Returns whether this scene contains `id`, in constant time.
    #[must_use]
    pub fn has_source<Q>(&self, id: &Q) -> bool
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.source_ids.contains_key(id)
    }

    /// Finds a mutable source by project-local ID.
    pub fn source_mut(&mut self, id: &Identifier) -> Option<&mut SourceSpec> {
        let index = *self.source_ids.get(id)?;
        self.sources.get_mut(index)
    }

    /// Finds an immutable source by an owned or borrowed project-local ID.
    #[must_use]
    pub fn source<Q>(&self, id: &Q) -> Option<&SourceSpec>
    where
        Identifier: Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        let index = *self.source_ids.get(id)?;
        self.sources.get(index)
    }

    /// Removes one source item and returns its definition.
    pub fn remove_source(&mut self, id: &Identifier) -> Option<SourceSpec> {
        let index = self.source_ids.remove(id)?;
        let source = self.sources.remove(index);
        for (index, source) in self.sources.iter().enumerate().skip(index) {
            if let Some(entry) = self.source_ids.get_mut(source.id()) {
                *entry = index;
            }
        }
        Some(source)
    }

    /// Moves one source item to an existing scene order position.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::UnknownSource`] when the source is absent or
    /// [`ProjectError::InvalidSourceOrder`] when the destination is out of range.
    pub fn move_source(
        &mut self,
        id: &Identifier,
        target_index: usize,
    ) -> Result<(), ProjectError> {
        let current_index = self
            .source_ids
            .get(id)
            .copied()
            .ok_or_else(|| ProjectError::UnknownSource(id.clone()))?;
        if target_index >= self.sources.len() {
            return Err(ProjectError::InvalidSourceOrder {
                index: target_index,
            });
        }
        let source = self.sources.remove(current_index);
        self.sources.insert(target_index, source);
        let first = current_index.min(target_index);
        for (index, source) in self.sources.iter().enumerate().skip(first) {
            if let Some(entry) = self.source_ids.get_mut(source.id()) {
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

/// One named profile containing video settings and ordered scenes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    pub(crate) id: Identifier,
    pub(crate) name: String,
    pub(crate) video_format: VideoFormat,
    pub(crate) render_backend: RenderBackendPreference,
    pub(crate) output_kind: OutputProfileKind,
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
