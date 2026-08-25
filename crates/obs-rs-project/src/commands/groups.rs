//! Group-addressed scene-item project commands.

use obs_rs_media::FrameTransform;
use obs_rs_util::Identifier;
use std::collections::HashSet;

use super::super::{
    error::ProjectError,
    model::{
        group_at, group_mut_at, GroupSpec, Profile, Project, SceneItemSpec, SceneSpec,
        MAX_GROUP_NESTING_DEPTH,
    },
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

fn parent_items<'a>(
    scene: &'a SceneSpec,
    group_path: &[Identifier],
) -> Result<&'a [SceneItemSpec], ProjectError> {
    if group_path.is_empty() {
        Ok(scene.items())
    } else {
        group_at(scene.items(), group_path)
            .map(GroupSpec::items)
            .ok_or(ProjectError::InvalidGroupPath)
    }
}

fn path_is_unlocked(scene: &SceneSpec, group_path: &[Identifier]) -> Result<(), ProjectError> {
    let mut items = scene.items();
    for group_id in group_path {
        let group_item = items
            .iter()
            .find(|item| item.id() == group_id)
            .ok_or(ProjectError::InvalidGroupPath)?;
        if group_item.locked() {
            return Err(ProjectError::LockedSceneItem(group_id.clone()));
        }
        items = group_item
            .group()
            .ok_or(ProjectError::InvalidGroupPath)?
            .items();
    }
    Ok(())
}

fn parse_scene_item_target(target: &str) -> Result<(Vec<Identifier>, Identifier), ProjectError> {
    if target.is_empty() {
        return Err(ProjectError::InvalidGroupSelection);
    }
    let mut path = Vec::with_capacity(4);
    for (index, part) in target.split('/').enumerate() {
        if part.is_empty() || index > MAX_GROUP_NESTING_DEPTH {
            return Err(ProjectError::InvalidGroupSelection);
        }
        path.push(identifier(part, "scene item id")?);
    }
    let item = path.pop().ok_or(ProjectError::InvalidGroupSelection)?;
    Ok((path, item))
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

/// Applies visibility to a stable flattened target.
pub(super) fn set_scene_item_visibility_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    visible: bool,
) -> Result<(), ProjectError> {
    let (group_path, item) = parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::set_scene_item_visibility(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            visible,
        )
    } else {
        let owner_groups = owner_groups
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        set_group_item_visibility(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups,
            item.as_str(),
            visible,
        )
    }
}

/// Applies lock state to a stable flattened target.
pub(super) fn set_scene_item_locked_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    locked: bool,
) -> Result<(), ProjectError> {
    let (group_path, item) = parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::set_scene_item_locked(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            locked,
        )
    } else {
        let owner_groups = owner_groups
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        set_group_item_locked(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups,
            item.as_str(),
            locked,
        )
    }
}

/// Removes a stable flattened target from the scene that owns its leaf.
pub(super) fn remove_scene_item_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
) -> Result<(), ProjectError> {
    let (group_path, item) = parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    if owner_groups.is_empty() {
        super::remove_scene_item(project, profile, owner_scene.as_str(), item.as_str())
    } else {
        let owner_groups = owner_groups
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        remove_group_item(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups,
            item.as_str(),
        )
    }
}

pub(super) fn set_group_name(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    name: &str,
) -> Result<(), ProjectError> {
    let path = parse_group_path(group_path)?;
    let group_id = path.last().ok_or(ProjectError::InvalidGroupPath)?;
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;

    if path.len() == 1 {
        scene
            .item_mut(group_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(group_id.clone()))?
            .group_mut()
            .ok_or(ProjectError::InvalidGroupPath)?
            .set_name(name)
    } else {
        let parent_path = &group_path[..group_path.len().saturating_sub(1)];
        group_mut_at(&mut scene.items, &parse_group_path(parent_path)?)
            .ok_or(ProjectError::InvalidGroupPath)?
            .item_mut(group_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(group_id.clone()))?
            .group_mut()
            .ok_or(ProjectError::InvalidGroupPath)?
            .set_name(name)
    }
}

