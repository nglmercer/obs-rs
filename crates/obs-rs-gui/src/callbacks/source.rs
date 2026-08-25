use std::{cell::RefCell, error::Error, rc::Rc};

use slint::{DataTransfer, Weak};

use obs_rs_config::Config;
use obs_rs_media::FrameTransform;
use obs_rs_project::{Profile, ProjectCommand, SceneItemDuplicateMode, SceneItemSpec, SceneSpec};
use obs_rs_ui::{DesktopState, UiCommand};

#[path = "source_targets.rs"]
mod source_targets;

pub(crate) use source_targets::{
    scene_item_target, scene_item_target_is_locked, selected_target, source_target,
    source_target_is_locked, target_settings_document, SceneItemTarget, SourceTarget,
};

use crate::{
    callbacks::canvas::{
        canvas_item_for_target, canvas_target_is_locked_in_profile, transform_for_command,
        CanvasTransformCommand,
    },
    refresh_ui, MainWindow, PreviewSurface,
};

pub(crate) fn remove_scene_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    scene_id: &str,
) {
    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let result = state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveScene {
            profile,
            scene: scene_id.to_owned(),
        }));
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Remove scene failed: {error}").into()),
    }
}

pub(crate) fn move_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
    delta: i32,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        if locked {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let (source_index, source_count) =
            source_order_state(&state.borrow(), scene.as_str(), source_id)?;
        let target = i32::try_from(source_index)
            .unwrap_or(i32::MAX)
            .saturating_add(delta);
        let target_index = usize::try_from(target)
            .map_err(|_| std::io::Error::other("source cannot move above the scene"))?
            .min(source_count.saturating_sub(1));
        let command = ProjectCommand::MoveSceneItem {
            profile,
            scene,
            item: source_id.to_owned(),
            target_index,
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Move source failed: {error}").into()),
    }
}

pub(crate) fn move_source_to_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
    target_index: i32,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        if locked {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let (_, source_count) = source_order_state(&state.borrow(), scene.as_str(), source_id)?;
        let target_index = usize::try_from(target_index)
            .map_err(|_| std::io::Error::other("source order is invalid"))?
            .min(source_count.saturating_sub(1));
        let command = ProjectCommand::MoveSceneItem {
            profile,
            scene,
            item: source_id.to_owned(),
            target_index,
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Move source failed: {error}").into()),
    }
}

pub(crate) fn move_source_to_group_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
    destination: &str,
) {
    let result: Result<String, Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        if locked {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let item = source_id
            .rsplit('/')
            .next()
            .filter(|item| !item.is_empty())
            .ok_or_else(|| std::io::Error::other("source target is invalid"))?;
        let destination_path = if destination.is_empty() {
            Vec::new()
        } else {
            destination.split('/').map(str::to_owned).collect()
        };
        let new_target = if destination.is_empty() {
            item.to_owned()
        } else {
            format!("{destination}/{item}")
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveSceneItemToParent {
                profile,
                scene,
                item: source_id.to_owned(),
                destination: destination_path,
                // The menu chooses the top of the destination group. A later
                // drag/drop packet can use another validated index.
                target_index: 0,
            }))?;
        state.borrow_mut().dispatch(UiCommand::SelectSource {
            id: new_target.clone(),
        })?;
        Ok(new_target)
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(_) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Move source failed: {error}").into()),
    }
}

