//! Rust-owned project state and deterministic persistence for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use obs_rs_config::{Config, ConfigError};
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, MediaError, VideoFormat};
use obs_rs_util::{Identifier, IdentifierError};

const MAGIC: &str = "OBSRPROJECT1";
/// Maximum serialized project size accepted by the parser.
pub const MAX_PROJECT_BYTES: usize = 1_048_576;

/// A source definition stored in a scene collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpec {
    id: Identifier,
    kind: Identifier,
    name: String,
    settings: Config,
    transform: FrameTransform,
    filters: Vec<FrameFilter>,
    visible: bool,
    locked: bool,
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
    id: Identifier,
    name: String,
    sources: Vec<SourceSpec>,
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
        if self.sources.iter().any(|item| item.id() == source.id()) {
            return Err(ProjectError::DuplicateSource(source.id().clone()));
        }
        self.sources.push(source);
        Ok(())
    }

    /// Finds a mutable source by project-local ID.
    pub fn source_mut(&mut self, id: &Identifier) -> Option<&mut SourceSpec> {
        self.sources.iter_mut().find(|source| source.id() == id)
    }

    /// Removes one source item and returns its definition.
    pub fn remove_source(&mut self, id: &Identifier) -> Option<SourceSpec> {
        let index = self.sources.iter().position(|source| source.id() == id)?;
        Some(self.sources.remove(index))
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
            .sources
            .iter()
            .position(|source| source.id() == id)
            .ok_or_else(|| ProjectError::UnknownSource(id.clone()))?;
        if target_index >= self.sources.len() {
            return Err(ProjectError::InvalidSourceOrder {
                index: target_index,
            });
        }
        let source = self.sources.remove(current_index);
        self.sources.insert(target_index, source);
        Ok(())
    }
}

/// One named profile containing video settings and ordered scenes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    id: Identifier,
    name: String,
    video_format: VideoFormat,
    scenes: BTreeMap<Identifier, SceneSpec>,
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

    /// Returns a mutable scene by ID.
    pub fn scene_mut(&mut self, id: &Identifier) -> Option<&mut SceneSpec> {
        self.scenes.get_mut(id)
    }
}