pub(super) fn group_scene_items(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item_ids: &[String],
    mut group: SceneItemSpec,
) -> Result<(), ProjectError> {
    if item_ids.len() < 2 {
        return Err(ProjectError::InvalidGroupSelection);
    }
    if !group.is_group() || group.group().is_some_and(|group| !group.items().is_empty()) {
        return Err(ProjectError::InvalidGroupPath);
    }
    let mut parent_path = None;
    let mut unique = HashSet::with_capacity(item_ids.len());
    let item_ids = item_ids
        .iter()
        .map(|target| {
            let (path, item) = parse_scene_item_target(target)?;
            if parent_path.as_ref().is_some_and(|current| current != &path) {
                return Err(ProjectError::InvalidGroupSelection);
            }
            parent_path = Some(path);
            if !unique.insert(item.clone()) {
                return Err(ProjectError::InvalidGroupSelection);
            }
            Ok(item)
        })
        .collect::<Result<Vec<_>, ProjectError>>()?;
    let parent_path = parent_path.ok_or(ProjectError::InvalidGroupSelection)?;

    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile_ref = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene_ref = profile_ref
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let parent_ref = parent_items(scene_ref, &parent_path)?;
    if parent_ref.iter().any(|item| item.id() == group.id()) {
        return Err(ProjectError::DuplicateSceneItem(group.id().clone()));
    }

    let mut selected = item_ids
        .iter()
        .map(|id| {
            let (index, item) = parent_ref
                .iter()
                .enumerate()
                .find(|(_, item)| item.id() == id)
                .ok_or_else(|| ProjectError::UnknownSceneItem(id.clone()))?;
            Ok((index, item))
        })
        .collect::<Result<Vec<_>, ProjectError>>()?;
    selected.sort_unstable_by_key(|(index, _)| *index);
    let insertion_index = selected.first().map_or(0, |(index, _)| *index);
    let selected_ids = selected
        .iter()
        .map(|(_, item)| item.id().clone())
        .collect::<Vec<_>>();
    let selected_items = selected
        .into_iter()
        .map(|(_, item)| item.clone())
        .collect::<Vec<_>>();
    if let Some(item) = selected_items.iter().find(|item| item.locked()) {
        return Err(ProjectError::LockedSceneItem(item.id().clone()));
    }

    let group_items = group.group_mut().ok_or(ProjectError::InvalidGroupPath)?;
    for item in selected_items {
        group_items.add_item(item)?;
    }
    let group_id = group.id().clone();

    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    if parent_path.is_empty() {
        for item_id in &selected_ids {
            scene
                .remove_item(item_id)
                .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
        }
    } else {
        let parent =
            group_mut_at(&mut scene.items, &parent_path).ok_or(ProjectError::InvalidGroupPath)?;
        for item_id in &selected_ids {
            parent
                .remove_item(item_id)
                .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
        }
        parent.add_item(group)?;
        return parent.move_item(&group_id, insertion_index);
    }
    scene.add_item(group)?;
    scene.move_item(&group_id, insertion_index)
}

