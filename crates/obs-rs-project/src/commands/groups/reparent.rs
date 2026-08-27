//! Cross-owner scene-item path resolution and transactional reparenting.

use obs_rs_util::Identifier;

use super::super::super::{
    error::ProjectError,
    model::{Profile, Project},
    validation::identifier,
};

/// Resolves the scene and group owner of a flattened parent path. A scene
/// reference changes the current scene and clears the local group path; a
/// group keeps walking inside the current scene.
pub(crate) fn resolve_flattened_parent(
    profile: &Profile,
    scene: &str,
    groups: &[Identifier],
) -> Result<(Identifier, Vec<Identifier>), ProjectError> {
    let mut scene_id = identifier(scene, "scene id")?;
    let mut owner_groups = Vec::new();
    let mut items = profile
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?
        .items();
    for group_id in groups {
        let parent = items
            .iter()
            .find(|candidate| candidate.id() == group_id)
            .ok_or(ProjectError::InvalidGroupPath)?;
        if let Some(group) = parent.group() {
            owner_groups.push(group_id.clone());
            items = group.items();
        } else if let Some(child_scene) = parent.scene_id() {
            scene_id = child_scene.clone();
            owner_groups.clear();
            items = profile
                .scene(&scene_id)
                .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?
                .items();
        } else {
            return Err(ProjectError::InvalidGroupPath);
        }
    }
    Ok((scene_id, owner_groups))
}

/// Validates lock state on every group or Scene-reference boundary crossed by
/// one flattened parent path. The local owner path is checked separately by
/// the movement core.
pub(crate) fn flattened_path_is_unlocked(
    profile: &Profile,
    scene: &str,
    groups: &[Identifier],
) -> Result<(), ProjectError> {
    let mut scene_id = identifier(scene, "scene id")?;
    let mut items = profile
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?
        .items();
    for group_id in groups {
        let parent = items
            .iter()
            .find(|candidate| candidate.id() == group_id)
            .ok_or(ProjectError::InvalidGroupPath)?;
        if parent.locked() {
            return Err(ProjectError::LockedSceneItem(group_id.clone()));
        }
        if let Some(group) = parent.group() {
            items = group.items();
        } else if let Some(child_scene) = parent.scene_id() {
            scene_id = child_scene.clone();
            items = profile
                .scene(&scene_id)
                .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?
                .items();
        } else {
            return Err(ProjectError::InvalidGroupPath);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct SceneItemMoveRequest<'a> {
    pub(crate) profile: &'a str,
    pub(crate) source_scene: &'a Identifier,
    pub(crate) source_parent_path: &'a [Identifier],
    pub(crate) item_id: &'a Identifier,
    pub(crate) destination_scene: &'a Identifier,
    pub(crate) destination_path: &'a [Identifier],
    pub(crate) target_index: usize,
}

/// Moves an already validated item between two different owning scenes. The
/// caller resolves flattened paths and validates crossed Scene-reference
/// boundaries; this core keeps the two-scene mutation transactional by
/// applying it to a project clone before publishing it.
pub(crate) fn move_scene_item_between_parents(
    project: &mut Project,
    request: &SceneItemMoveRequest<'_>,
) -> Result<(), ProjectError> {
    let request = *request;
    let SceneItemMoveRequest {
        profile,
        source_scene,
        source_parent_path,
        item_id,
        destination_scene,
        destination_path,
        target_index,
    } = request;
    let profile_id = identifier(profile, "profile id")?;
    let profile_ref = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let source_scene_ref = profile_ref
        .scene(source_scene)
        .ok_or_else(|| ProjectError::UnknownScene(source_scene.clone()))?;
    let destination_scene_ref = profile_ref
        .scene(destination_scene)
        .ok_or_else(|| ProjectError::UnknownScene(destination_scene.clone()))?;

    super::path_is_unlocked(source_scene_ref, source_parent_path)?;
    let source_items = super::parent_items(source_scene_ref, source_parent_path)?;
    let source_item = source_items
        .iter()
        .find(|candidate| candidate.id() == item_id)
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
    if source_item.locked() {
        return Err(ProjectError::LockedSceneItem(item_id.clone()));
    }
    super::path_is_unlocked(destination_scene_ref, destination_path)?;
    let destination_items = super::parent_items(destination_scene_ref, destination_path)?;
    if destination_items
        .iter()
        .any(|candidate| candidate.id() == item_id)
    {
        return Err(ProjectError::DuplicateSceneItem(item_id.clone()));
    }
    if target_index > destination_items.len() {
        return Err(ProjectError::InvalidSceneItemOrder {
            index: target_index,
        });
    }
    super::validate_scene_item(
        profile_ref,
        destination_scene,
        source_item,
        destination_path.len(),
    )?;

    let destination_len = destination_items.len();
    let mut updated = project.clone();
    let profile = updated
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let moved = {
        let scene = profile
            .scene_mut(source_scene)
            .ok_or_else(|| ProjectError::UnknownScene(source_scene.clone()))?;
        if source_parent_path.is_empty() {
            scene
                .remove_item(item_id)
                .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
        } else {
            super::group_mut_at(&mut scene.items, source_parent_path)
                .ok_or(ProjectError::InvalidGroupPath)?
                .remove_item(item_id)
                .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
        }
    };
    let scene = profile
        .scene_mut(destination_scene)
        .ok_or_else(|| ProjectError::UnknownScene(destination_scene.clone()))?;
    if destination_path.is_empty() {
        scene.add_item(moved)?;
        if target_index < destination_len {
            scene.move_item(item_id, target_index)?;
        }
    } else {
        let destination_group = super::group_mut_at(&mut scene.items, destination_path)
            .ok_or(ProjectError::InvalidGroupPath)?;
        destination_group.add_item(moved)?;
        if target_index < destination_len {
            destination_group.move_item(item_id, target_index)?;
        }
    }
    *project = updated;
    Ok(())
}