/// A complete project document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    title: String,
    active_profile: Identifier,
    profiles: BTreeMap<Identifier, Profile>,
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
        let active_profile =
            Identifier::new("default").map_err(|error| ProjectError::InvalidIdentifier {
                kind: "default profile id",
                error,
            })?;
        Ok(Self {
            title: title.to_owned(),
            active_profile,
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

    /// Returns a mutable profile by ID.
    pub fn profile_mut(&mut self, id: &Identifier) -> Option<&mut Profile> {
        self.profiles.get_mut(id)
    }

    /// Serializes the project into a deterministic, escaped line format.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut document = String::new();
        document.push_str(MAGIC);
        document.push('\n');
        document.push_str("project|");
        document.push_str(&escape(&self.title));
        document.push('|');
        document.push_str(self.active_profile.as_str());
        document.push('\n');
        for profile in self.profiles.values() {
            let format = profile.video_format;
            document.push_str("profile|");
            document.push_str(profile.id.as_str());
            document.push('|');
            document.push_str(&escape(&profile.name));
            document.push('|');
            document.push_str(&format.width().to_string());
            document.push('|');
            document.push_str(&format.height().to_string());
            document.push('|');
            document.push_str(&format.frame_rate().numerator().to_string());
            document.push('|');
            document.push_str(&format.frame_rate().denominator().to_string());
            document.push('\n');

            for scene in profile.scenes.values() {
                document.push_str("scene|");
                document.push_str(profile.id.as_str());
                document.push('|');
                document.push_str(scene.id.as_str());
                document.push('|');
                document.push_str(&escape(&scene.name));
                document.push('\n');
                for source in &scene.sources {
                    document.push_str("source|");
                    document.push_str(profile.id.as_str());
                    document.push('|');
                    document.push_str(scene.id.as_str());
                    document.push('|');
                    document.push_str(source.id.as_str());
                    document.push('|');
                    document.push_str(source.kind.as_str());
                    document.push('|');
                    document.push_str(&escape(&source.name));
                    document.push('|');
                    document.push_str(&escape(&source.settings.serialize()));
                    document.push('|');
                    append_transform(&mut document, source.transform);
                    document.push('|');
                    document.push_str(&serialize_filters(&source.filters));
                    document.push('|');
                    document.push_str(if source.visible { "1" } else { "0" });
                    document.push('|');
                    document.push_str(if source.locked { "1" } else { "0" });
                    document.push('\n');
                }
            }
        }
        document
    }

    /// Parses a serialized project document.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] for malformed lines, duplicate objects, invalid
    /// settings, invalid media values, or unknown references.
    pub fn parse(document: &str) -> Result<Self, ProjectError> {
        if document.len() > MAX_PROJECT_BYTES {
            return Err(ProjectError::DocumentTooLarge);
        }
        let mut lines = document.lines();
        if lines.next() != Some(MAGIC) {
            return Err(ProjectError::InvalidDocument {
                line: 1,
                reason: "invalid project header".to_owned(),
            });
        }

        let project_line = lines.next().ok_or_else(|| ProjectError::InvalidDocument {
            line: 2,
            reason: "missing project record".to_owned(),
        })?;
        let project_fields = fields(project_line, 2, "project", 3)?;
        let title = decode(project_fields[1], 2)?;
        let mut project = Self::new(&title)?;
        project.active_profile = identifier(project_fields[2], "active profile id")?;

        for (index, line) in lines.enumerate() {
            let line_number = index + 3;
            if line.trim().is_empty() {
                return Err(ProjectError::InvalidDocument {
                    line: line_number,
                    reason: "blank lines are not allowed".to_owned(),
                });
            }
            let kind = line.split('|').next().unwrap_or_default();
            match kind {
                "profile" => parse_profile(&mut project, line, line_number)?,
                "scene" => parse_scene(&mut project, line, line_number)?,
                "source" => parse_source(&mut project, line, line_number)?,
                _ => {
                    return Err(ProjectError::InvalidDocument {
                        line: line_number,
                        reason: format!("unknown record type: {kind}"),
                    });
                }
            }
        }
        if !project.profiles.is_empty() && !project.profiles.contains_key(&project.active_profile) {
            return Err(ProjectError::UnknownProfile(project.active_profile));
        }
        Ok(project)
    }
}

/// Commands that mutate project state through one validated path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCommand {
    /// Adds a profile.
    AddProfile(Profile),
    /// Selects an existing profile.
    SetActiveProfile { id: String },
    /// Adds a scene to a profile.
    AddScene { profile: String, scene: SceneSpec },
    /// Adds a source to a profile scene.
    AddSource {
        profile: String,
        scene: String,
        source: SourceSpec,
    },
    /// Removes a scene from a profile.
    RemoveScene { profile: String, scene: String },
    /// Renames a scene in a profile.
    SetSceneName {
        profile: String,
        scene: String,
        name: String,
    },
    /// Replaces one source's validated settings document.
    SetSourceSettings {
        profile: String,
        scene: String,
        source: String,
        settings: Config,
    },
    /// Replaces one source transform.
    SetSourceTransform {
        profile: String,
        scene: String,
        source: String,
        transform: FrameTransform,
    },
    /// Appends one source filter.
    AddSourceFilter {
        profile: String,
        scene: String,
        source: String,
        filter: FrameFilter,
    },
    /// Reorders one source item within a scene.
    MoveSource {
        profile: String,
        scene: String,
        source: String,
        target_index: usize,
    },
    /// Removes one source item from a scene.
    RemoveSource {
        profile: String,
        scene: String,
        source: String,
    },
    /// Replaces the ordered filter chain for one source.
    SetSourceFilters {
        profile: String,
        scene: String,
        source: String,
        filters: Vec<FrameFilter>,
    },
    /// Changes whether one source participates in scene composition.
    SetSourceVisibility {
        profile: String,
        scene: String,
        source: String,
        visible: bool,
    },
    /// Changes whether one source is protected from desktop editing.
    SetSourceLocked {
        profile: String,
        scene: String,
        source: String,
        locked: bool,
    },
}

