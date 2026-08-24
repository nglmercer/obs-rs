use super::{
    error::ProjectError,
    model::{
        Profile, Project, RenderBackendPreference, SceneItemSpec, SceneSpec, SourceFilterSpec,
        SourceSpec,
    },
    validation::{identifier, source_id},
};
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::{Identifier, MAX_IDENTIFIER_BYTES};
use std::collections::HashSet;

mod filters;
mod groups;

use filters::legacy_filter_spec;
use groups::{
    duplicate_group_item, group_scene_items, move_group_item, paste_group_item, remove_group_item,
    set_group_item_locked, set_group_item_transform, set_group_item_visibility, set_group_name,
    ungroup_scene_item,
};

mod types;

pub use types::{ProjectCommand, SceneItemDuplicateMode};

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
                duplicate_scene(self, &profile, &scene, SceneItemDuplicateMode::Reference)
            }
            ProjectCommand::DuplicateSceneWithMode {
                profile,
                scene,
                mode,
            } => duplicate_scene(self, &profile, &scene, mode),
            ProjectCommand::AddSource {
                profile,
                scene,
                source,
            } => add_source(self, &profile, &scene, source),
            ProjectCommand::AddSceneItem {
                profile,
                scene,
                item,
            } => add_scene_item(self, &profile, &scene, item),
            ProjectCommand::GroupSceneItems {
                profile,
                scene,
                items,
                group,
            } => group_scene_items(self, &profile, &scene, &items, group),
            ProjectCommand::UngroupSceneItem {
                profile,
                scene,
                group,
            } => ungroup_scene_item(self, &profile, &scene, &group),
            ProjectCommand::RemoveSceneItem {
                profile,
                scene,
                item,
            } => remove_scene_item(self, &profile, &scene, &item),
            ProjectCommand::DuplicateSceneItem {
                profile,
                scene,
                item,
                mode,
            } => duplicate_scene_item(self, &profile, &scene, &item, mode),
            ProjectCommand::PasteSceneItem {
                profile,
                scene,
                item,
                mode,
            } => paste_scene_item(self, &profile, &scene, item, mode),
            ProjectCommand::PasteGroupItem {
                profile,
                scene,
                group_path,
                item,
                mode,
            } => paste_group_item(self, &profile, &scene, &group_path, item, mode),
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
            ProjectCommand::SetSceneTransitionOverride {
                profile,
                scene,
                transition,
            } => {
                let scene = scene_mut(self, &profile, &scene)?;
                scene.set_transition_override(transition);
                Ok(())
            }
            ProjectCommand::MoveScene {
                profile,
                scene,
                target_index,
            } => move_scene(self, &profile, &scene, target_index),
            ProjectCommand::DuplicateSource { profile, source } => {
                duplicate_source(self, &profile, &source)
            }
            ProjectCommand::SetSourceName {
                profile,
                source,
                name,
            } => set_source_name(self, &profile, &source, &name),
            ProjectCommand::SetGroupName {
                profile,
                scene,
                group_path,
                name,
            } => set_group_name(self, &profile, &scene, &group_path, &name),
            ProjectCommand::SetSourceSettings {
                profile,
                source,
                settings,
            } => set_source_settings(self, &profile, &source, settings),
            ProjectCommand::SetSceneItemTransform {
                profile,
                scene,
                item,
                transform,
            } => set_scene_item_transform(self, &profile, &scene, &item, transform),
            ProjectCommand::SetSceneItemTransforms {
                profile,
                scene,
                items,
            } => set_scene_item_transforms(self, &profile, &scene, items),
            ProjectCommand::AddSourceFilter {
                profile,
                source,
                filter,
            } => add_source_filter(self, &profile, &source, filter),
            ProjectCommand::RemoveSourceFilter {
                profile,
                source,
                filter,
            } => remove_source_filter(self, &profile, &source, &filter),
            ProjectCommand::SetSourceFilterName {
                profile,
                source,
                filter,
                name,
            } => set_source_filter_name(self, &profile, &source, &filter, &name),
            ProjectCommand::SetSourceFilterEnabled {
                profile,
                source,
                filter,
                enabled,
            } => set_source_filter_enabled(self, &profile, &source, &filter, enabled),
            ProjectCommand::SetSourceFilterSettings {
                profile,
                source,
                filter,
                settings,
            } => set_source_filter_settings(self, &profile, &source, &filter, settings),
            ProjectCommand::MoveSourceFilter {
                profile,
                source,
                filter,
                target_index,
            } => move_source_filter(self, &profile, &source, &filter, target_index),
            ProjectCommand::SetSourceFilters {
                profile,
                source,
                filters,
            } => set_source_filters(self, &profile, &source, filters),
            ProjectCommand::SetSceneItemVisibility {
                profile,
                scene,
                item,
                visible,
            } => set_scene_item_visibility(self, &profile, &scene, &item, visible),
            ProjectCommand::SetSceneItemLocked {
                profile,
                scene,
                item,
                locked,
            } => set_scene_item_locked(self, &profile, &scene, &item, locked),
            ProjectCommand::SetGroupItemVisibility {
                profile,
                scene,
                group_path,
                item,
                visible,
            } => set_group_item_visibility(self, &profile, &scene, &group_path, &item, visible),
            ProjectCommand::SetGroupItemLocked {
                profile,
                scene,
                group_path,
                item,
                locked,
            } => set_group_item_locked(self, &profile, &scene, &group_path, &item, locked),
            ProjectCommand::SetGroupItemTransform {
                profile,
                scene,
                group_path,
                item,
                transform,
            } => set_group_item_transform(self, &profile, &scene, &group_path, &item, transform),
            ProjectCommand::MoveSceneItem {
                profile,
                scene,
                item,
                target_index,
            } => move_scene_item(self, &profile, &scene, &item, target_index),
            ProjectCommand::MoveGroupItem {
                profile,
                scene,
                group_path,
                item,
                target_index,
            } => move_group_item(self, &profile, &scene, &group_path, &item, target_index),
            ProjectCommand::RemoveGroupItem {
                profile,
                scene,
                group_path,
                item,
            } => remove_group_item(self, &profile, &scene, &group_path, &item),
            ProjectCommand::DuplicateGroupItem {
                profile,
                scene,
                group_path,
                item,
                mode,
            } => duplicate_group_item(self, &profile, &scene, &group_path, &item, mode),
            ProjectCommand::RemoveSource { profile, source } => {
                remove_source(self, &profile, &source)
            }
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
    let profile = project
        .profile_mut(&profile_id)
        .ok_or(ProjectError::UnknownProfile(profile_id))?;
    for item in scene.items() {
        validate_scene_item(profile, scene.id(), item, 0)?;
    }
    profile.add_scene(scene)
}

