use std::{cell::RefCell, error::Error, rc::Rc};

use slint::Weak;

use obs_rs_config::Config;
use obs_rs_media::FrameTransform;
use obs_rs_project::{ProjectCommand, SceneItemDuplicateMode, SceneItemSpec, SceneSpec};
use obs_rs_ui::{DesktopState, UiCommand};

use crate::{
    callbacks::canvas::{transform_for_command, CanvasTransformCommand},
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
        let command = if let Some((group_path, item)) = group_target(source_id) {
            ProjectCommand::MoveGroupItem {
                profile,
                scene,
                group_path,
                item,
                target_index,
            }
        } else {
            ProjectCommand::MoveSceneItem {
                profile,
                scene,
                item: source_id.to_owned(),
                target_index,
            }
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
        let command = if let Some((group_path, item)) = group_target(source_id) {
            ProjectCommand::MoveGroupItem {
                profile,
                scene,
                group_path,
                item,
                target_index,
            }
        } else {
            ProjectCommand::MoveSceneItem {
                profile,
                scene,
                item: source_id.to_owned(),
                target_index,
            }
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
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
                profile,
                scene,
                item: source_id.to_owned(),
            }))?;
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
        let command = if let Some((group_path, item)) = group_target(source_id) {
            ProjectCommand::SetGroupItemVisibility {
                profile,
                scene,
                group_path,
                item,
                visible: !visible,
            }
        } else {
            ProjectCommand::SetSceneItemVisibility {
                profile,
                scene,
                item: source_id.to_owned(),
                visible: !visible,
            }
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
        let command = if let Some((group_path, item)) = group_target(source_id) {
            ProjectCommand::SetGroupItemLocked {
                profile,
                scene,
                group_path,
                item,
                locked: !locked,
            }
        } else {
            ProjectCommand::SetSceneItemLocked {
                profile,
                scene,
                item: source_id.to_owned(),
                locked: !locked,
            }
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
                .and_then(|profile| profile.scene(scene.as_str()))
                .and_then(|scene| scene.item(source_id))
                .map(obs_rs_project::SceneItemSpec::transform)
                .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
            let surface = surface.borrow();
            (transform, (surface.format.width(), surface.format.height()))
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
                profile,
                scene,
                item: source_id.to_owned(),
                transform: transform_for_command(transform, command, canvas),
            }))?;
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
                .and_then(|profile| profile.scene(scene.as_str()))
                .and_then(|scene| scene.item(source_id))
                .map(obs_rs_project::SceneItemSpec::transform)
                .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
                profile,
                scene,
                item: source_id.to_owned(),
                transform: update(transform)?,
            }))?;
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
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::DuplicateSceneItem {
                profile,
                scene,
                item: source_id.to_owned(),
                mode: SceneItemDuplicateMode::DuplicateSource,
            }))?;
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
    name: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, _, _, source) = selected_source_context(&state.borrow())?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceName {
                profile,
                source,
                name: name.to_owned(),
            }))?;
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

fn group_items_for_path<'a>(scene: &'a SceneSpec, path: &[String]) -> Option<&'a [SceneItemSpec]> {
    let mut items = scene.items();
    for group_id in path {
        let group_item = items.iter().find(|item| item.id().as_str() == group_id)?;
        items = group_item.group()?.items();
    }
    Some(items)
}

fn item_for_target<'a>(scene: &'a SceneSpec, target: &str) -> Option<&'a SceneItemSpec> {
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
    let scene = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .ok_or_else(|| std::io::Error::other("preview scene is missing"))?;
    let (group_path, item_id) =
        group_target(target).map_or((None, target.to_owned()), |(path, item)| (Some(path), item));
    let items = group_path
        .as_deref()
        .and_then(|path| group_items_for_path(scene, path))
        .unwrap_or_else(|| scene.items());
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
    let scene = profile
        .scene(scene_id)
        .ok_or_else(|| std::io::Error::other("preview scene is missing"))?;
    let item = item_for_target(scene, source_id)
        .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
    Ok((
        profile_id,
        scene_id.to_owned(),
        item.visible(),
        item.locked(),
    ))
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
    target: &SourceTarget,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        if is_locked(&state.borrow(), target) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source is locked",
            )
            .into());
        }
        let transform = parse_source_transform(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
                profile: target.profile.clone(),
                scene: target.scene.clone(),
                item: target.item.clone(),
                transform,
            }))?;
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
    let result =
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransforms {
                profile: profile.to_owned(),
                scene: scene.to_owned(),
                items: transforms,
            }));
    if let Err(error) = result {
        ui.set_status_message(format!("Source transform failed: {error}").into());
    } else {
        refresh_ui(ui, state, surface);
    }
}

/// Returns whether a target's scene item is protected from editing.
fn is_locked(state: &DesktopState, target: &SourceTarget) -> bool {
    state
        .project_session()
        .project()
        .profile(target.profile.as_str())
        .and_then(|profile| profile.scene(target.scene.as_str()))
        .and_then(|scene| scene.item(target.item.as_str()))
        .is_some_and(obs_rs_project::SceneItemSpec::locked)
}

/// A stable reference to one scene item and the source definition it shows.
///
/// Anything that outlives the click that started it — a dialog the user leaves
/// open, a portal handshake, a pointer gesture — has to carry one of these. The
/// alternative is asking "what is selected?" when the work finishes, which is a
/// different answer by then often enough to matter: it is how a screen
/// capture's portal token ends up on a camera.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceTarget {
    pub(crate) profile: String,
    pub(crate) scene: String,
    /// The scene item, which is what a transform and the dock selection name.
    pub(crate) item: String,
    /// The profile-wide source definition, which is what settings belong to.
    pub(crate) source: String,
}

/// Resolves one scene item in the preview scene to a stable target.
pub(crate) fn source_target(state: &DesktopState, item: &str) -> Option<SourceTarget> {
    let project = state.project_session().project();
    let scene = state.preview_scene()?.to_owned();
    let source = project
        .active_profile_spec()?
        .scene(scene.as_str())?
        .item(item)?;
    let source = source.is_source().then(|| source.source_id().to_string())?;
    Some(SourceTarget {
        profile: project.active_profile().to_string(),
        scene,
        item: item.to_owned(),
        source,
    })
}

/// Resolves the selected scene item to a stable target.
pub(crate) fn selected_target(state: &DesktopState) -> Option<SourceTarget> {
    source_target(state, state.selected_source()?)
}

/// Returns a target's settings document from the live project.
pub(crate) fn target_settings_document(
    state: &Rc<RefCell<DesktopState>>,
    target: &SourceTarget,
) -> Option<String> {
    let state = state.borrow();
    let session = state.project_session();
    let profile = session.project().profile(target.profile.as_str())?;
    Some(
        profile
            .source(target.source.as_str())?
            .settings()
            .serialize(),
    )
}

fn selected_source_context(
    state: &DesktopState,
) -> Result<(String, String, String, String), Box<dyn Error>> {
    let target = selected_target(state).ok_or_else(|| {
        std::io::Error::other(if state.preview_scene().is_none() {
            "no preview scene is selected"
        } else if state.selected_source().is_none() {
            "no source is selected"
        } else {
            "selected source item is missing"
        })
    })?;
    Ok((target.profile, target.scene, target.item, target.source))
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
