use std::{cell::RefCell, error::Error, rc::Rc};

use slint::Weak;

use crate::{refresh_ui, MainWindow, PreviewRenderer};
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform};
use obs_rs_project::ProjectCommand;
use obs_rs_ui::{DesktopState, UiCommand};

pub(crate) fn remove_scene_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Remove scene failed: {error}").into()),
    }
}

pub(crate) fn move_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
        let target_index = {
            let state = state.borrow();
            let project = state.project_session().project();
            let scene = project
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene.as_str()));
            let source_index = scene
                .and_then(|scene| {
                    scene
                        .sources()
                        .iter()
                        .position(|source| source.id().as_str() == source_id)
                })
                .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
            let target = i32::try_from(source_index)
                .unwrap_or(i32::MAX)
                .saturating_add(delta);
            usize::try_from(target)
                .map_err(|_| std::io::Error::other("source cannot move above the scene"))?
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveSource {
                profile,
                scene,
                source: source_id.to_owned(),
                target_index,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Move source failed: {error}").into()),
    }
}

pub(crate) fn move_source_to_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
        let source_count = {
            let state = state.borrow();
            let project = state.project_session().project();
            let scene = project
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene.as_str()))
                .ok_or_else(|| std::io::Error::other("preview scene is missing"))?;
            scene.sources().len()
        };
        let target_index = usize::try_from(target_index)
            .map_err(|_| std::io::Error::other("source order is invalid"))?
            .min(source_count.saturating_sub(1));
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveSource {
                profile,
                scene,
                source: source_id.to_owned(),
                target_index,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Move source failed: {error}").into()),
    }
}

pub(crate) fn remove_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSource {
                profile,
                scene,
                source: source_id.to_owned(),
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Remove source failed: {error}").into()),
    }
}

pub(crate) fn toggle_source_visibility_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, visible, _) = source_display_state(&state.borrow(), source_id)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceVisibility {
                profile,
                scene,
                source: source_id.to_owned(),
                visible: !visible,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Source visibility failed: {error}").into()),
    }
}

pub(crate) fn toggle_source_locked_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, locked) = source_display_state(&state.borrow(), source_id)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceLocked {
                profile,
                scene,
                source: source_id.to_owned(),
                locked: !locked,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Source lock failed: {error}").into()),
    }
}

pub(crate) fn reset_source_transform_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    source_id: &str,
) {
    update_source_transform_and_refresh(weak, state, renderer, source_id, |_: FrameTransform| {
        Ok(FrameTransform::IDENTITY)
    });
}

pub(crate) fn flip_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    source_id: &str,
    horizontal: bool,
) {
    update_source_transform_and_refresh(weak, state, renderer, source_id, move |transform| {
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
        .with_crop(
            transform.crop_left(),
            transform.crop_top(),
            transform.crop_right(),
            transform.crop_bottom(),
        )
        .map_err(Into::into)
    });
}

fn update_source_transform_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
                .and_then(|scene| scene.source(source_id))
                .map(obs_rs_project::SourceSpec::transform)
                .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceTransform {
                profile,
                scene,
                source: source_id.to_owned(),
                transform: update(transform)?,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Source transform failed: {error}").into()),
    }
}

pub(crate) fn duplicate_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    source_id: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, _, _) = source_display_state(&state.borrow(), source_id)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::DuplicateSource {
                profile,
                scene,
                source: source_id.to_owned(),
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Duplicate source failed: {error}").into()),
    }
}

pub(crate) fn apply_source_name_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    name: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, source) = selected_source_context(&state.borrow())?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceName {
                profile,
                scene,
                source,
                name: name.to_owned(),
            }))?;
        Ok(())
    })();
    match result {
        Ok(()) => refresh_ui(ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Rename source failed: {error}").into()),
    }
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
    let source = scene
        .source(source_id)
        .ok_or_else(|| std::io::Error::other("source is not in the preview scene"))?;
    Ok((
        profile_id,
        scene_id.to_owned(),
        source.visible(),
        source.locked(),
    ))
}

pub(crate) fn apply_source_settings_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    document: &str,
) {
    let (profile, scene, source) = {
        let state = state.borrow();
        (
            state
                .project_session()
                .project()
                .active_profile()
                .to_string(),
            state.preview_scene().map(str::to_owned),
            state.selected_source().map(str::to_owned),
        )
    };
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = scene.ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
        let source = source.ok_or_else(|| std::io::Error::other("no source is selected"))?;
        let settings = Config::parse(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceSettings {
                profile,
                scene,
                source,
                settings,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source settings failed: {error}").into());
    } else {
        refresh_ui(ui, state, renderer);
    }
}

pub(crate) fn apply_source_transform_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, source) = selected_source_context(&state.borrow())?;
        let transform = parse_source_transform(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceTransform {
                profile,
                scene,
                source,
                transform,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source transform failed: {error}").into());
    } else {
        refresh_ui(ui, state, renderer);
    }
}

pub(crate) fn apply_source_filters_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, source) = selected_source_context(&state.borrow())?;
        let filters = parse_source_filters(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceFilters {
                profile,
                scene,
                source,
                filters,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source filters failed: {error}").into());
    } else {
        refresh_ui(ui, state, renderer);
    }
}

fn selected_source_context(
    state: &DesktopState,
) -> Result<(String, String, String), Box<dyn Error>> {
    let profile = state
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let scene = state
        .preview_scene()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
    let source = state
        .selected_source()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("no source is selected"))?;
    Ok((profile, scene, source))
}

fn parse_source_transform(document: &str) -> Result<FrameTransform, Box<dyn Error>> {
    let values = document.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 7 && values.len() != 11 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "transform needs 7 or 11 comma-separated values",
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

fn parse_source_filters(document: &str) -> Result<Vec<FrameFilter>, Box<dyn Error>> {
    let document = document.trim();
    if document.is_empty() {
        return Ok(Vec::new());
    }
    document
        .split(',')
        .map(str::trim)
        .map(|filter| {
            if filter == "gray" || filter == "grayscale" {
                return Ok(FrameFilter::Grayscale);
            }
            if let Some(value) = filter.strip_prefix("brightness:") {
                return Ok(FrameFilter::Brightness {
                    milli: value.trim().parse()?,
                });
            }
            if let Some(value) = filter.strip_prefix("opacity:") {
                return Ok(FrameFilter::Opacity(value.trim().parse()?));
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown filter: {filter}"),
            )
            .into())
        })
        .collect()
}

pub(crate) fn source_transform_document(transform: FrameTransform) -> String {
    format!(
        "{},{},{},{},{},{},{},{},{},{},{}",
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
        transform.crop_bottom()
    )
}

pub(crate) fn source_filters_document(filters: &[FrameFilter]) -> String {
    filters
        .iter()
        .map(|filter| match filter {
            FrameFilter::Grayscale => "gray".to_owned(),
            FrameFilter::Brightness { milli } => format!("brightness:{milli}"),
            FrameFilter::Opacity(opacity) => format!("opacity:{opacity}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}