impl Project {
    /// Applies one command atomically from the caller's perspective.
    ///
    /// # Errors
    ///
    /// Returns a validation or reference error without intentionally applying a
    /// partial command.
    pub fn apply(&mut self, command: ProjectCommand) -> Result<(), ProjectError> {
        match command {
            ProjectCommand::AddProfile(profile) => self.add_profile(profile),
            ProjectCommand::SetActiveProfile { id } => self.set_active_profile(&id),
            ProjectCommand::AddScene { profile, scene } => {
                let profile_id = identifier(&profile, "profile id")?;
                let profile = self
                    .profile_mut(&profile_id)
                    .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
                profile.add_scene(scene)
            }
            ProjectCommand::AddSource {
                profile,
                scene,
                source,
            } => {
                let profile_id = identifier(&profile, "profile id")?;
                let profile = self
                    .profile_mut(&profile_id)
                    .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
                let scene_id = identifier(&scene, "scene id")?;
                let scene = profile
                    .scene_mut(&scene_id)
                    .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
                scene.add_source(source)
            }
            ProjectCommand::RemoveScene { profile, scene } => {
                let profile_id = identifier(&profile, "profile id")?;
                let profile = self
                    .profile_mut(&profile_id)
                    .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
                let scene_id = identifier(&scene, "scene id")?;
                profile.remove_scene(&scene_id).map(|_| ())
            }
            ProjectCommand::SetSceneName {
                profile,
                scene,
                name,
            } => {
                let profile_id = identifier(&profile, "profile id")?;
                let profile = self
                    .profile_mut(&profile_id)
                    .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
                let scene_id = identifier(&scene, "scene id")?;
                let scene = profile
                    .scene_mut(&scene_id)
                    .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
                scene.set_name(&name)
            }
            ProjectCommand::SetSourceSettings {
                profile,
                scene,
                source,
                settings,
            } => set_source_settings(self, &profile, &scene, &source, settings),
            ProjectCommand::SetSourceTransform {
                profile,
                scene,
                source,
                transform,
            } => set_source_transform(self, &profile, &scene, &source, transform),
            ProjectCommand::AddSourceFilter {
                profile,
                scene,
                source,
                filter,
            } => add_source_filter(self, &profile, &scene, &source, filter),
            ProjectCommand::SetSourceFilters {
                profile,
                scene,
                source,
                filters,
            } => set_source_filters(self, &profile, &scene, &source, filters),
            ProjectCommand::SetSourceVisibility {
                profile,
                scene,
                source,
                visible,
            } => set_source_visibility(self, &profile, &scene, &source, visible),
            ProjectCommand::SetSourceLocked {
                profile,
                scene,
                source,
                locked,
            } => set_source_locked(self, &profile, &scene, &source, locked),
            ProjectCommand::MoveSource {
                profile,
                scene,
                source,
                target_index,
            } => move_source(self, &profile, &scene, &source, target_index),
            ProjectCommand::RemoveSource {
                profile,
                scene,
                source,
            } => remove_source(self, &profile, &scene, &source),
        }
    }
}

/// Mutable project controller that tracks unsaved changes.
pub struct ProjectSession {
    project: Project,
    dirty: bool,
}

impl ProjectSession {
    /// Opens a clean session around a project.
    #[must_use]
    pub const fn new(project: Project) -> Self {
        Self {
            project,
            dirty: false,
        }
    }

    /// Applies a command and marks the project dirty only after success.
    ///
    /// # Errors
    ///
    /// Returns the project validation error and leaves the dirty flag unchanged.
    pub fn dispatch(&mut self, command: ProjectCommand) -> Result<(), ProjectError> {
        self.project.apply(command)?;
        self.dirty = true;
        Ok(())
    }

    /// Returns the current project.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    /// Returns whether commands have changed the persisted state.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Serializes and marks the session clean, representing a successful save.
    #[must_use]
    pub fn save(&mut self) -> String {
        let document = self.document();
        self.mark_saved();
        document
    }

    /// Serializes the current state without changing dirty status.
    #[must_use]
    pub fn document(&self) -> String {
        self.project.serialize()
    }