fn move_scene(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .move_scene(&scene_id, target_index)
}

fn duplicate_scene(
    project: &mut Project,
    profile: &str,
    scene: &str,
    mode: SceneItemDuplicateMode,
) -> Result<(), ProjectError> {
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
    if mode == SceneItemDuplicateMode::DuplicateSource {
        let mut cloned_sources = Vec::new();
        let mut source_ids: std::collections::HashMap<Identifier, Identifier> =
            std::collections::HashMap::new();
        for item in &mut duplicate.items {
            duplicate_item_sources(profile, item, &mut source_ids, &mut cloned_sources)?;
        }
        profile.add_sources(cloned_sources)?;
    }
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

fn validate_scene_target(
    profile: &Profile,
    owner: &Identifier,
    target: &Identifier,
) -> Result<(), ProjectError> {
    if profile.scene(target).is_none() {
        return Err(ProjectError::UnknownScene(target.clone()));
    }
    if owner == target {
        return Err(ProjectError::CircularSceneReference(owner.clone()));
    }
    let mut visited = HashSet::new();
    if scene_reaches(profile, target, owner, &mut visited)? {
        return Err(ProjectError::CircularSceneReference(owner.clone()));
    }
    Ok(())
}

fn scene_reaches(
    profile: &Profile,
    current: &Identifier,
    target: &Identifier,
    visited: &mut HashSet<Identifier>,
) -> Result<bool, ProjectError> {
    if current == target {
        return Ok(true);
    }
    if !visited.insert(current.clone()) {
        return Ok(false);
    }
    let scene = profile
        .scene(current)
        .ok_or_else(|| ProjectError::UnknownScene(current.clone()))?;
    for item in scene.items() {
        if scene_item_reaches(profile, item, target, visited)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn scene_item_reaches(
    profile: &Profile,
    item: &SceneItemSpec,
    target: &Identifier,
    visited: &mut HashSet<Identifier>,
) -> Result<bool, ProjectError> {
    if let Some(next) = item.scene_id() {
        if scene_reaches(profile, next, target, visited)? {
            return Ok(true);
        }
    }
    if let Some(group) = item.group() {
        for child in group.items() {
            if scene_item_reaches(profile, child, target, visited)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn validate_scene_item(
    profile: &Profile,
    owner: &Identifier,
    item: &SceneItemSpec,
    group_depth: usize,
) -> Result<(), ProjectError> {
    if let Some(target) = item.scene_id() {
        validate_scene_target(profile, owner, target)?;
    } else if item.is_source() && !profile.has_source(item.source_id()) {
        return Err(ProjectError::UnknownSource(item.source_id().clone()));
    }
    if let Some(group) = item.group() {
        if group_depth >= super::model::MAX_GROUP_NESTING_DEPTH {
            return Err(ProjectError::GroupNestingTooDeep(
                super::model::MAX_GROUP_NESTING_DEPTH,
            ));
        }
        for child in group.items() {
            validate_scene_item(profile, owner, child, group_depth.saturating_add(1))?;
        }
    }
    Ok(())
}

fn duplicate_item_sources(
    profile: &Profile,
    item: &mut SceneItemSpec,
    source_ids: &mut std::collections::HashMap<Identifier, Identifier>,
    cloned_sources: &mut Vec<SourceSpec>,
) -> Result<(), ProjectError> {
    if item.is_source() {
        let original_source_id = item.source_id().clone();
        let new_source_id = if let Some(new_id) = source_ids.get(&original_source_id) {
            new_id.clone()
        } else {
            let source = profile
                .source(&original_source_id)
                .cloned()
                .ok_or_else(|| ProjectError::UnknownSource(original_source_id.clone()))?;
            let (new_id, new_name) =
                copy_identity(source.id().as_str(), source.name(), |candidate| {
                    profile.has_source(candidate)
                        || cloned_sources
                            .iter()
                            .any(|source: &SourceSpec| source.id().as_str() == candidate)
                })?;
            let mut source = source;
            source.id = new_id.clone();
            source.name = new_name;
            source_ids.insert(original_source_id, new_id.clone());
            cloned_sources.push(source);
            new_id
        };
        item.set_source_id(new_source_id);
    } else if let Some(group) = item.group_mut() {
        for child in group.items_mut() {
            duplicate_item_sources(profile, child, source_ids, cloned_sources)?;
        }
    }
    Ok(())
}

fn add_source(
    project: &mut Project,
    profile: &str,
    scene: &str,
    source: SourceSpec,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let source_id = source.id().clone();
    let item = SceneItemSpec::for_source(source_id.as_str())?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene_spec = profile
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    if profile.has_source(&source_id) {
        return Err(ProjectError::DuplicateSource(source_id));
    }
    if scene_spec.has_item(item.id()) {
        return Err(ProjectError::DuplicateSceneItem(item.id().clone()));
    }
    profile.add_source(source)?;
    profile
        .scene_mut(&scene_id)
        .ok_or(ProjectError::UnknownScene(scene_id))?
        .add_item(item)
}

fn add_scene_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: SceneItemSpec,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    validate_scene_item(profile, &scene_id, &item, 0)?;
    profile
        .scene_mut(&scene_id)
        .ok_or(ProjectError::UnknownScene(scene_id))?
        .add_item(item)
}

fn remove_scene_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    scene_mut(project, profile, scene)?
        .remove_item(&item_id)
        .map(|_| ())
        .ok_or(ProjectError::UnknownSceneItem(item_id))
}

fn duplicate_scene_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
    mode: SceneItemDuplicateMode,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let original = project
        .profile(&profile_id)
        .and_then(|profile| profile.scene(&scene_id))
        .and_then(|scene| scene.item(&item_id))
        .cloned()
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
    paste_scene_item(project, profile, scene, original, mode)
}

fn paste_scene_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    mut item: SceneItemSpec,
    mode: SceneItemDuplicateMode,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let duplicate_item_id = {
        let scene = profile
            .scene(&scene_id)
            .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
        if scene.has_item(item.id()) {
            copy_identity(item.id().as_str(), item.id().as_str(), |candidate| {
                scene.has_item(candidate)
            })?
            .0
        } else {
            item.id().clone()
        }
    };

    if item.scene_id().is_some() || item.is_group() {
        validate_scene_item(profile, &scene_id, &item, 0)?;
        if item.is_group() && mode == SceneItemDuplicateMode::DuplicateSource {
            let mut cloned_sources = Vec::new();
            let mut source_ids = std::collections::HashMap::new();
            duplicate_item_sources(profile, &mut item, &mut source_ids, &mut cloned_sources)?;
            profile.add_sources(cloned_sources)?;
        }
        item.id = duplicate_item_id;
        return profile
            .scene_mut(&scene_id)
            .ok_or(ProjectError::UnknownScene(scene_id))?
            .add_item(item);
    }

    let source_id = match mode {
        SceneItemDuplicateMode::Reference => {
            let source_id = item.source_id().clone();
            if !profile.has_source(&source_id) {
                return Err(ProjectError::UnknownSource(source_id));
            }
            source_id
        }
        SceneItemDuplicateMode::DuplicateSource => {
            let source_id = item.source_id().clone();
            let source = profile
                .source(&source_id)
                .cloned()
                .ok_or_else(|| ProjectError::UnknownSource(source_id.clone()))?;
            let (new_id, new_name) =
                copy_identity(source.id().as_str(), source.name(), |candidate| {
                    profile.has_source(candidate)
                })?;
            let mut duplicate = source;
            duplicate.id = new_id.clone();
            duplicate.name = new_name;
            profile.add_source(duplicate)?;
            new_id
        }
    };
    item.id = duplicate_item_id;
    item.set_source_id(source_id);
    profile
        .scene_mut(&scene_id)
        .ok_or(ProjectError::UnknownScene(scene_id))?
        .add_item(item)
}

fn source_mut<'a>(
    project: &'a mut Project,
    profile: &str,
    source: &str,
) -> Result<&'a mut SourceSpec, ProjectError> {
    let source_id = source_id(source)?;
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .source_mut(&source_id)
        .ok_or(ProjectError::UnknownSource(source_id))
}