pub(super) fn ungroup_scene_item(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_target: &str,
) -> Result<(), ProjectError> {
    let (parent_path, group_id) =
        parse_scene_item_target(group_target).map_err(|_| ProjectError::InvalidGroupPath)?;
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile_ref = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene_ref = profile_ref
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let parent_ref = parent_items(scene_ref, &parent_path)?;
    let group_index = parent_ref
        .iter()
        .position(|item| item.id() == &group_id)
        .ok_or_else(|| ProjectError::UnknownSceneItem(group_id.clone()))?;
    let group_item = parent_ref
        .get(group_index)
        .ok_or_else(|| ProjectError::UnknownSceneItem(group_id.clone()))?;
    if !group_item.is_group() {
        return Err(ProjectError::InvalidGroupPath);
    }
    if group_item.locked() {
        return Err(ProjectError::LockedSceneItem(group_id.clone()));
    }
    let children = group_item
        .group()
        .ok_or(ProjectError::InvalidGroupPath)?
        .items()
        .to_vec();
    let mut sibling_ids = parent_ref
        .iter()
        .filter(|item| item.id() != &group_id)
        .map(|item| item.id().clone())
        .collect::<HashSet<_>>();
    for child in &children {
        if !sibling_ids.insert(child.id().clone()) {
            return Err(ProjectError::DuplicateSceneItem(child.id().clone()));
        }
    }

    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let child_ids = children
        .iter()
        .map(|child| child.id().clone())
        .collect::<Vec<_>>();
    if parent_path.is_empty() {
        scene
            .remove_item(&group_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(group_id.clone()))?;
        scene.add_items(children)?;
        for (offset, child_id) in child_ids.into_iter().enumerate() {
            scene.move_item(&child_id, group_index + offset)?;
        }
    } else {
        let parent =
            group_mut_at(&mut scene.items, &parent_path).ok_or(ProjectError::InvalidGroupPath)?;
        parent
            .remove_item(&group_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(group_id.clone()))?;
        for child in children {
            parent.add_item(child)?;
        }
        for (offset, child_id) in child_ids.into_iter().enumerate() {
            parent.move_item(&child_id, group_index + offset)?;
        }
    }
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

/// Applies a transform to a stable flattened target.
///
/// The batch canvas command uses this adapter so root scene items and nested
/// group/scene-reference children can share one undo boundary without exposing
/// path traversal to the UI crate. The target's local transform is written to
/// the scene that owns the leaf; its enclosing axis-aligned transforms are
/// validated before mutation.
pub(super) fn set_scene_item_transform_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    target: &str,
    transform: FrameTransform,
) -> Result<(), ProjectError> {
    let (group_path, item) = parse_scene_item_target(target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let video_format = profile_spec.video_format();
    let (owner_scene, owner_groups) =
        resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
    let parent_transform = flattened_target_parent_transform(
        profile_spec,
        scene,
        &group_path,
        video_format.width(),
        video_format.height(),
    )?;
    if parent_transform != FrameTransform::IDENTITY {
        transform
            .compose_axis_aligned(
                parent_transform,
                video_format.width(),
                video_format.height(),
            )
            .map_err(|_| ProjectError::UnsupportedNestedSceneTransform(item.clone()))?;
    }
    if owner_groups.is_empty() {
        super::set_scene_item_transform(
            project,
            profile,
            owner_scene.as_str(),
            item.as_str(),
            transform,
        )
    } else {
        let owner_groups = owner_groups
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        set_group_item_transform(
            project,
            profile,
            owner_scene.as_str(),
            &owner_groups,
            item.as_str(),
            transform,
        )
    }
}

/// Resolves the scene and group owner of a flattened leaf path. A scene
/// reference changes the current scene and clears the local group path; a
/// group keeps walking inside the current scene.
fn resolve_flattened_target(
    profile: &Profile,
    scene: &str,
    groups: &[Identifier],
    item: &Identifier,
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
    if items.iter().any(|candidate| candidate.id() == item) {
        Ok((scene_id, owner_groups))
    } else {
        Err(ProjectError::UnknownSceneItem(item.clone()))
    }
}

/// Composes the transforms crossed before a flattened leaf. This validates
/// the same axis-aligned boundary that the runtime flattener uses, including
/// scene references nested inside groups.
fn flattened_target_parent_transform(
    profile: &Profile,
    scene: &str,
    groups: &[Identifier],
    width: u32,
    height: u32,
) -> Result<FrameTransform, ProjectError> {
    let mut scene_id = identifier(scene, "scene id")?;
    let mut parent_transform = FrameTransform::IDENTITY;
    let mut items = profile
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?
        .items();
    for group_id in groups {
        let parent = items
            .iter()
            .find(|candidate| candidate.id() == group_id)
            .ok_or(ProjectError::InvalidGroupPath)?;
        parent_transform = parent
            .transform()
            .compose_axis_aligned(parent_transform, width, height)
            .map_err(|_| ProjectError::UnsupportedNestedSceneTransform(group_id.clone()))?;
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
    Ok(parent_transform)
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

pub(super) fn move_scene_item_to_parent(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item: &str,
    destination: &[String],
    target_index: usize,
) -> Result<(), ProjectError> {
    let (source_parent_path, item_id) =
        parse_scene_item_target(item).map_err(|_| ProjectError::InvalidGroupPath)?;
    let destination_path = if destination.is_empty() {
        Vec::new()
    } else {
        parse_group_path(destination)?
    };
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let profile_ref = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene_ref = profile_ref
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;

    path_is_unlocked(scene_ref, &source_parent_path)?;
    let source_items = parent_items(scene_ref, &source_parent_path)?;
    let source_item = source_items
        .iter()
        .find(|candidate| candidate.id() == &item_id)
        .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
    if source_item.locked() {
        return Err(ProjectError::LockedSceneItem(item_id));
    }

    // A group cannot be moved below one of its own descendants: doing so
    // would turn the tree into a cycle and would make flattening recurse.
    let mut source_target = source_parent_path.clone();
    source_target.push(source_item.id().clone());
    if destination_path.starts_with(&source_target) {
        return Err(ProjectError::InvalidGroupPath);
    }
    path_is_unlocked(scene_ref, &destination_path)?;
    let destination_items = parent_items(scene_ref, &destination_path)?;
    if destination_items
        .iter()
        .any(|candidate| candidate.id() == source_item.id())
        && source_parent_path != destination_path
    {
        return Err(ProjectError::DuplicateSceneItem(source_item.id().clone()));
    }

    if source_parent_path == destination_path {
        if target_index >= source_items.len() {
            return Err(ProjectError::InvalidSceneItemOrder {
                index: target_index,
            });
        }
        let scene = project
            .profile_mut(&profile_id)
            .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
            .scene_mut(&scene_id)
            .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
        if source_parent_path.is_empty() {
            return scene.move_item(&item_id, target_index);
        }
        return group_mut_at(&mut scene.items, &source_parent_path)
            .ok_or(ProjectError::InvalidGroupPath)?
            .move_item(&item_id, target_index);
    }

    // A destination index may point just after the current last item, which
    // is useful for drag/drop callers. Every failure after this point is
    // ruled out by the immutable validation above.
    let destination_len = destination_items.len();
    if target_index > destination_len {
        return Err(ProjectError::InvalidSceneItemOrder {
            index: target_index,
        });
    }
    let scene = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    let moved = if source_parent_path.is_empty() {
        scene
            .remove_item(&item_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
    } else {
        group_mut_at(&mut scene.items, &source_parent_path)
            .ok_or(ProjectError::InvalidGroupPath)?
            .remove_item(&item_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?
    };
    if destination_path.is_empty() {
        scene.add_item(moved)?;
        if target_index < destination_len {
            scene.move_item(&item_id, target_index)?;
        }
    } else {
        let destination_group = group_mut_at(&mut scene.items, &destination_path)
            .ok_or(ProjectError::InvalidGroupPath)?;
        destination_group.add_item(moved)?;
        if target_index < destination_len {
            destination_group.move_item(&item_id, target_index)?;
        }
    }
    Ok(())
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

/// Removes a bounded selection of root or nested scene items in one atomic
/// project edit. Validation runs against the original scene and mutations are
/// applied to a clone, so even an unexpected stale-index failure cannot leave
/// the caller with a partially removed selection.
pub(super) fn remove_scene_items(
    project: &mut Project,
    profile: &str,
    scene: &str,
    items: &[String],
) -> Result<(), ProjectError> {
    if items.is_empty() {
        return Err(ProjectError::InvalidGroupSelection);
    }
    let profile_id = identifier(profile, "profile id")?;
    let scene_id = identifier(scene, "scene id")?;
    let source_scene = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .scene(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;

    let mut seen = HashSet::with_capacity(items.len());
    let mut parsed = Vec::with_capacity(items.len());
    for target in items {
        if !seen.insert(target.clone()) {
            return Err(ProjectError::InvalidGroupSelection);
        }
        let (group_path, item_id) = parse_scene_item_target(target)?;
        path_is_unlocked(source_scene, &group_path)?;
        let parent = parent_items(source_scene, &group_path)?;
        let item = parent
            .iter()
            .find(|candidate| candidate.id() == &item_id)
            .ok_or_else(|| ProjectError::UnknownSceneItem(item_id.clone()))?;
        if item.locked() {
            return Err(ProjectError::LockedSceneItem(item_id));
        }
        let mut full_path = group_path.clone();
        full_path.push(item_id.clone());
        parsed.push((full_path, group_path, item_id));
    }

    // A selected group owns all of its descendants. Collapse those descendant
    // targets instead of attempting to remove an item from a parent that may
    // already have been removed by the same user gesture.
    let mut canonical = parsed
        .iter()
        .filter(|(target, _, _)| {
            !parsed.iter().any(|(candidate, _, _)| {
                candidate.len() < target.len() && target.starts_with(candidate)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    canonical.sort_by_key(|target| std::cmp::Reverse(target.1.len()));

    let mut updated = project.clone();
    let updated_scene = updated
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    for (_, group_path, item_id) in canonical {
        let removed = if group_path.is_empty() {
            updated_scene.remove_item(&item_id)
        } else {
            group_mut_at(&mut updated_scene.items, &group_path)
                .ok_or(ProjectError::InvalidGroupPath)?
                .remove_item(&item_id)
        };
        if removed.is_none() {
            return Err(ProjectError::UnknownSceneItem(item_id));
        }
    }
    *project = updated;
    Ok(())
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