/// Applies one Sources-dock pointer drop through the same typed reparenting
/// command used by the context menu. A container target receives the item at
/// its front; a leaf target inserts before or after that leaf according to the
/// bounded drop mode supplied by Slint.
pub(crate) fn move_source_by_drop_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    data: &DataTransfer,
    target: &str,
    mode: i32,
) {
    let result: Result<String, Box<dyn Error>> = (|| {
        let source_id = data
            .plain_text()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .to_string();
        if source_id.is_empty() {
            return Err(std::io::Error::other("source drag payload is empty").into());
        }
        if mode != 0 && mode != 1 && mode != 2 {
            return Err(std::io::Error::other("source drop mode is invalid").into());
        }
        let (profile, scene, destination, target_index) = {
            let state = state.borrow();
            let (profile, scene, _, locked) = source_display_state(&state, &source_id)?;
            if locked {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "source is locked",
                )
                .into());
            }
            if source_id == target && mode != 0 {
                return Ok(source_id);
            }
            let project = state.project_session().project();
            let profile_spec = project
                .active_profile_spec()
                .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
            let target_item = canvas_item_for_target(profile_spec, scene.as_str(), target)
                .ok_or_else(|| std::io::Error::other("drop target is not in the preview scene"))?;
            if target_item.is_group() || target_item.is_scene_reference() {
                (profile, scene, target.to_owned(), 0)
            } else {
                let (target_index, _) = source_order_state(&state, scene.as_str(), target)?;
                let source_parent = crate::callbacks::source_parent_path(&source_id)
                    .ok_or_else(|| std::io::Error::other("source target is invalid"))?;
                let destination_path = crate::callbacks::source_parent_path(target)
                    .ok_or_else(|| std::io::Error::other("drop target is invalid"))?;
                let source_index = source_order_state(&state, scene.as_str(), &source_id)?.0;
                let requested = if mode == 2 {
                    target_index.saturating_add(1)
                } else {
                    target_index
                };
                let target_index = if source_parent == destination_path && source_index < requested
                {
                    requested.saturating_sub(1)
                } else {
                    requested
                };
                (profile, scene, destination_path.join("/"), target_index)
            }
        };
        let item_id = source_id
            .rsplit('/')
            .next()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| std::io::Error::other("source target is invalid"))?;
        let destination_path = if destination.is_empty() {
            Vec::new()
        } else {
            destination.split('/').map(str::to_owned).collect()
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveSceneItemToParent {
                profile,
                scene,
                item: source_id,
                destination: destination_path,
                target_index,
            }))?;
        let new_target = if destination.is_empty() {
            item_id.clone()
        } else {
            format!("{destination}/{item_id}")
        };
        state.borrow_mut().dispatch(UiCommand::SelectSource {
            id: new_target.clone(),
        })?;
        Ok(new_target)
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(_) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Move source failed: {error}").into()),
    }
}

pub(crate) fn remove_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        if locked {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let command = ProjectCommand::RemoveSceneItem {
            profile,
            scene,
            item: source_id.to_owned(),
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Remove source failed: {error}").into()),
    }
}

pub(crate) fn toggle_source_visibility_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, visible, _) = source_display_state(&state.borrow(), source_id)?;
        let command = ProjectCommand::SetSceneItemVisibility {
            profile,
            scene,
            item: source_id.to_owned(),
            visible: !visible,
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Source visibility failed: {error}").into()),
    }
}

pub(crate) fn toggle_source_locked_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        let command = ProjectCommand::SetSceneItemLocked {
            profile,
            scene,
            item: source_id.to_owned(),
            locked: !locked,
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Source lock failed: {error}").into()),
    }
}

pub(crate) fn reset_source_transform_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
) {
    update_source_transform_and_refresh(weak, state, surface, source_id, |_: FrameTransform| {
        Ok(FrameTransform::IDENTITY)
    });
}

pub(crate) fn flip_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
    horizontal: bool,
) {
    update_source_transform_and_refresh(weak, state, surface, source_id, move |transform| {
        FrameTransform::new(
            transform.scale_x_milli(),
            transform.scale_y_milli(),
            transform.translate_x(),
            transform.translate_y(),
            if horizontal {
                !transform.flip_x()
            } else {
                transform.flip_x()
            },
            if horizontal {
                transform.flip_y()
            } else {
                !transform.flip_y()
            },
            transform.opacity(),
        )?
        .with_rotation_milli_degrees(transform.rotation_milli_degrees())?
        .with_crop(
            transform.crop_left(),
            transform.crop_top(),
            transform.crop_right(),
            transform.crop_bottom(),
        )
        .map_err(Into::into)
    });
}

/// Applies one bounded Transform submenu command to a named scene item.
pub(crate) fn transform_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
    action: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let command = CanvasTransformCommand::from_action(action)
            .ok_or_else(|| std::io::Error::other("unknown transform command"))?;
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        if locked {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let (transform, canvas) = {
            let state = state.borrow();
            let project = state.project_session().project();
            let transform = project
                .active_profile_spec()
                .and_then(|profile| canvas_item_for_target(profile, scene.as_str(), source_id))
                .map(obs_rs_project::SceneItemSpec::transform)
                .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
            let surface = surface.borrow();
            (transform, (surface.format.width(), surface.format.height()))
        };
        let transform = transform_for_command(transform, command, canvas);
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(scene_item_transform_command(
                profile, scene, source_id, transform,
            )))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Transform failed: {error}").into()),
    }
}

