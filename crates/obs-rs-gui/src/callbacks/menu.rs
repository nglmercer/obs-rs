//! The menu-bar actions that are not scene, source, output, or dock edits.
//!
//! Everything reachable from the navigation bar has one handler here: history
//! (undo/redo), session lifecycle (new project, quit), scene collections,
//! projectors, layout reset, and the informational entries. Keeping them
//! together means the bar has no entry that dispatches into nothing, which is
//! the failure mode the menus previously had.

use std::{
    cell::RefCell,
    error::Error,
    path::{Path, PathBuf},
    rc::Rc,
};

use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, VecModel};

use crate::settings::{
    scale_window_dimension, FloatingGeometry, ProjectorGeometry, ProjectorKind, ProjectorMonitor,
    ProjectorTarget,
};
use crate::{
    callbacks::docks::{clamp_window_position, current_desktop_bounds, DockController},
    dispatch_and_refresh,
    fixtures::{desktop_bounds, screen_monitors, DesktopBounds, MonitorChoice},
    initial_project,
    preview_worker::{SceneProjectorTarget, SourceProjectorTarget},
    project_store, refresh_ui, source_target, MainWindow, MonitorRow, PreviewSurface, ProfileRow,
    ProjectorWindow,
};

/// Extension every scene-collection document uses.
///
/// Collections are ordinary project files; the distinct extension is what lets
/// the picker list them without also offering unrelated text files that happen
/// to sit in the same folder.
const COLLECTION_EXTENSION: &str = "obsrproj";

/// Directory, relative to the configured project file, that holds collections.
const COLLECTION_DIRECTORY: &str = "collections";

/// Longest collection name accepted from the dialog.
///
/// The name becomes a file name, so it is bounded and stripped of separators
/// rather than passed to the filesystem as typed.
const MAX_COLLECTION_NAME: usize = 64;

#[path = "menu_collections.rs"]
mod collections;
#[path = "menu_projectors.rs"]
mod projectors;
#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;

#[allow(unused_imports)]
use collections::{
    collection_file_name, collections_root, create_collection, discover_collections,
    duplicate_collection, export_collection, import_collection, install_collections,
    rename_collection,
};
pub(crate) use projectors::ProjectorController;
#[allow(unused_imports)]
use projectors::{install_projectors, monitor_containing_point, projector_monitor_rows_for};

/// Installs every menu-bar action and returns the projector controller.
pub(crate) fn install_menu_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    docks: &Rc<DockController>,
) -> Rc<ProjectorController> {
    let projectors = Rc::new(ProjectorController::new());

    install_history(ui, state, surface);
    install_session(ui, state, surface, docks);
    install_collections(ui, state, surface);
    install_projectors(ui, state, &projectors);
    install_information(ui);

    refresh_menu_models(ui, state);
    projectors
}

/// Republishes the menu-bar models that do not come from the project itself.
///
/// The collection list is filesystem state, so it is rebuilt when an action
/// could have changed it rather than polled on the animation timer.
pub(crate) fn refresh_menu_models(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let path = ui.get_project_path().to_string();
    let rows = discover_collections(&path);
    ui.set_active_collection(
        rows.iter()
            .find(|row| row.active)
            .map_or_else(|| "—".into(), |row| row.name.clone()),
    );
    ui.set_collection_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.set_app_platform(crate::platform_capture_summary().into());
    let state = state.borrow();
    ui.set_can_undo(state.can_undo());
    ui.set_can_redo(state.can_redo());
}

fn install_history(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let undo_state = Rc::clone(state);
    let undo_surface = Rc::clone(surface);
    ui.on_undo_edit(move || {
        dispatch_and_refresh(&weak, &undo_state, &undo_surface, UiCommand::Undo);
    });

    let weak = ui.as_weak();
    let redo_state = Rc::clone(state);
    let redo_surface = Rc::clone(surface);
    ui.on_redo_edit(move || {
        dispatch_and_refresh(&weak, &redo_state, &redo_surface, UiCommand::Redo);
    });
}

fn install_session(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    docks: &Rc<DockController>,
) {
    let weak = ui.as_weak();
    let new_state = Rc::clone(state);
    let new_surface = Rc::clone(surface);
    ui.on_new_project(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match initial_project() {
            Ok(project) => {
                // `DesktopState::new` also resets the history, which is what a
                // new document should do: undo must not reach the old project.
                *new_state.borrow_mut() = DesktopState::new(project);
                new_state
                    .borrow_mut()
                    .set_project_selection_key(ui.get_project_path().as_str());
                refresh_ui(&ui, &new_state, &new_surface);
                refresh_menu_models(&ui, &new_state);
                ui.set_status_message("Started a new project".into());
            }
            Err(error) => ui.set_status_message(format!("New project failed: {error}").into()),
        }
    });

    // Leaving through the menu takes the same path as closing the window, so
    // the layout and project are persisted by `main` either way.
    ui.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });

    let weak = ui.as_weak();
    let docks = Rc::clone(docks);
    ui.on_reset_dock_layout(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        docks.reset_floating(&ui);
        crate::settings::apply_default_layout(&ui);
        let tree = crate::dock_tree::DockNode::from_legacy(
            &[1, 0, 2, 3, 4, 5],
            &[1.0, 1.0, 1.85, 1.0, 1.4, 1.1],
        )
        .expect("the built-in dock layout must be valid");
        docks.replace_tree(&tree, &ui);
        ui.set_status_message("Dock layout reset".into());
    });
}

fn install_information(ui: &MainWindow) {
    let weak = ui.as_weak();
    ui.on_open_documentation(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        ui.set_status_message(match open_documentation() {
            Ok(path) => format!("Opened {}", path.display()).into(),
            Err(error) => format!("Documentation: {error}").into(),
        });
    });
}

/// Locates the shipped README and hands it to the desktop's file opener.
///
/// The path is resolved by walking up from the working directory so the action
/// works from a checkout and from an installed layout, and a missing opener is
/// reported as text rather than leaving the menu entry looking broken.
fn open_documentation() -> Result<PathBuf, Box<dyn Error>> {
    let path = documentation_path().ok_or_else(|| {
        std::io::Error::other("no README.md was found next to the working directory")
    })?;
    let status = std::process::Command::new("xdg-open")
        .arg(&path)
        .status()
        .map_err(|error| std::io::Error::other(format!("xdg-open is unavailable: {error}")))?;
    if !status.success() {
        return Err(std::io::Error::other(format!("xdg-open refused {}", path.display())).into());
    }
    Ok(path)
}

fn documentation_path() -> Option<PathBuf> {
    let mut directory = std::env::current_dir().ok()?;
    loop {
        let candidate = directory.join("README.md");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !directory.pop() {
            return None;
        }
    }
}
