//! Adapters for stable targets that cross group and Scene-reference boundaries.

use super::super::super::{
    error::ProjectError,
    model::{Project, SceneItemSpec},
    validation::identifier,
};
use super::super::types::SceneItemDuplicateMode;
use obs_rs_util::Identifier;

pub(crate) fn group_scene_items_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    item_targets: &[String],
    group: SceneItemSpec,
) -> Result<(), ProjectError> {
    if item_targets.len() < 2 {
        return Err(ProjectError::InvalidGroupSelection);
    }

    let profile_id = identifier(profile, "profile id")?;
    let (owner_scene, owner_groups, local_items) = {
        let profile_spec = project
            .profile(&profile_id)
            .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
        let mut owner = None;
        let mut local_items = Vec::with_capacity(item_targets.len());
        for target in item_targets {
            let (group_path, item) = super::parse_scene_item_target(target)?;
            let resolved =
                super::resolve_flattened_target(profile_spec, scene, &group_path, &item)?;
            if owner
                .as_ref()
                .is_some_and(|current: &(Identifier, Vec<Identifier>)| current != &resolved)
            {
                return Err(ProjectError::InvalidGroupSelection);
            }
            owner = Some(resolved);
            local_items.push(item);
        }
        let (owner_scene, owner_groups) = owner.ok_or(ProjectError::InvalidGroupSelection)?;
        (owner_scene, owner_groups, local_items)
    };

    let local_prefix = owner_groups
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect::<Vec<_>>();
    let local_items = local_items
        .into_iter()
        .map(|item| {
            let mut target = local_prefix.clone();
            target.push(item.as_str().to_owned());
            target.join("/")
        })
        .collect::<Vec<_>>();
    super::group_scene_items(project, profile, owner_scene.as_str(), &local_items, group)
}

pub(crate) fn ungroup_scene_item_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_target: &str,
) -> Result<(), ProjectError> {
    let (group_path, group_id) =
        super::parse_scene_item_target(group_target).map_err(|_| ProjectError::InvalidGroupPath)?;
    let profile_id = identifier(profile, "profile id")?;
    let (owner_scene, owner_groups) = {
        let profile_spec = project
            .profile(&profile_id)
            .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
        super::resolve_flattened_target(profile_spec, scene, &group_path, &group_id)?
    };
    let mut local_target = owner_groups
        .into_iter()
        .map(|id| id.as_str().to_owned())
        .collect::<Vec<_>>();
    local_target.push(group_id.as_str().to_owned());
    let local_target = local_target.join("/");
    super::ungroup_scene_item(project, profile, owner_scene.as_str(), &local_target)
}

pub(crate) fn set_scene_item_group_name_target(
    project: &mut Project,
    profile: &str,
    scene: &str,
    group_path: &[String],
    name: &str,
) -> Result<(), ProjectError> {
    if group_path.is_empty() {
        return Err(ProjectError::InvalidGroupPath);
    }
    let target = group_path.join("/");
    let (parent_path, group_id) = super::parse_scene_item_target(&target)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile_spec = project
        .profile(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let (owner_scene, owner_groups) =
        super::resolve_flattened_target(profile_spec, scene, &parent_path, &group_id)?;
    let mut owner_path = owner_groups
        .into_iter()
        .map(|id| id.as_str().to_owned())
        .collect::<Vec<_>>();
    owner_path.push(group_id.as_str().to_owned());
    super::set_group_name(project, profile, owner_scene.as_str(), &owner_path, name)
}

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