fn set_source_settings(
    project: &mut Project,
    profile: &str,
    source: &str,
    settings: Config,
) -> Result<(), ProjectError> {
    source_mut(project, profile, source)?.set_settings(settings);
    Ok(())
}

fn duplicate_source(
    project: &mut Project,
    profile: &str,
    source: &str,
) -> Result<(), ProjectError> {
    let source_id = source_id(source)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let original = profile
        .source(&source_id)
        .cloned()
        .ok_or_else(|| ProjectError::UnknownSource(source_id.clone()))?;
    let (id, name) = copy_identity(original.id().as_str(), original.name(), |candidate| {
        profile.has_source(candidate)
    })?;
    let mut duplicate = original;
    duplicate.id = id;
    duplicate.name = name;
    profile.add_source(duplicate)
}

fn set_source_name(
    project: &mut Project,
    profile: &str,
    source: &str,
    name: &str,
) -> Result<(), ProjectError> {
    source_mut(project, profile, source)?.set_name(name)
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

fn item_mut<'a>(
    project: &'a mut Project,
    profile: &str,
    scene: &str,
    item: &str,
) -> Result<&'a mut SceneItemSpec, ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    scene_mut(project, profile, scene)?
        .item_mut(&item_id)
        .ok_or(ProjectError::UnknownSceneItem(item_id))
}