fn update_source_transform_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
    update: impl FnOnce(FrameTransform) -> Result<FrameTransform, Box<dyn Error>>,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        if locked {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let transform = {
            let state = state.borrow();
            let project = state.project_session().project();
            project
                .active_profile_spec()
                .and_then(|profile| canvas_item_for_target(profile, scene.as_str(), source_id))
                .map(obs_rs_project::SceneItemSpec::transform)
                .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?
        };
        let transform = update(transform)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(scene_item_transform_command(
                profile, scene, source_id, transform,
            )))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Source transform failed: {error}").into()),
    }
}

pub(crate) fn duplicate_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, _) = source_display_state(&state.borrow(), source_id)?;
        // OBS selects the newly-created top-level item after Duplicate. Keep
        // the pre-edit IDs so the selection follows the command's fresh item
        // rather than guessing from the source name (which may already have
        // copies). Nested rows intentionally remain outside the canvas
        // selection model, so their existing selection is preserved.
        let before_root_items = if group_target(source_id).is_none() {
            Some(root_item_ids(&state.borrow(), scene.as_str())?)
        } else {
            None
        };
        let scene_for_selection = scene.clone();
        let command = ProjectCommand::DuplicateSceneItem {
            profile,
            scene,
            item: source_id.to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        if let Some(before_root_items) = before_root_items {
            let duplicated = {
                let state = state.borrow();
                newly_added_root_item(&state, scene_for_selection.as_str(), &before_root_items)?
            };
            state
                .borrow_mut()
                .dispatch(UiCommand::SelectSource { id: duplicated })?;
        }
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Duplicate source failed: {error}").into()),
    }
}

pub(crate) fn apply_source_name_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    target: &str,
    name: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        if target.trim().is_empty() {
            return Err(std::io::Error::other("no source or group rename target is set").into());
        }
        let command = {
            let state = state.borrow();
            let project = state.project_session().project();
            let profile = project
                .active_profile_spec()
                .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
            let scene_id = state
                .preview_scene()
                .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
            let item = canvas_item_for_target(profile, scene_id, target)
                .or_else(|| {
                    profile
                        .scene(scene_id)
                        .and_then(|scene| item_for_target(scene, target))
                })
                .ok_or_else(|| {
                    std::io::Error::other("rename target is not in the preview scene")
                })?;
            if item.is_group() {
                ProjectCommand::SetGroupName {
                    profile: project.active_profile().to_string(),
                    scene: scene_id.to_owned(),
                    group_path: target.split('/').map(str::to_owned).collect(),
                    name: name.to_owned(),
                }
            } else if item.is_source() {
                ProjectCommand::SetSourceName {
                    profile: project.active_profile().to_string(),
                    source: item.source_id().to_string(),
                    name: name.to_owned(),
                }
            } else {
                return Err(std::io::Error::other("rename target is not a source or group").into());
            }
        };
        state.borrow_mut().dispatch(UiCommand::Project(command))?;
        Ok(())
    })();
    match result {
        Ok(()) => refresh_ui(ui, state, surface),
        Err(error) => ui.set_status_message(format!("Rename source failed: {error}").into()),
    }
}

fn group_target(target: &str) -> Option<(Vec<String>, String)> {
    let mut parts = target.split('/').map(str::to_owned).collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().any(String::is_empty) {
        return None;
    }
    let item = parts.pop()?;
    Some((parts, item))
}

fn root_item_ids(state: &DesktopState, scene_id: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let project = state.project_session().project();
    let scene = project
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .ok_or_else(|| std::io::Error::other("source scene is missing"))?;
    Ok(scene
        .items()
        .iter()
        .map(|item| item.id().to_string())
        .collect())
}

fn newly_added_root_item(
    state: &DesktopState,
    scene_id: &str,
    before: &[String],
) -> Result<String, Box<dyn Error>> {
    let project = state.project_session().project();
    let scene = project
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .ok_or_else(|| std::io::Error::other("source scene is missing after duplicate"))?;
    let mut added = scene
        .items()
        .iter()
        .filter(|item| !before.iter().any(|id| id == item.id().as_str()))
        .map(|item| item.id().to_string());
    let duplicated = added
        .next()
        .ok_or_else(|| std::io::Error::other("duplicate did not add a root source item"))?;
    if added.next().is_some() {
        return Err(std::io::Error::other("duplicate added more than one root source item").into());
    }
    Ok(duplicated)
}

