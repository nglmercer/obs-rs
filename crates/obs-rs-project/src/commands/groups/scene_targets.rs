//! Adapters for stable targets that cross group and Scene-reference boundaries.

use super::super::super::{error::ProjectError, model::Project, validation::identifier};
use super::super::types::SceneItemDuplicateMode;

pub(crate) fn set_scene_item_visibility_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    visible: bool,
) -> Result<(), ProjectError> {
    let (group_path, item) = super::parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        super::resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::super::set_scene_item_visibility(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            visible,
        )
    } else {
        super::set_group_item_visibility(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups
                .into_iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            item.as_str(),
            visible,
        )
    }
}

pub(crate) fn set_scene_item_locked_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    locked: bool,
) -> Result<(), ProjectError> {
    let (group_path, item) = super::parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        super::resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::super::set_scene_item_locked(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            locked,
        )
    } else {
        super::set_group_item_locked(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups
                .into_iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            item.as_str(),
            locked,
        )
    }
}

pub(crate) fn remove_scene_item_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
) -> Result<(), ProjectError> {
    let (group_path, item) = super::parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        super::resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::super::remove_scene_item(project, profile, owner_scene.as_str(), item.as_str())
    } else {
        super::remove_group_item(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups
                .into_iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            item.as_str(),
        )
    }
}

pub(crate) fn duplicate_scene_item_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    mode: SceneItemDuplicateMode,
) -> Result<(), ProjectError> {
    let (group_path, item) = super::parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        super::resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::super::duplicate_scene_item(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            mode,
        )
    } else {
        super::duplicate_group_item(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups
                .into_iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            item.as_str(),
            mode,
        )
    }
}

pub(crate) fn move_scene_item_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    target_index: usize,
) -> Result<(), ProjectError> {
    let (group_path, item) = super::parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        super::resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::super::move_scene_item(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            target_index,
        )
    } else {
        super::move_group_item(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups
                .into_iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            item.as_str(),
            target_index,
        )
    }
}