    /// Marks the session clean after an external persistence operation succeeds.
    pub const fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Marks the session dirty after recovering an unswitched temporary file.
    pub const fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// Crash-safe project-file persistence using temporary-file plus rename.
pub struct ProjectFileStore {
    final_path: PathBuf,
    temp_path: PathBuf,
}

impl ProjectFileStore {
    /// Creates a file store with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidPaths`] when a path is empty or both paths
    /// are equal.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
    ) -> Result<Self, ProjectError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(ProjectError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(ProjectError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            final_path,
            temp_path,
        })
    }

    /// Saves a session without marking it clean until the rename succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] for filesystem failures. The final path is
    /// left untouched when writing or synchronization fails.
    pub fn save(&self, session: &mut ProjectSession) -> Result<usize, ProjectError> {
        let document = session.document();
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| ProjectError::Io {
                    operation: "open project temporary file",
                    message: error.to_string(),
                })?;
            file.write_all(document.as_bytes())
                .map_err(|error| ProjectError::Io {
                    operation: "write project temporary file",
                    message: error.to_string(),
                })?;
            file.sync_all().map_err(|error| ProjectError::Io {
                operation: "sync project temporary file",
                message: error.to_string(),
            })?;
            fs::rename(&self.temp_path, &self.final_path).map_err(|error| ProjectError::Io {
                operation: "rename project file",
                message: error.to_string(),
            })?;
            Ok::<(), ProjectError>(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }
        session.mark_saved();
        Ok(document.len())
    }

    /// Loads and parses the final project file.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] for read failures or a parser error for an
    /// invalid document.
    pub fn load(&self) -> Result<Project, ProjectError> {
        let document = fs::read_to_string(&self.final_path).map_err(|error| ProjectError::Io {
            operation: "read project file",
            message: error.to_string(),
        })?;
        Project::parse(&document)
    }

    /// Reads a valid, unswitched temporary project after an interrupted save.
    ///
    /// The temporary file is never removed by this read. The caller can decide
    /// whether to recover it into memory and publish it through a later save.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] when the temporary file exists but cannot be
    /// read, or a parser error when its contents are incomplete or invalid.
    pub fn recover(&self) -> Result<Option<Project>, ProjectError> {
        if !self.temp_path.exists() {
            return Ok(None);
        }
        let document = fs::read_to_string(&self.temp_path).map_err(|error| ProjectError::Io {
            operation: "read project recovery file",
            message: error.to_string(),
        })?;
        Project::parse(&document).map(Some)
    }

    /// Returns whether an interrupted-save temporary file is present.
    #[must_use]
    pub fn recovery_available(&self) -> bool {
        self.temp_path.is_file()
    }

    /// Returns the final project path.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary project path.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }
}

fn scene_mut<'a>(
    project: &'a mut Project,
    profile: &str,
    scene: &str,
) -> Result<&'a mut SceneSpec, ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene_id = identifier(scene, "scene id")?;
    profile
        .scene_mut(&scene_id)
        .ok_or(ProjectError::UnknownScene(scene_id))
}

fn source_mut<'a>(
    project: &'a mut Project,
    profile: &str,
    scene: &str,
    source: &str,
) -> Result<&'a mut SourceSpec, ProjectError> {
    let source_id = source_id(source)?;
    let scene = scene_mut(project, profile, scene)?;
    scene
        .source_mut(&source_id)
        .ok_or(ProjectError::UnknownSource(source_id))
}

fn set_source_settings(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    settings: Config,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.set_settings(settings);
    Ok(())
}

fn set_source_transform(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    transform: FrameTransform,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.set_transform(transform);
    Ok(())
}

fn add_source_filter(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filter: FrameFilter,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.add_filter(filter);
    Ok(())
}

fn set_source_filters(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filters: Vec<FrameFilter>,
) -> Result<(), ProjectError> {
    let source = source_mut(project, profile, scene, source)?;
    source.filters.clear();
    for filter in filters {
        source.add_filter(filter);
    }
    Ok(())
}

fn set_source_visibility(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    visible: bool,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.set_visible(visible);
    Ok(())
}

fn set_source_locked(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    locked: bool,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.set_locked(locked);
    Ok(())
}

fn move_source(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let source = source_id(source)?;
    scene_mut(project, profile, scene)?.move_source(&source, target_index)
}

fn remove_source(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
) -> Result<(), ProjectError> {
    let source = source_id(source)?;
    scene_mut(project, profile, scene)?
        .remove_source(&source)
        .map(|_| ())
        .ok_or(ProjectError::UnknownSource(source))
}

fn source_id(input: &str) -> Result<Identifier, ProjectError> {
    identifier(input, "source id")
}