pub(crate) fn scene_item_transform_command(
    profile: String,
    scene: String,
    target: &str,
    transform: FrameTransform,
) -> ProjectCommand {
    if target.contains('/') {
        // A slash-addressed target may cross either embedded groups or a
        // Scene source. The atomic path adapter owns that distinction and
        // writes a nested Scene leaf into its owning scene.
        ProjectCommand::SetSceneItemTransforms {
            profile,
            scene,
            items: vec![(target.to_owned(), transform)],
        }
    } else {
        ProjectCommand::SetSceneItemTransform {
            profile,
            scene,
            item: target.to_owned(),
            transform,
        }
    }
}

fn group_items_for_path<'a>(scene: &'a SceneSpec, path: &[String]) -> Option<&'a [SceneItemSpec]> {
    let mut items = scene.items();
    for group_id in path {
        let group_item = items.iter().find(|item| item.id().as_str() == group_id)?;
        items = group_item.group()?.items();
    }
    Some(items)
}

fn flattened_parent_items<'a>(
    profile: &'a Profile,
    scene_id: &str,
    path: &[String],
) -> Option<&'a [SceneItemSpec]> {
    let mut items = profile.scene(scene_id)?.items();
    for parent_id in path {
        let parent = items.iter().find(|item| item.id().as_str() == parent_id)?;
        items = if let Some(group) = parent.group() {
            group.items()
        } else {
            let child_scene = parent.scene_id()?;
            profile.scene(child_scene)?.items()
        };
    }
    Some(items)
}

pub(crate) fn item_for_target<'a>(scene: &'a SceneSpec, target: &str) -> Option<&'a SceneItemSpec> {
    if let Some((group_path, item_id)) = group_target(target) {
        group_items_for_path(scene, &group_path)?
            .iter()
            .find(|item| item.id().as_str() == item_id)
    } else {
        scene.item(target)
    }
}

fn source_order_state(
    state: &DesktopState,
    scene_id: &str,
    target: &str,
) -> Result<(usize, usize), Box<dyn Error>> {
    let profile = state
        .project_session()
        .project()
        .active_profile_spec()
        .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
    let scene = profile
        .scene(scene_id)
        .ok_or_else(|| std::io::Error::other("preview scene is missing"))?;
    let (group_path, item_id) =
        group_target(target).map_or((None, target.to_owned()), |(path, item)| (Some(path), item));
    let items = match group_path.as_deref() {
        Some(path) => flattened_parent_items(profile, scene_id, path)
            .ok_or_else(|| std::io::Error::other("source parent is not in the preview scene"))?,
        None => scene.items(),
    };
    let index = items
        .iter()
        .position(|item| item.id().as_str() == item_id)
        .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
    Ok((index, items.len()))
}

fn source_display_state(
    state: &DesktopState,
    source_id: &str,
) -> Result<(String, String, bool, bool), Box<dyn Error>> {
    let project = state.project_session().project();
    let profile_id = project.active_profile().to_string();
    let scene_id = state
        .preview_scene()
        .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
    let profile = project
        .active_profile_spec()
        .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
    let item = canvas_item_for_target(profile, scene_id, source_id)
        .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
    let locked = canvas_target_is_locked_in_profile(profile, scene_id, source_id);
    Ok((profile_id, scene_id.to_owned(), item.visible(), locked))
}

pub(crate) fn apply_source_settings_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    document: &str,
) {
    match selected_target(&state.borrow()) {
        Some(target) => apply_source_settings_to(ui, state, surface, &target, document),
        None => ui.set_status_message("Source settings failed: no source is selected".into()),
    }
}

/// Writes settings to one named source, whatever is selected right now.
///
/// Anything that finishes asynchronously — a portal handshake, a device probe —
/// must come through here with the source it started on. Resolving the current
/// selection when a background answer arrives writes a screen capture's portal
/// token onto whichever source the user happened to click in the meantime.
pub(crate) fn apply_source_settings_to(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    target: &SourceTarget,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let settings = Config::parse(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceSettings {
                profile: target.profile.clone(),
                source: target.source.clone(),
                settings,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source settings failed: {error}").into());
    } else {
        refresh_ui(ui, state, surface);
    }
}

/// Writes a transform to one named scene item, whatever is selected right now.
///
/// A pointer gesture and an open dialog both start on a specific item and
/// finish some time later. Both commit through here with the item they started
/// on, so a selection change in between moves nothing unexpected.
pub(crate) fn apply_source_transform_to(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    target: &SceneItemTarget,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        if scene_item_target_is_locked(&state.borrow(), target) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let transform = parse_source_transform(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(scene_item_transform_command(
                target.profile.clone(),
                target.scene.clone(),
                target.item.as_str(),
                transform,
            )))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source transform failed: {error}").into());
    } else {
        refresh_ui(ui, state, surface);
    }
}

