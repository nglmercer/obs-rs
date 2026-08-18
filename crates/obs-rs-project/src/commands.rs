use super::{
    error::ProjectError,
    model::{
        Profile, Project, RenderBackendPreference, SceneSpec, SourceFilterCategory,
        SourceFilterSpec, SourceSpec,
    },
    validation::{identifier, source_id},
};
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::{Identifier, MAX_IDENTIFIER_BYTES};
/// Commands that mutate project state through one validated path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCommand {
    /// Adds a profile.
    AddProfile(Profile),
    /// Selects an existing profile.
    SetActiveProfile { id: String },
    /// Replaces one profile's canvas resolution and frame rate.
    SetProfileVideoFormat {
        profile: String,
        format: VideoFormat,
    },
    /// Selects the preferred renderer while retaining runtime fallback.
    SetProfileRenderBackend {
        profile: String,
        backend: RenderBackendPreference,
    },
    /// Selects an exact output profile for runtime capability negotiation.
    SetProfileOutput {
        profile: String,
        output: OutputProfileKind,
    },
    /// Adds a scene to a profile.
    AddScene { profile: String, scene: SceneSpec },
    /// Duplicates one scene with a fresh project-local ID.
    DuplicateScene { profile: String, scene: String },
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
    /// Duplicates one source with a fresh scene-local ID.
    DuplicateSource {
        profile: String,
        scene: String,
        source: String,
    },
    /// Replaces one source's display name.
    SetSourceName {
        profile: String,
        scene: String,
        source: String,
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
    /// Adds one persistent source filter instance.
    AddSourceFilter {
        profile: String,
        scene: String,
        source: String,
        filter: SourceFilterSpec,
    },
    /// Removes one persistent source filter instance.
    RemoveSourceFilter {
        profile: String,
        scene: String,
        source: String,
        filter: String,
    },
    /// Replaces one source filter's display name.
    SetSourceFilterName {
        profile: String,
        scene: String,
        source: String,
        filter: String,
        name: String,
    },
    /// Enables or disables one source filter instance.
    SetSourceFilterEnabled {
        profile: String,
        scene: String,
        source: String,
        filter: String,
        enabled: bool,
    },
    /// Replaces one source filter's independent settings document.
    SetSourceFilterSettings {
        profile: String,
        scene: String,
        source: String,
        filter: String,
        settings: Config,
    },
    /// Moves one source filter to an existing order position.
    MoveSourceFilter {
        profile: String,
        scene: String,
        source: String,
        filter: String,
        target_index: usize,
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
    #[allow(
        clippy::too_many_lines,
        reason = "the command enum intentionally has one centralized dispatch point"
    )]
    pub fn apply(&mut self, command: ProjectCommand) -> Result<(), ProjectError> {
        match command {
            ProjectCommand::AddProfile(profile) => self.add_profile(profile),
            ProjectCommand::SetActiveProfile { id } => self.set_active_profile(&id),
            ProjectCommand::SetProfileVideoFormat { profile, format } => {
                set_profile_video_format(self, &profile, format)
            }
            ProjectCommand::SetProfileRenderBackend { profile, backend } => {
                set_profile_render_backend(self, &profile, backend)
            }
            ProjectCommand::SetProfileOutput { profile, output } => {
                set_profile_output(self, &profile, output)
            }
            ProjectCommand::AddScene { profile, scene } => add_scene(self, &profile, scene),
            ProjectCommand::DuplicateScene { profile, scene } => {
                duplicate_scene(self, &profile, &scene)
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
            ProjectCommand::DuplicateSource {
                profile,
                scene,
                source,
            } => duplicate_source(self, &profile, &scene, &source),
            ProjectCommand::SetSourceName {
                profile,
                scene,
                source,
                name,
            } => set_source_name(self, &profile, &scene, &source, &name),
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
            ProjectCommand::RemoveSourceFilter {
                profile,
                scene,
                source,
                filter,
            } => remove_source_filter(self, &profile, &scene, &source, &filter),
            ProjectCommand::SetSourceFilterName {
                profile,
                scene,
                source,
                filter,
                name,
            } => set_source_filter_name(self, &profile, &scene, &source, &filter, &name),
            ProjectCommand::SetSourceFilterEnabled {
                profile,
                scene,
                source,
                filter,
                enabled,
            } => set_source_filter_enabled(self, &profile, &scene, &source, &filter, enabled),
            ProjectCommand::SetSourceFilterSettings {
                profile,
                scene,
                source,
                filter,
                settings,
            } => set_source_filter_settings(self, &profile, &scene, &source, &filter, settings),
            ProjectCommand::MoveSourceFilter {
                profile,
                scene,
                source,
                filter,
                target_index,
            } => move_source_filter(self, &profile, &scene, &source, &filter, target_index),
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

fn set_profile_video_format(
    project: &mut Project,
    profile: &str,
    format: VideoFormat,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or(ProjectError::UnknownProfile(profile_id))?
        .set_video_format(format);
    Ok(())
}

fn set_profile_render_backend(
    project: &mut Project,
    profile: &str,
    backend: RenderBackendPreference,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or(ProjectError::UnknownProfile(profile_id))?
        .set_render_backend(backend);
    Ok(())
}

fn set_profile_output(
    project: &mut Project,
    profile: &str,
    output: OutputProfileKind,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or(ProjectError::UnknownProfile(profile_id))?
        .set_output_profile(output);
    Ok(())
}

fn add_scene(project: &mut Project, profile: &str, scene: SceneSpec) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or(ProjectError::UnknownProfile(profile_id))?
        .add_scene(scene)
}

fn duplicate_scene(project: &mut Project, profile: &str, scene: &str) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let original = profile
        .scene(&scene_id)
        .cloned()
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let (id, name) = copy_identity(original.id().as_str(), original.name(), |candidate| {
        profile.scene(candidate).is_some()
    })?;
    let mut duplicate = original;
    duplicate.id = id;
    duplicate.name = name;
    profile.add_scene(duplicate)
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

fn duplicate_source(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
) -> Result<(), ProjectError> {
    let source_id = source_id(source)?;
    let scene = scene_mut(project, profile, scene)?;
    let original = scene
        .source(&source_id)
        .cloned()
        .ok_or_else(|| ProjectError::UnknownSource(source_id.clone()))?;
    let (id, name) = copy_identity(original.id().as_str(), original.name(), |candidate| {
        scene.has_source(candidate)
    })?;
    let mut duplicate = original;
    duplicate.id = id;
    duplicate.name = name;
    scene.add_source(duplicate)
}

fn set_source_name(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    name: &str,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.set_name(name)
}

/// Builds a deterministic copy ID and display name without letting the GUI
/// invent identifiers or bypass the project's validation rules.
fn copy_identity(
    base_id: &str,
    base_name: &str,
    is_taken: impl Fn(&str) -> bool,
) -> Result<(Identifier, String), ProjectError> {
    for ordinal in 1..=10_000_u32 {
        let suffix = if ordinal == 1 {
            "_copy".to_owned()
        } else {
            format!("_copy_{ordinal}")
        };
        let prefix_length = MAX_IDENTIFIER_BYTES.saturating_sub(suffix.len());
        let prefix = base_id
            .get(..base_id.len().min(prefix_length))
            .unwrap_or(base_id);
        let candidate = format!("{prefix}{suffix}");
        if !is_taken(&candidate) {
            let id =
                Identifier::new(&candidate).map_err(|error| ProjectError::InvalidIdentifier {
                    kind: "duplicate id",
                    error,
                })?;
            let name = if ordinal == 1 {
                format!("{base_name} Copy")
            } else {
                format!("{base_name} Copy {ordinal}")
            };
            return Ok((id, name));
        }
    }
    Err(ProjectError::InvalidIdentifier {
        kind: "duplicate id",
        error: obs_rs_util::IdentifierError::TooLong,
    })
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
    filter: SourceFilterSpec,
) -> Result<(), ProjectError> {
    source_mut(project, profile, scene, source)?.add_filter(filter)
}

fn set_source_filters(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filters: Vec<FrameFilter>,
) -> Result<(), ProjectError> {
    // Compatibility bridge for older frontends. New code must use the
    // instance commands above; converting here keeps old command documents
    // from silently reintroducing renderer values into the project model.
    let specs = filters
        .into_iter()
        .enumerate()
        .map(|(index, filter)| legacy_filter_spec(index, filter))
        .collect::<Result<Vec<_>, _>>()?;
    source_mut(project, profile, scene, source)?.filters = specs;
    Ok(())
}

fn remove_source_filter(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filter: &str,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    source_mut(project, profile, scene, source)?
        .remove_filter(&filter)
        .map(|_| ())
        .ok_or(ProjectError::UnknownFilter(filter))
}

fn set_source_filter_name(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filter: &str,
    name: &str,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    source_mut(project, profile, scene, source)?
        .filter_mut(&filter)
        .ok_or_else(|| ProjectError::UnknownFilter(filter.clone()))?
        .set_name(name)
}

fn set_source_filter_enabled(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filter: &str,
    enabled: bool,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    let filter = source_mut(project, profile, scene, source)?
        .filter_mut(&filter)
        .ok_or(ProjectError::UnknownFilter(filter))?;
    filter.set_enabled(enabled);
    Ok(())
}

fn set_source_filter_settings(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filter: &str,
    settings: Config,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    let filter = source_mut(project, profile, scene, source)?
        .filter_mut(&filter)
        .ok_or(ProjectError::UnknownFilter(filter))?;
    filter.set_settings(settings);
    Ok(())
}

fn move_source_filter(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: &str,
    filter: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    source_mut(project, profile, scene, source)?.move_filter(&filter, target_index)
}

fn legacy_filter_spec(index: usize, filter: FrameFilter) -> Result<SourceFilterSpec, ProjectError> {
    let (kind, name, settings) = match filter {
        FrameFilter::Grayscale => ("grayscale", "Grayscale", Config::new()),
        FrameFilter::Brightness { milli } => {
            let mut settings = Config::new();
            settings
                .set("milli", &milli.to_string())
                .map_err(ProjectError::Config)?;
            ("brightness", "Brightness", settings)
        }
        FrameFilter::Opacity(value) => {
            let mut settings = Config::new();
            settings
                .set("value", &value.to_string())
                .map_err(ProjectError::Config)?;
            ("opacity", "Opacity", settings)
        }
    };
    SourceFilterSpec::with_category(
        &format!("legacy_filter_{}", index.saturating_add(1)),
        name,
        kind,
        SourceFilterCategory::Effect,
        settings,
    )
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