fn identifier(input: &str, kind: &'static str) -> Result<Identifier, ProjectError> {
    Identifier::new(input).map_err(|error| ProjectError::InvalidIdentifier { kind, error })
}

fn fields<'a>(
    line: &'a str,
    line_number: usize,
    expected_kind: &str,
    expected_count: usize,
) -> Result<Vec<&'a str>, ProjectError> {
    let values = line.split('|').collect::<Vec<_>>();
    if values.len() != expected_count || values[0] != expected_kind {
        return Err(ProjectError::InvalidDocument {
            line: line_number,
            reason: format!("expected {expected_kind} record with {expected_count} fields"),
        });
    }
    Ok(values)
}

fn parse_profile(
    project: &mut Project,
    line: &str,
    line_number: usize,
) -> Result<(), ProjectError> {
    let values = fields(line, line_number, "profile", 7)?;
    let id = values[1];
    let name = decode(values[2], line_number)?;
    let width = number(values[3], line_number, "profile width")?;
    let height = number(values[4], line_number, "profile height")?;
    let numerator = number(values[5], line_number, "profile frame-rate numerator")?;
    let denominator = number(values[6], line_number, "profile frame-rate denominator")?;
    let rate = FrameRate::new(numerator, denominator).map_err(ProjectError::Media)?;
    let format = VideoFormat::new(width, height, rate).map_err(ProjectError::Media)?;
    project.add_profile(Profile::new(id, &name, format)?)
}

fn parse_scene(project: &mut Project, line: &str, line_number: usize) -> Result<(), ProjectError> {
    let values = fields(line, line_number, "scene", 4)?;
    let profile_id = identifier(values[1], "profile id")?;
    let scene = SceneSpec::new(values[2], &decode(values[3], line_number)?)?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    profile.add_scene(scene)
}

fn parse_source(project: &mut Project, line: &str, line_number: usize) -> Result<(), ProjectError> {
    let values = line.split('|').collect::<Vec<_>>();
    if (values.len() != 9 && values.len() != 11) || values.first() != Some(&"source") {
        return Err(ProjectError::InvalidDocument {
            line: line_number,
            reason: "expected source record with 9 or 11 fields".to_owned(),
        });
    }
    let profile_id = identifier(values[1], "profile id")?;
    let scene_id = identifier(values[2], "scene id")?;
    let name = decode(values[5], line_number)?;
    let settings_text = decode(values[6], line_number)?;
    let settings = Config::parse(&settings_text).map_err(ProjectError::Config)?;
    let transform = parse_transform(values[7], line_number)?;
    let filters = parse_filters(values[8], line_number)?;
    let mut source = SourceSpec::new(values[3], values[4], &name, settings)?;
    source.set_transform(transform);
    for filter in filters {
        source.add_filter(filter);
    }
    if values.len() == 11 {
        source.set_visible(parse_flag(values[9], line_number, "source visibility")?);
        source.set_locked(parse_flag(values[10], line_number, "source lock")?);
    }
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    scene.add_source(source)
}

fn parse_flag(value: &str, line: usize, field: &'static str) -> Result<bool, ProjectError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(ProjectError::InvalidDocument {
            line,
            reason: format!("invalid {field}; expected 0 or 1"),
        }),
    }
}