/// Writes a bounded set of scene-item transforms as one undoable project edit.
///
/// Canvas group gestures carry their original item IDs so a selection change
/// while the pointer is down cannot redirect the commit to a different source.
pub(crate) fn apply_source_transforms_to(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    profile: &str,
    scene: &str,
    transforms: Vec<(String, FrameTransform)>,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let local_transforms = {
            let state_ref = state.borrow();
            let profile_spec = state_ref
                .project_session()
                .project()
                .profile(profile)
                .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
            let scene_spec = profile_spec
                .scene(scene)
                .ok_or_else(|| std::io::Error::other("preview scene is missing"))?;
            transforms
                .into_iter()
                .map(|(target, transform)| {
                    let local = crate::callbacks::canvas::local_transform_for_canvas_item(
                        profile_spec,
                        scene_spec,
                        target.as_str(),
                        transform,
                    )
                    .ok_or_else(|| {
                        std::io::Error::other(format!(
                            "nested canvas transform is not representable for {target}"
                        ))
                    })?;
                    Ok((target, local))
                })
                .collect::<Result<Vec<_>, Box<dyn Error>>>()?
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransforms {
                profile: profile.to_owned(),
                scene: scene.to_owned(),
                items: local_transforms,
            }))
            .map_err(|error| Box::new(error) as Box<dyn Error>)
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source transform failed: {error}").into());
    } else {
        refresh_ui(ui, state, surface);
    }
}

fn parse_source_transform(document: &str) -> Result<FrameTransform, Box<dyn Error>> {
    let values = document.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 7 && values.len() != 11 && values.len() != 12 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "transform needs 7, 11, or 12 comma-separated values",
        )
        .into());
    }
    let flip_x = parse_transform_flag(values[4], "flip-x")?;
    let flip_y = parse_transform_flag(values[5], "flip-y")?;
    let transform = FrameTransform::new(
        values[0].parse()?,
        values[1].parse()?,
        values[2].parse()?,
        values[3].parse()?,
        flip_x,
        flip_y,
        values[6].parse()?,
    )?;
    let transform = if values.len() == 12 {
        transform.with_rotation_milli_degrees(values[11].parse()?)?
    } else {
        transform
    };
    if values.len() == 7 {
        return Ok(transform);
    }
    Ok(transform.with_crop(
        values[7].parse()?,
        values[8].parse()?,
        values[9].parse()?,
        values[10].parse()?,
    )?)
}

fn parse_transform_flag(value: &str, field: &str) -> Result<bool, Box<dyn Error>> {
    match value {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{field} must be 0, 1, false, or true"),
        )
        .into()),
    }
}

pub(crate) fn source_transform_document(transform: FrameTransform) -> String {
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{}",
        transform.scale_x_milli(),
        transform.scale_y_milli(),
        transform.translate_x(),
        transform.translate_y(),
        u8::from(transform.flip_x()),
        u8::from(transform.flip_y()),
        transform.opacity(),
        transform.crop_left(),
        transform.crop_top(),
        transform.crop_right(),
        transform.crop_bottom(),
        transform.rotation_milli_degrees()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_document_round_trips_crop_and_rotation() {
        let transform = FrameTransform::new(1_250, 900, 12, -8, true, false, 180)
            .expect("transform")
            .with_rotation_milli_degrees(-12_500)
            .expect("rotation")
            .with_crop(4, 5, 6, 7)
            .expect("crop");

        assert_eq!(
            parse_source_transform(&source_transform_document(transform))
                .expect("serialized transform"),
            transform
        );
    }

    #[test]
    fn legacy_transform_documents_keep_zero_rotation() {
        let transform = parse_source_transform("1000,1000,0,0,0,0,255").expect("legacy transform");
        assert_eq!(transform, FrameTransform::IDENTITY);
    }
}
