use super::{
    error::ProjectError,
    model::{Profile, Project, SceneSpec, SourceSpec},
    validation::{identifier, source_id},
};
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform};
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