fn number<T>(value: &str, line: usize, field: &'static str) -> Result<T, ProjectError>
where
    T: std::str::FromStr,
{
    value.parse().map_err(|_| ProjectError::InvalidDocument {
        line,
        reason: format!("invalid {field}"),
    })
}

fn append_transform(document: &mut String, transform: FrameTransform) {
    document.push_str(&transform.scale_x_milli().to_string());
    document.push(',');
    document.push_str(&transform.scale_y_milli().to_string());
    document.push(',');
    document.push_str(&transform.translate_x().to_string());
    document.push(',');
    document.push_str(&transform.translate_y().to_string());
    document.push(',');
    document.push_str(if transform.flip_x() { "1" } else { "0" });
    document.push(',');
    document.push_str(if transform.flip_y() { "1" } else { "0" });
    document.push(',');
    document.push_str(&transform.opacity().to_string());
}

fn parse_transform(value: &str, line: usize) -> Result<FrameTransform, ProjectError> {
    let values = value.split(',').collect::<Vec<_>>();
    if values.len() != 7 {
        return Err(ProjectError::InvalidDocument {
            line,
            reason: "invalid transform field count".to_owned(),
        });
    }
    FrameTransform::new(
        number(values[0], line, "horizontal scale")?,
        number(values[1], line, "vertical scale")?,
        number(values[2], line, "horizontal translation")?,
        number(values[3], line, "vertical translation")?,
        number::<u8>(values[4], line, "horizontal flip")? != 0,
        number::<u8>(values[5], line, "vertical flip")? != 0,
        number(values[6], line, "opacity")?,
    )
    .map_err(ProjectError::Media)
}

fn serialize_filters(filters: &[FrameFilter]) -> String {
    filters
        .iter()
        .map(|filter| match filter {
            FrameFilter::Grayscale => "gray".to_owned(),
            FrameFilter::Brightness { milli } => format!("brightness:{milli}"),
            FrameFilter::Opacity(opacity) => format!("opacity:{opacity}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_filters(value: &str, line: usize) -> Result<Vec<FrameFilter>, ProjectError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|filter| {
            if filter == "gray" {
                return Ok(FrameFilter::Grayscale);
            }
            if let Some(value) = filter.strip_prefix("brightness:") {
                return Ok(FrameFilter::Brightness {
                    milli: number(value, line, "brightness")?,
                });
            }
            if let Some(value) = filter.strip_prefix("opacity:") {
                return Ok(FrameFilter::Opacity(number(value, line, "opacity")?));
            }
            Err(ProjectError::InvalidDocument {
                line,
                reason: format!("unknown filter: {filter}"),
            })
        })
        .collect()
}

fn escape(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            escaped.push(hex(byte >> 4));
            escaped.push(hex(byte & 0x0F));
        }
    }
    escaped
}

fn decode(value: &str, line: usize) -> Result<String, ProjectError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(ProjectError::InvalidDocument {
                line,
                reason: "truncated escaped value".to_owned(),
            });
        }
        let high = from_hex(bytes[index + 1]).ok_or_else(|| ProjectError::InvalidDocument {
            line,
            reason: "invalid escaped value".to_owned(),
        })?;
        let low = from_hex(bytes[index + 2]).ok_or_else(|| ProjectError::InvalidDocument {
            line,
            reason: "invalid escaped value".to_owned(),
        })?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| ProjectError::InvalidDocument {
        line,
        reason: "escaped value is not UTF-8".to_owned(),
    })
}

fn hex(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'A' + value - 10),
    }
}

