//! Group-addressed scene-item project commands.

use obs_rs_media::FrameTransform;
use obs_rs_util::Identifier;

use super::super::{
    error::ProjectError,
    model::{group_at, group_mut_at, GroupSpec, Project, SceneItemSpec, MAX_GROUP_NESTING_DEPTH},
    validation::identifier,
};
use super::types::SceneItemDuplicateMode;
use super::{copy_identity, duplicate_item_sources, validate_scene_item};

fn parse_group_path(path: &[String]) -> Result<Vec<Identifier>, ProjectError> {
    if path.is_empty() || path.len() > MAX_GROUP_NESTING_DEPTH {
        return Err(ProjectError::InvalidGroupPath);
    }
    path.iter()
        .map(|id| identifier(id, "group item id"))
        .collect()
}

fn group_mut<'a>(
    project: &'a mut Project,
    profile: &str,
    scene: &str,
    path: &[String],
) -> Result<&'a mut GroupSpec, ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let path = parse_group_path(path)?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    group_mut_at(&mut scene.items, &path).ok_or(ProjectError::InvalidGroupPath)
}

pub(super) fn set_group_item_visibility(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    item: &str,
    visible: bool,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    group_mut(project, profile, scene, group_path)?
        .item_mut(&item_id)
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
        .set_visible(visible);
    Ok(())
}

pub(super) fn set_group_item_locked(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    item: &str,
    locked: bool,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    group_mut(project, profile, scene, group_path)?
        .item_mut(&item_id)
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
        .set_locked(locked);
    Ok(())
}

pub(super) fn set_group_item_transform(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    item: &str,
    transform: FrameTransform,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    let parent_transform = group_parent_transform(project, profile, scene, group_path)?;
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let path = parse_group_path(group_path)?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let video_format = profile_spec.video_format();
    let scene_spec = profile_spec
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let group = group_at(scene_spec.items(), &path).ok_or(ProjectError::InvalidGroupPath)?;
    if !group.has_item(&item_id) {
        return Err(ProjectError::UnknownSceneItem(item_id));
    }
    if parent_transform != FrameTransform::IDENTITY {
        transform
            .compose_axis_aligned(
                parent_transform,
                video_format.width(),
                video_format.height(),
            )
            .map_err(|_| ProjectError::UnsupportedNestedSceneTransform(item_id.clone()))?;
    }
    group_mut(project, profile, scene, group_path)?
        .item_mut(&item_id)
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
        .set_transform(transform);
    Ok(())
}

fn group_parent_transform(
    project: &Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
) -> Result<FrameTransform, ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let path = parse_group_path(group_path)?;
    let profile = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let video_format = profile.video_format();
    let scene = profile
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let mut items = scene.items();
    let mut parent = FrameTransform::IDENTITY;
    for group_id in path {
        let group_item = items
            .iter()
            .find(|item| item.id() == &group_id)
            .ok_or(ProjectError::InvalidGroupPath)?;
        parent = group_item
            .transform()
            .compose_axis_aligned(parent, video_format.width(), video_format.height())
            .map_err(|_| ProjectError::UnsupportedNestedSceneTransform(group_id.clone()))?;
        items = group_item
            .group()
            .ok_or(ProjectError::InvalidGroupPath)?
            .items();
    }
    Ok(parent)
}

pub(super) fn move_group_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    item: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    group_mut(project, profile, scene, group_path)?.move_item(&item_id, target_index)
}

pub(super) fn remove_group_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    item: &str,
) -> Result<(), ProjectError> {
    let item_id = identifier(item, "scene item id")?;
    group_mut(project, profile, scene, group_path)?
        .remove_item(&item_id)
        .map(|_| ())
        .ok_or(ProjectError::UnknownSceneItem(item_id))
}

pub(super) fn duplicate_group_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    item: &str,
    mode: SceneItemDuplicateMode,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let item_id = identifier(item, "scene item id")?;
    let path = parse_group_path(group_path)?;
    let profile_ref = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene_ref = profile_ref
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let group = group_at(scene_ref.items(), &path).ok_or(ProjectError::InvalidGroupPath)?;
    let original = group
        .items()
        .iter()
        .find(|candidate| candidate.id() == &item_id)
        .cloned()
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
    paste_group_item(project, profile, scene, group_path, original, mode)
}

pub(super) fn paste_group_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    mut item: SceneItemSpec,
    mode: SceneItemDuplicateMode,
) -> Result<(), ProjectError> {
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let path = parse_group_path(group_path)?;
    let (duplicate_item_id, mut cloned_sources) = {
        let profile = project
            .profile(&profile_id)
            .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
        let scene = profile
            .scene(&scene_id)
            .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
        let group = group_at(scene.items(), &path).ok_or(ProjectError::InvalidGroupPath)?;
        validate_scene_item(profile, &scene_id, &item, 0)?;
        let duplicate_item_id = if group.has_item(item.id()) {
            copy_identity(item.id().as_str(), item.source_id().as_str(), |candidate| {
                group.has_item(candidate)
            })?
            .0
        } else {
            item.id().clone()
        };
        let mut cloned_sources = Vec::new();
        if mode == SceneItemDuplicateMode::DuplicateSource {
            let mut source_ids = std::collections::HashMap::new();
            duplicate_item_sources(profile, &mut item, &mut source_ids, &mut cloned_sources)?;
        }
        (duplicate_item_id, cloned_sources)
    };

    item.id = duplicate_item_id;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    profile.add_sources(std::mem::take(&mut cloned_sources))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let group = group_mut_at(&mut scene.items, &path).ok_or(ProjectError::InvalidGroupPath)?;
    group.add_item(item)
}