fn set_scene_item_transform(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
    transform: FrameTransform,
) -> Result<(), ProjectError> {
    item_mut(project, profile, scene, item)?.set_transform(transform);
    Ok(())
}

fn set_scene_item_transforms(
    project: &mut Project,
    profile: &str,
    scene: &str,
    items: Vec<(String, FrameTransform)>,
) -> Result<(), ProjectError> {
    let scene = scene_mut(project, profile, scene)?;
    let mut updates = Vec::with_capacity(items.len());
    let mut seen = HashSet::with_capacity(items.len());
    for (item, transform) in items {
        let item = identifier(&item, "scene item id")?;
        if !seen.insert(item.clone()) {
            return Err(ProjectError::UnknownSceneItem(item));
        }
        if !scene.has_item(&item) {
            return Err(ProjectError::UnknownSceneItem(item));
        }
        updates.push((item, transform));
    }
    for (item, transform) in updates {
        // Every item was validated before any mutation, so this lookup cannot
        // fail and the command remains atomic from the caller's perspective.
        scene
            .item_mut(&item)
            .expect("validated scene item exists")
            .set_transform(transform);
    }
    Ok(())
}

fn add_source_filter(
    project: &mut Project,
    profile: &str,
    source: &str,
    filter: SourceFilterSpec,
) -> Result<(), ProjectError> {
    source_mut(project, profile, source)?.add_filter(filter)
}