fn from_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Errors raised by project state and persistence operations.
#[derive(Debug, Eq, PartialEq)]
pub enum ProjectError {
    /// The serialized document exceeds [`MAX_PROJECT_BYTES`].
    DocumentTooLarge,
    /// A project, profile, scene, or source name is empty.
    InvalidName { kind: &'static str },
    /// An identifier failed validation.
    InvalidIdentifier {
        /// Logical identifier kind.
        kind: &'static str,
        /// Underlying validation failure.
        error: IdentifierError,
    },
    /// A serialized line is malformed.
    InvalidDocument { line: usize, reason: String },
    /// Final and temporary persistence paths are invalid.
    InvalidPaths { reason: String },
    /// A project file operation failed.
    Io {
        /// Logical filesystem operation.
        operation: &'static str,
        /// Underlying operating-system message.
        message: String,
    },
    /// A source setting document is invalid.
    Config(ConfigError),
    /// A video or transform value is invalid.
    Media(MediaError),
    /// A profile ID is already present.
    DuplicateProfile(Identifier),
    /// A scene ID is already present.
    DuplicateScene(Identifier),
    /// A source ID is already present in a scene.
    DuplicateSource(Identifier),
    /// A profile ID is not present.
    UnknownProfile(Identifier),
    /// A scene ID is not present.
    UnknownScene(Identifier),
    /// A source ID is not present.
    UnknownSource(Identifier),
    /// A source move destination is outside the scene order.
    InvalidSourceOrder { index: usize },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DocumentTooLarge => formatter.write_str("project document is too large"),
            Self::InvalidName { kind } => write!(formatter, "{kind} name is empty"),
            Self::InvalidIdentifier { kind, error } => {
                write!(formatter, "invalid {kind} identifier: {error}")
            }
            Self::InvalidDocument { line, reason } => {
                write!(formatter, "invalid project document line {line}: {reason}")
            }
            Self::InvalidPaths { reason } => write!(formatter, "invalid project paths: {reason}"),
            Self::Io { operation, message } => write!(formatter, "{operation} failed: {message}"),
            Self::Config(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
            Self::DuplicateProfile(id) => write!(formatter, "profile {id} already exists"),
            Self::DuplicateScene(id) => write!(formatter, "scene {id} already exists"),
            Self::DuplicateSource(id) => write!(formatter, "source {id} already exists"),
            Self::UnknownProfile(id) => write!(formatter, "profile {id} does not exist"),
            Self::UnknownScene(id) => write!(formatter, "scene {id} does not exist"),
            Self::UnknownSource(id) => write!(formatter, "source {id} does not exist"),
            Self::InvalidSourceOrder { index } => {
                write!(formatter, "source order index {index} is out of range")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn format() -> VideoFormat {
        VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format")
    }

    fn settings() -> Config {
        let mut config = Config::new();
        config.set("color", "#102030FF").expect("color");
        config
    }

    fn project() -> Project {
        let mut project = Project::new("Studio | Demo").expect("project");
        let mut profile = Profile::new("live", "Live profile", format()).expect("profile");
        let mut scene = SceneSpec::new("main", "Main scene").expect("scene");
        let mut source = SourceSpec::new("background", "color_source", "Background", settings())
            .expect("source");
        source.set_transform(
            FrameTransform::new(1_000, 1_000, 4, -3, true, false, 220).expect("transform"),
        );
        source.add_filter(FrameFilter::Brightness { milli: 750 });
        source.add_filter(FrameFilter::Opacity(200));
        scene.add_source(source).expect("source attach");
        profile.add_scene(scene).expect("scene attach");
        project.add_profile(profile).expect("profile add");
        project
    }

    fn unique_paths(label: &str) -> (PathBuf, PathBuf) {
        use std::time::{SystemTime, UNIX_EPOCH};

        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir();
        (
            root.join(format!("obs-rs-project-{label}-{token}.txt")),
            root.join(format!("obs-rs-project-{label}-{token}.part")),
        )
    }

    #[test]
    fn project_round_trips_deterministically_with_escaped_values() {
        let project = project();
        let encoded = project.serialize();
        let decoded = Project::parse(&encoded).expect("parse project");

        assert_eq!(decoded, project);
        assert_eq!(decoded.serialize(), encoded);
        assert!(encoded.contains("Studio%20%7C%20Demo"));
    }

    #[test]
    fn parser_keeps_legacy_sources_visible_and_unlocked() {
        let encoded = project().serialize();
        let legacy = encoded
            .lines()
            .map(|line| {
                if line.starts_with("source|") {
                    line.split('|').take(9).collect::<Vec<_>>().join("|")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let decoded = Project::parse(&legacy).expect("legacy project parses");
        let source = decoded
            .profiles()
            .next()
            .and_then(|profile| profile.scenes().next())
            .and_then(|scene| scene.sources().first())
            .expect("legacy source");
        assert!(source.visible());
        assert!(!source.locked());
    }

    #[test]
    fn command_session_tracks_dirty_state_and_rejects_bad_references() {
        let mut session = ProjectSession::new(project());
        assert!(!session.is_dirty());
        let source = SourceSpec::new("foreground", "test_pattern", "Foreground", Config::new())
            .expect("source");
        session
            .dispatch(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source,
            })
            .expect("add source command");
        let mut replacement_settings = Config::new();
        replacement_settings
            .set("color", "#203040FF")
            .expect("replacement settings");
        session
            .dispatch(ProjectCommand::SetSourceSettings {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source: "background".to_owned(),
                settings: replacement_settings,
            })
            .expect("set source settings command");
        session
            .dispatch(ProjectCommand::SetSourceFilters {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source: "background".to_owned(),
                filters: vec![FrameFilter::Grayscale],
            })
            .expect("set source filters command");
        session
            .dispatch(ProjectCommand::SetSourceVisibility {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source: "background".to_owned(),
                visible: false,
            })
            .expect("set source visibility command");
        session
            .dispatch(ProjectCommand::SetSourceLocked {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source: "background".to_owned(),
                locked: true,
            })
            .expect("set source locked command");
        session
            .dispatch(ProjectCommand::MoveSource {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source: "background".to_owned(),
                target_index: 1,
            })
            .expect("move source command");
        session
            .dispatch(ProjectCommand::RemoveSource {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                source: "foreground".to_owned(),
            })
            .expect("remove source command");
        assert!(session.is_dirty());
        let saved = session.save();
        assert!(!session.is_dirty());
        assert_eq!(
            Project::parse(&saved).expect("saved project"),
            *session.project()
        );
        let source = session
            .project()
            .profiles()
            .next()
            .and_then(|profile| profile.scenes().next())
            .and_then(|scene| {
                scene
                    .sources()
                    .iter()
                    .find(|source| source.id().as_str() == "background")
            })
            .expect("background source");
        assert!(!source.visible());
        assert!(source.locked());

        assert_eq!(
            session.dispatch(ProjectCommand::SetSourceTransform {
                profile: "missing".to_owned(),
                scene: "main".to_owned(),
                source: "background".to_owned(),
                transform: FrameTransform::IDENTITY,
            }),
            Err(ProjectError::UnknownProfile(
                Identifier::new("missing").expect("identifier")
            ))
        );
        assert!(!session.is_dirty());
    }

    #[test]
    fn remove_scene_command_updates_project_without_partial_mutation() {
        let mut project = project();
        let extra = SceneSpec::new("extra", "Extra").expect("scene");
        project
            .apply(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: extra,
            })
            .expect("add scene");
        project
            .apply(ProjectCommand::SetSceneName {
                profile: "live".to_owned(),
                scene: "extra".to_owned(),
                name: "Renamed".to_owned(),
            })
            .expect("rename scene");
        assert_eq!(
            project
                .profiles()
                .next()
                .expect("profile")
                .scenes()
                .find(|scene| scene.id().as_str() == "extra")
                .expect("renamed scene")
                .name(),
            "Renamed"
        );
        project
            .apply(ProjectCommand::RemoveScene {
                profile: "live".to_owned(),
                scene: "extra".to_owned(),
            })
            .expect("remove scene");
        assert!(project
            .profiles()
            .next()
            .expect("profile")
            .scenes()
            .all(|scene| scene.id().as_str() != "extra"));
        assert_eq!(
            project.apply(ProjectCommand::RemoveScene {
                profile: "live".to_owned(),
                scene: "missing".to_owned(),
            }),
            Err(ProjectError::UnknownScene(
                Identifier::new("missing").expect("id")
            ))
        );
    }

    #[test]
    fn parser_rejects_duplicate_and_unknown_records() {
        let project = project();
        let mut duplicate = project.serialize();
        duplicate.push_str("profile|live|Other|640|360|30|1\n");
        assert!(matches!(
            Project::parse(&duplicate),
            Err(ProjectError::DuplicateProfile(_))
        ));

        let unknown = format!("{MAGIC}\nproject|title|live\nunknown|value\n");
        assert!(matches!(
            Project::parse(&unknown),
            Err(ProjectError::InvalidDocument { .. })
        ));
    }

    #[test]
    fn project_file_store_commits_atomically_and_loads_the_same_state() {
        let (final_path, temp_path) = unique_paths("save");
        let store = ProjectFileStore::new(&final_path, &temp_path).expect("store");
        let mut session = ProjectSession::new(project());
        session
            .dispatch(ProjectCommand::SetActiveProfile {
                id: "live".to_owned(),
            })
            .expect("select profile");
        assert!(session.is_dirty());

        let bytes = store.save(&mut session).expect("save project");
        assert!(bytes > 0);
        assert!(!session.is_dirty());
        assert!(!temp_path.exists());
        assert_eq!(store.load().expect("load project"), *session.project());
        std::fs::remove_file(final_path).expect("remove project fixture");
    }

    #[test]
    fn project_file_store_recovers_a_valid_unswitched_temporary_file() {
        let (final_path, temp_path) = unique_paths("recovery");
        let store = ProjectFileStore::new(&final_path, &temp_path).expect("store");
        let project = project();
        std::fs::write(&temp_path, project.serialize()).expect("write recovery fixture");

        assert_eq!(store.recover().expect("recover project"), Some(project));
        assert!(!final_path.exists());
        std::fs::remove_file(temp_path).expect("remove recovery fixture");
    }
}