fn set_source_filters(
    project: &mut Project,
    profile: &str,
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
    source_mut(project, profile, source)?.filters = specs;
    Ok(())
}

fn remove_source_filter(
    project: &mut Project,
    profile: &str,
    source: &str,
    filter: &str,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    source_mut(project, profile, source)?
        .remove_filter(&filter)
        .map(|_| ())
        .ok_or(ProjectError::UnknownFilter(filter))
}

fn set_source_filter_name(
    project: &mut Project,
    profile: &str,
    source: &str,
    filter: &str,
    name: &str,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    source_mut(project, profile, source)?
        .filter_mut(&filter)
        .ok_or_else(|| ProjectError::UnknownFilter(filter.clone()))?
        .set_name(name)
}

fn set_source_filter_enabled(
    project: &mut Project,
    profile: &str,
    source: &str,
    filter: &str,
    enabled: bool,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    let filter = source_mut(project, profile, source)?
        .filter_mut(&filter)
        .ok_or(ProjectError::UnknownFilter(filter))?;
    filter.set_enabled(enabled);
    Ok(())
}

fn set_source_filter_settings(
    project: &mut Project,
    profile: &str,
    source: &str,
    filter: &str,
    settings: Config,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    let filter = source_mut(project, profile, source)?
        .filter_mut(&filter)
        .ok_or(ProjectError::UnknownFilter(filter))?;
    filter.set_settings(settings);
    Ok(())
}

fn move_source_filter(
    project: &mut Project,
    profile: &str,
    source: &str,
    filter: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let filter = identifier(filter, "filter id")?;
    source_mut(project, profile, source)?.move_filter(&filter, target_index)
}

fn set_scene_item_visibility(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
    visible: bool,
) -> Result<(), ProjectError> {
    item_mut(project, profile, scene, item)?.set_visible(visible);
    Ok(())
}

fn set_scene_item_locked(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
    locked: bool,
) -> Result<(), ProjectError> {
    item_mut(project, profile, scene, item)?.set_locked(locked);
    Ok(())
}

fn move_scene_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let item = identifier(item, "scene item id")?;
    scene_mut(project, profile, scene)?.move_item(&item, target_index)
}

fn remove_source(project: &mut Project, profile: &str, source: &str) -> Result<(), ProjectError> {
    let source = source_id(source)?;
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .remove_source(&source)
        .map(|_| ())
}
