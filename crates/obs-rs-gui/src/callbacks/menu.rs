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
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::settings::{scale_window_dimension, FloatingGeometry, ProjectorGeometry, ProjectorKind};
use crate::{
    callbacks::docks::{clamp_window_position, current_desktop_bounds, DockController},
    dispatch_and_refresh, initial_project,
    preview_worker::{SceneProjectorTarget, SourceProjectorTarget},
    project_store, refresh_ui, source_target, MainWindow, PreviewSurface, ProfileRow,
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

/// Owns the open projector windows.
///
/// A projector renders nothing itself: it mirrors the composited image the
/// studio window already produced, so a projector can never show a different
/// frame from the one the operator is watching.
pub(crate) struct ProjectorController {
    program: RefCell<Option<ProjectorWindow>>,
    preview: RefCell<Option<ProjectorWindow>>,
    multiview: RefCell<Option<ProjectorWindow>>,
    source: RefCell<Option<ProjectorWindow>>,
    scene: RefCell<Option<ProjectorWindow>>,
    source_target: RefCell<Option<SourceProjectorTarget>>,
    scene_target: RefCell<Option<SceneProjectorTarget>>,
    geometry: RefCell<Vec<ProjectorGeometry>>,
}

#[derive(Clone, Copy)]
enum ProjectorFeed {
    Program,
    Preview,
    Multiview,
    Source,
    Scene,
}

impl ProjectorFeed {
    const fn kind(self) -> ProjectorKind {
        match self {
            Self::Program => ProjectorKind::Program,
            Self::Preview => ProjectorKind::Preview,
            Self::Multiview => ProjectorKind::Multiview,
            Self::Source => ProjectorKind::Source,
            Self::Scene => ProjectorKind::Scene,
        }
    }

    const fn is_fullscreen(self) -> bool {
        matches!(self, Self::Program | Self::Multiview)
    }
}

impl ProjectorController {
    /// Returns whether a program projector needs the program canvas rendered.
    ///
    /// Single-canvas editing skips the program render to save a full-size
    /// composite per frame, so an open program projector has to ask for it back.
    pub(crate) fn wants_program(&self) -> bool {
        self.slot(ProjectorFeed::Program).borrow().is_some()
    }

    /// Returns whether a preview projector needs the preview feed rendered.
    pub(crate) fn wants_preview(&self) -> bool {
        self.slot(ProjectorFeed::Preview).borrow().is_some()
    }

    /// Returns whether a multiview projector needs the bounded scene grid
    /// rendered, even when the main window is in another view mode.
    pub(crate) fn wants_multiview(&self) -> bool {
        self.slot(ProjectorFeed::Multiview).borrow().is_some()
    }

    /// Returns the selected source target while its projector is open.
    pub(crate) fn source_target(&self) -> Option<SourceProjectorTarget> {
        self.slot(ProjectorFeed::Source)
            .borrow()
            .as_ref()
            .and(self.source_target.borrow().as_ref())
            .cloned()
    }

    /// Returns the stable scene target while its projector is open.
    pub(crate) fn scene_target(&self) -> Option<SceneProjectorTarget> {
        self.slot(ProjectorFeed::Scene)
            .borrow()
            .as_ref()
            .and(self.scene_target.borrow().as_ref())
            .cloned()
    }

    /// Loads bounded window geometry captured from the previous session.
    pub(crate) fn restore_geometry(&self, geometry: &[ProjectorGeometry]) {
        let mut stored = self.geometry.borrow_mut();
        stored.clear();
        for entry in geometry.iter().copied() {
            if stored
                .iter()
                .all(|other| other.projector != entry.projector)
                && stored.len() < ProjectorKind::ALL.len()
            {
                stored.push(entry);
            }
        }
        stored.sort_unstable_by_key(|entry| entry.projector);
    }

    /// Captures open projectors while retaining the last known state for feeds
    /// that are currently closed.
    pub(crate) fn capture_geometry(&self) -> Vec<ProjectorGeometry> {
        let mut geometry = self.geometry.borrow().clone();
        for (feed, slot) in [
            (ProjectorFeed::Program, &self.program),
            (ProjectorFeed::Preview, &self.preview),
            (ProjectorFeed::Multiview, &self.multiview),
            (ProjectorFeed::Source, &self.source),
            (ProjectorFeed::Scene, &self.scene),
        ] {
            let window = slot.borrow();
            if let Some(window) = window.as_ref() {
                if let Some(entry) = capture_projector_geometry(feed, window) {
                    replace_projector_geometry(&mut geometry, entry);
                }
            }
        }
        geometry.sort_unstable_by_key(|entry| entry.projector);
        geometry
    }

    fn remember_geometry(&self, feed: ProjectorFeed, window: &ProjectorWindow) {
        let Some(entry) = capture_projector_geometry(feed, window) else {
            return;
        };
        replace_projector_geometry(&mut self.geometry.borrow_mut(), entry);
    }

    fn stored_geometry(&self, feed: ProjectorFeed) -> Option<ProjectorGeometry> {
        self.geometry
            .borrow()
            .iter()
            .find(|entry| entry.projector == feed.kind())
            .copied()
    }

    /// Pushes the studio's current images into any open projector.
    pub(crate) fn sync(&self, ui: &MainWindow) {
        if let Some(window) = self.program.borrow().as_ref() {
            window.set_source_image(ui.get_program_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.preview.borrow().as_ref() {
            window.set_source_image(ui.get_preview_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.multiview.borrow().as_ref() {
            window.set_source_image(ui.get_multiview_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.source.borrow().as_ref() {
            window.set_source_image(ui.get_source_projector_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.scene.borrow().as_ref() {
            window.set_source_image(ui.get_scene_projector_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
    }

    /// Repaints open projectors when the studio theme changes.
    pub(crate) fn set_tokens(&self, tokens: &crate::ThemeTokens) {
        for window in [
            &self.program,
            &self.preview,
            &self.multiview,
            &self.source,
            &self.scene,
        ] {
            if let Some(window) = window.borrow().as_ref() {
                window.global::<crate::Palette>().set_tokens(tokens.clone());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_open(&self, program: bool) -> bool {
        self.slot(if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        })
        .borrow()
        .is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_multiview_open(&self) -> bool {
        self.slot(ProjectorFeed::Multiview).borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_multiview_fullscreen(&self) -> bool {
        self.slot(ProjectorFeed::Multiview)
            .borrow()
            .as_ref()
            .is_some_and(|window| window.window().is_fullscreen())
    }

    #[cfg(test)]
    pub(crate) fn is_source_open(&self) -> bool {
        self.slot(ProjectorFeed::Source).borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_scene_open(&self) -> bool {
        self.slot(ProjectorFeed::Scene).borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_fullscreen(&self, program: bool) -> bool {
        self.slot(if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        })
        .borrow()
        .as_ref()
        .is_some_and(|window| window.window().is_fullscreen())
    }

    const fn slot(&self, feed: ProjectorFeed) -> &RefCell<Option<ProjectorWindow>> {
        match feed {
            ProjectorFeed::Program => &self.program,
            ProjectorFeed::Preview => &self.preview,
            ProjectorFeed::Multiview => &self.multiview,
            ProjectorFeed::Source => &self.source,
            ProjectorFeed::Scene => &self.scene,
        }
    }
}

/// Installs every menu-bar action and returns the projector controller.
pub(crate) fn install_menu_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    docks: &Rc<DockController>,
) -> Rc<ProjectorController> {
    let projectors = Rc::new(ProjectorController {
        program: RefCell::new(None),
        preview: RefCell::new(None),
        multiview: RefCell::new(None),
        source: RefCell::new(None),
        scene: RefCell::new(None),
        source_target: RefCell::new(None),
        scene_target: RefCell::new(None),
        geometry: RefCell::new(Vec::new()),
    });

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
        let tree =
            crate::dock_tree::DockNode::from_legacy(&[1, 0, 2, 3, 4], &[1.0, 1.0, 1.85, 1.0, 1.4])
                .expect("the built-in dock layout must be valid");
        docks.replace_tree(&tree, &ui);
        ui.set_status_message("Dock layout reset".into());
    });
}

fn install_projectors(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
) {
    let weak = ui.as_weak();
    let preview_state = Rc::clone(state);
    let preview_projectors = Rc::clone(projectors);
    ui.on_open_projector(move |program| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Selecting an open projector again closes it, so the menu entry is a
        // toggle rather than a way to stack duplicate windows.
        let feed = if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        };
        if preview_projectors.slot(feed).borrow().is_some() {
            close_projector(&preview_projectors, feed);
            return;
        }
        match open_projector(&ui, &preview_state, &preview_projectors, feed) {
            Ok(window) => {
                *preview_projectors.slot(feed).borrow_mut() = Some(window);
                preview_projectors.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Projector: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let multiview_state = Rc::clone(state);
    let multiview_projectors = Rc::clone(projectors);
    ui.on_open_multiview_projector(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let feed = ProjectorFeed::Multiview;
        if multiview_projectors.slot(feed).borrow().is_some() {
            close_projector(&multiview_projectors, feed);
            return;
        }
        match open_projector(&ui, &multiview_state, &multiview_projectors, feed) {
            Ok(window) => {
                *multiview_projectors.slot(feed).borrow_mut() = Some(window);
                multiview_projectors.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Projector: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_projectors = Rc::clone(projectors);
    ui.on_open_source_projector(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let feed = ProjectorFeed::Source;
        if source_projectors.slot(feed).borrow().is_some() {
            close_projector(&source_projectors, feed);
            return;
        }
        let item = ui.get_selected_source().to_string();
        let target = source_target(&source_state.borrow(), &item);
        let Some(target) = target else {
            ui.set_status_message("Select a source before opening its projector".into());
            return;
        };
        *source_projectors.source_target.borrow_mut() = Some(SourceProjectorTarget {
            scene: target.scene,
            item: target.item,
        });
        match open_projector(&ui, &source_state, &source_projectors, feed) {
            Ok(window) => {
                *source_projectors.slot(feed).borrow_mut() = Some(window);
                source_projectors.sync(&ui);
            }
            Err(error) => {
                source_projectors.source_target.borrow_mut().take();
                ui.set_status_message(format!("Projector: {error}").into());
            }
        }
    });

    install_scene_projector(ui, state, projectors);
}

fn install_scene_projector(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
) {
    let weak = ui.as_weak();
    let scene_state = Rc::clone(state);
    let scene_projectors = Rc::clone(projectors);
    ui.on_open_scene_projector(move |scene| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let feed = ProjectorFeed::Scene;
        if scene_projectors.slot(feed).borrow().is_some() {
            close_projector(&scene_projectors, feed);
            return;
        }
        let scene = scene.to_string();
        let exists = scene_state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene.as_str()))
            .is_some();
        if !exists {
            ui.set_status_message("Scene projector target is unavailable".into());
            return;
        }
        *scene_projectors.scene_target.borrow_mut() = Some(SceneProjectorTarget { scene });
        match open_projector(&ui, &scene_state, &scene_projectors, feed) {
            Ok(window) => {
                *scene_projectors.slot(feed).borrow_mut() = Some(window);
                scene_projectors.sync(&ui);
            }
            Err(error) => {
                scene_projectors.scene_target.borrow_mut().take();
                ui.set_status_message(format!("Projector: {error}").into());
            }
        }
    });
}

fn replace_projector_geometry(geometry: &mut Vec<ProjectorGeometry>, entry: ProjectorGeometry) {
    if let Some(existing) = geometry
        .iter_mut()
        .find(|existing| existing.projector == entry.projector)
    {
        *existing = entry;
    } else if geometry.len() < ProjectorKind::ALL.len() {
        geometry.push(entry);
    }
}

fn capture_projector_geometry(
    feed: ProjectorFeed,
    window: &ProjectorWindow,
) -> Option<ProjectorGeometry> {
    let fullscreen = window.window().is_fullscreen();
    let position = window.window().position();
    let size = window.window().size();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the scale factor is finite and stored as bounded thousandths"
    )]
    let scale_milli = (window.window().scale_factor().max(0.5) * 1_000.0).round() as u32;
    ProjectorGeometry::new(
        feed.kind(),
        position.x,
        position.y,
        size.width,
        size.height,
        scale_milli,
    )
    .map(|entry| entry.with_fullscreen(fullscreen))
}

fn restore_projector_geometry(window: &ProjectorWindow, geometry: ProjectorGeometry) {
    window.window().set_fullscreen(geometry.fullscreen);
    if geometry.fullscreen {
        return;
    }
    let current_scale = window.window().scale_factor().max(0.5);
    #[allow(
        clippy::cast_precision_loss,
        reason = "the stored scale is bounded thousandths and f32 is sufficient for DPI"
    )]
    let saved_scale = (geometry.scale_milli as f32 / 1_000.0).max(0.5);
    let ratio = (current_scale / saved_scale).clamp(0.5, 2.0);
    let width = scale_window_dimension(
        geometry.width,
        ratio,
        FloatingGeometry::MIN_WIDTH,
        FloatingGeometry::MAX_WIDTH,
    );
    let height = scale_window_dimension(
        geometry.height,
        ratio,
        FloatingGeometry::MIN_HEIGHT,
        FloatingGeometry::MAX_HEIGHT,
    );
    let (x, y) = current_desktop_bounds().map_or((geometry.x, geometry.y), |bounds| {
        clamp_window_position(geometry.x, geometry.y, width, height, bounds)
    });
    window
        .window()
        .set_position(slint::PhysicalPosition::new(x, y));
    window
        .window()
        .set_size(slint::PhysicalSize::new(width, height));
}

fn open_projector(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
    feed: ProjectorFeed,
) -> Result<ProjectorWindow, slint::PlatformError> {
    let window = ProjectorWindow::new()?;
    let locale = state.borrow().locale();
    window
        .global::<crate::I18n>()
        .set_text(crate::i18n::catalog(locale));
    window
        .global::<crate::Palette>()
        .set_tokens(ui.global::<crate::Palette>().get_tokens());
    window.set_feed_label(crate::i18n::with_catalog(locale, |text| match feed {
        ProjectorFeed::Program => text.program.clone(),
        ProjectorFeed::Preview => text.preview.clone(),
        ProjectorFeed::Multiview => text.menu_multiview_projector.clone(),
        ProjectorFeed::Source => text.menu_source_projector.clone(),
        ProjectorFeed::Scene => text.scene_projector.clone(),
    }));
    window.set_source_image(match feed {
        ProjectorFeed::Program => ui.get_program_image(),
        ProjectorFeed::Preview => ui.get_preview_image(),
        ProjectorFeed::Multiview => ui.get_multiview_image(),
        ProjectorFeed::Source => ui.get_source_projector_image(),
        ProjectorFeed::Scene => ui.get_scene_projector_image(),
    });
    // OBS presents program and multiview projectors as borderless fullscreen
    // feeds by default. A stored toggle wins, so F11 survives a restart while
    // a first open still follows the feed's reference default.
    if let Some(geometry) = projectors.stored_geometry(feed) {
        restore_projector_geometry(&window, geometry);
    } else {
        window.window().set_fullscreen(feed.is_fullscreen());
    }

    let projectors = Rc::clone(projectors);
    window.on_close_requested(move || close_projector(&projectors, feed));
    let weak = window.as_weak();
    window.on_toggle_fullscreen(move || {
        if let Some(window) = weak.upgrade() {
            window
                .window()
                .set_fullscreen(!window.window().is_fullscreen());
        }
    });

    window.show()?;
    Ok(window)
}

fn close_projector(projectors: &Rc<ProjectorController>, feed: ProjectorFeed) {
    if let Some(window) = projectors.slot(feed).borrow_mut().take() {
        projectors.remember_geometry(feed, &window);
        let _ = window.hide();
    }
    match feed {
        ProjectorFeed::Source => {
            projectors.source_target.borrow_mut().take();
        }
        ProjectorFeed::Scene => {
            projectors.scene_target.borrow_mut().take();
        }
        ProjectorFeed::Program | ProjectorFeed::Preview | ProjectorFeed::Multiview => {}
    }
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

fn install_collections(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let select_state = Rc::clone(state);
    let select_surface = Rc::clone(surface);
    ui.on_select_collection(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let path = id.to_string();
        let result: Result<(), Box<dyn Error>> = (|| {
            let store = project_store(&path)?;
            select_state
                .borrow_mut()
                .load_project_for_key(&store, &path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                ui.set_project_path(path.as_str().into());
                crate::refresh::invalidate_recovery_cache();
                refresh_ui(&ui, &select_state, &select_surface);
                refresh_menu_models(&ui, &select_state);
                ui.set_status_message(format!("Opened collection {path}").into());
            }
            Err(error) => ui.set_status_message(format!("Collection: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let create_state = Rc::clone(state);
    let create_surface = Rc::clone(surface);
    ui.on_create_collection(move |name| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let current = ui.get_project_path().to_string();
        match create_collection(&create_state, &current, name.as_str()) {
            Ok(path) => {
                let path = path.to_string_lossy().into_owned();
                create_state.borrow_mut().set_project_selection_key(&path);
                ui.set_project_path(path.as_str().into());
                ui.set_collection_name("".into());
                crate::refresh::invalidate_recovery_cache();
                refresh_ui(&ui, &create_state, &create_surface);
                refresh_menu_models(&ui, &create_state);
                ui.set_status_message(format!("Created collection {path}").into());
            }
            Err(error) => ui.set_status_message(format!("Collection: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let duplicate_state = Rc::clone(state);
    let duplicate_surface = Rc::clone(surface);
    ui.on_duplicate_collection(move |name| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let current = ui.get_project_path().to_string();
        match duplicate_collection(&duplicate_state, &current, name.as_str()) {
            Ok(path) => {
                let path = path.to_string_lossy().into_owned();
                duplicate_state
                    .borrow_mut()
                    .set_project_selection_key(&path);
                ui.set_project_path(path.as_str().into());
                ui.set_collection_name("".into());
                crate::refresh::invalidate_recovery_cache();
                refresh_ui(&ui, &duplicate_state, &duplicate_surface);
                refresh_menu_models(&ui, &duplicate_state);
                ui.set_status_message(format!("Duplicated collection to {path}").into());
            }
            Err(error) => ui.set_status_message(format!("Collection: {error}").into()),
        }
    });

    install_rename_collection(ui, state, surface);
    install_export_collection(ui, state);
    install_import_collection(ui, state, surface);

    let weak = ui.as_weak();
    let save_state = Rc::clone(state);
    ui.on_save_collection(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let path = ui.get_project_path().to_string();
        let result: Result<usize, Box<dyn Error>> = (|| {
            let store = project_store(&path)?;
            Ok(save_state.borrow_mut().save_project(&store)?)
        })();
        match result {
            Ok(bytes) => {
                crate::refresh::invalidate_recovery_cache();
                refresh_menu_models(&ui, &save_state);
                ui.set_status_message(format!("Saved collection to {path} ({bytes} bytes)").into());
            }
            Err(error) => ui.set_status_message(format!("Collection: {error}").into()),
        }
    });
}

fn install_rename_collection(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let rename_state = Rc::clone(state);
    let rename_surface = Rc::clone(surface);
    ui.on_rename_collection(move |name| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let current = ui.get_project_path().to_string();
        match rename_collection(&rename_state, &current, name.as_str()) {
            Ok(path) => {
                let path = path.to_string_lossy().into_owned();
                rename_state.borrow_mut().set_project_selection_key(&path);
                ui.set_project_path(path.as_str().into());
                ui.set_collection_name("".into());
                crate::refresh::invalidate_recovery_cache();
                refresh_ui(&ui, &rename_state, &rename_surface);
                refresh_menu_models(&ui, &rename_state);
                ui.set_status_message(format!("Renamed collection to {path}").into());
            }
            Err(error) => ui.set_status_message(format!("Collection: {error}").into()),
        }
    });
}

fn install_export_collection(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let weak = ui.as_weak();
    let export_state = Rc::clone(state);
    ui.on_export_collection(move |path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match export_collection(&export_state, path.as_str()) {
            Ok((path, bytes)) => {
                let path = path.to_string_lossy().into_owned();
                ui.set_collection_transfer_path("".into());
                crate::refresh::invalidate_recovery_cache();
                refresh_menu_models(&ui, &export_state);
                ui.set_status_message(
                    format!("Exported collection to {path} ({bytes} bytes)").into(),
                );
            }
            Err(error) => ui.set_status_message(format!("Collection export: {error}").into()),
        }
    });
}

fn install_import_collection(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let import_state = Rc::clone(state);
    let import_surface = Rc::clone(surface);
    ui.on_import_collection(move |source| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let current = ui.get_project_path().to_string();
        match import_collection(&import_state, &current, source.as_str()) {
            Ok(path) => {
                let path = path.to_string_lossy().into_owned();
                ui.set_project_path(path.as_str().into());
                ui.set_collection_transfer_path("".into());
                crate::refresh::invalidate_recovery_cache();
                refresh_ui(&ui, &import_state, &import_surface);
                refresh_menu_models(&ui, &import_state);
                ui.set_status_message(format!("Imported collection to {path}").into());
            }
            Err(error) => ui.set_status_message(format!("Collection import: {error}").into()),
        }
    });
}

/// Writes the current scenes into a new named collection and switches to it.
///
/// The current document is committed first, so naming a new collection captures
/// the work in progress instead of discarding it.
fn create_collection(
    state: &Rc<RefCell<DesktopState>>,
    current_path: &str,
    name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let file_name = collection_file_name(name)
        .ok_or_else(|| std::io::Error::other("a collection needs a name"))?;
    let directory = collections_root(current_path);
    let path = directory.join(file_name);
    if path.exists() {
        return Err(std::io::Error::other(format!("{} already exists", path.display())).into());
    }
    std::fs::create_dir_all(&directory)?;
    // The in-progress document is committed before the switch, so naming a new
    // collection is never the action that loses the previous one's edits.
    if !current_path.trim().is_empty() {
        let store = project_store(current_path)?;
        let mut state = state.borrow_mut();
        state.set_project_selection_key(current_path);
        state.save_project(&store)?;
    }
    let path_text = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("the collection path is not valid UTF-8"))?
        .to_owned();
    let initial_document = DesktopState::new(initial_project()?).project_document();
    let store = project_store(&path_text)?;
    store.save_document(&initial_document)?;
    state
        .borrow_mut()
        .load_project_for_key(&store, &path_text)?;
    Ok(path)
}

/// Copies the current project document into a new collection and makes that
/// copy the active document.
///
/// The current project is saved first, so the duplicate includes edits that
/// have not yet been written to the original collection. Both writes use the
/// crash-safe project store; a failed target write leaves the in-memory
/// document and original file intact.
fn duplicate_collection(
    state: &Rc<RefCell<DesktopState>>,
    current_path: &str,
    name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let file_name = collection_file_name(name)
        .ok_or_else(|| std::io::Error::other("a collection needs a name"))?;
    let directory = collections_root(current_path);
    let path = directory.join(file_name);
    if path.exists() {
        return Err(std::io::Error::other(format!("{} already exists", path.display())).into());
    }
    std::fs::create_dir_all(&directory)?;
    if !current_path.trim().is_empty() {
        let store = project_store(current_path)?;
        state.borrow_mut().save_project(&store)?;
    }
    let path_text = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("the collection path is not valid UTF-8"))?
        .to_owned();
    let store = project_store(&path_text)?;
    state.borrow_mut().save_project(&store)?;
    Ok(path)
}

/// Saves the current document and atomically moves it to a new collection name.
///
/// The target is always a sibling in the managed collection directory. This
/// also gives the initial project file a stable collection location once it is
/// renamed. Existing targets are never overwritten.
fn rename_collection(
    state: &Rc<RefCell<DesktopState>>,
    current_path: &str,
    name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let current = current_path.trim();
    if current.is_empty() {
        return Err(std::io::Error::other("there is no active collection to rename").into());
    }
    let current_path = Path::new(current);
    let file_name = collection_file_name(name)
        .ok_or_else(|| std::io::Error::other("a collection needs a name"))?;
    let directory = collections_root(current);
    let target = directory.join(file_name);
    if target == current_path {
        let store = project_store(current)?;
        state.borrow_mut().save_project(&store)?;
        return Ok(current_path.to_path_buf());
    }
    if target.exists() {
        return Err(std::io::Error::other(format!("{} already exists", target.display())).into());
    }
    std::fs::create_dir_all(&directory)?;
    let store = project_store(current)?;
    state.borrow_mut().save_project(&store)?;
    std::fs::rename(current_path, &target)?;
    Ok(target)
}

/// Writes the active in-memory document to an explicit portable collection
/// path without changing the active project or its collection selection.
fn export_collection(
    state: &Rc<RefCell<DesktopState>>,
    target: &str,
) -> Result<(PathBuf, usize), Box<dyn Error>> {
    let path = collection_transfer_path(target)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let path_text = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("the export path is not valid UTF-8"))?;
    let store = project_store(path_text)?;
    let bytes = state.borrow().save_project_document(&store)?;
    Ok((path, bytes))
}

fn collection_transfer_path(target: &str) -> Result<PathBuf, Box<dyn Error>> {
    collection_path(target, "export")
}

fn collection_path(target: &str, operation: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = PathBuf::from(target.trim());
    if path.as_os_str().is_empty()
        || path.file_name().is_none()
        || path.extension().and_then(|extension| extension.to_str()) != Some(COLLECTION_EXTENSION)
    {
        return Err(std::io::Error::other(format!(
            "{operation} path must name a .{COLLECTION_EXTENSION} file"
        ))
        .into());
    }
    Ok(path)
}

/// Validates an external collection, copies its parsed document atomically
/// into the managed collection directory, and makes that copy active.
///
/// Parsing happens before the target directory is created, and an existing
/// managed collection is never overwritten. The caller owns the unsaved-change
/// confirmation before invoking this replacing operation.
fn import_collection(
    state: &Rc<RefCell<DesktopState>>,
    current_path: &str,
    source: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let source = collection_path(source, "import")?;
    if !source.is_file() {
        return Err(std::io::Error::other(format!(
            "the import file does not exist: {}",
            source.display()
        ))
        .into());
    }
    let source_name = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(collection_file_name)
        .ok_or_else(|| std::io::Error::other("the import file needs a valid collection name"))?;
    let target = collections_root(current_path).join(source_name);
    if target == source {
        return Err(std::io::Error::other(
            "the collection is already in the managed collections directory",
        )
        .into());
    }
    if target.exists() {
        return Err(std::io::Error::other(format!("{} already exists", target.display())).into());
    }

    let source_text = source
        .to_str()
        .ok_or_else(|| std::io::Error::other("the import path is not valid UTF-8"))?;
    let project = project_store(source_text)?.load()?;
    let document = project.serialize();
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| std::io::Error::other("the managed collection directory is invalid"))?;
    std::fs::create_dir_all(directory)?;
    let target_text = target
        .to_str()
        .ok_or_else(|| std::io::Error::other("the target path is not valid UTF-8"))?;
    let store = project_store(target_text)?;
    store.save_document(&document)?;
    state
        .borrow_mut()
        .load_project_for_key(&store, target_text)?;
    Ok(target)
}

/// Turns a typed name into a bounded, separator-free file name.
///
/// The name reaches the filesystem, so path separators and traversal segments
/// are dropped here rather than trusted.
fn collection_file_name(name: &str) -> Option<String> {
    let cleaned = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim().trim_matches('-').trim();
    if cleaned.is_empty() {
        return None;
    }
    let mut cleaned = cleaned.to_owned();
    if cleaned.len() > MAX_COLLECTION_NAME {
        let mut end = MAX_COLLECTION_NAME;
        while !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned.truncate(end);
    }
    Some(format!("{}.{COLLECTION_EXTENSION}", cleaned.trim()))
}

/// Returns the directory that holds this session's scene collections.
///
/// A bare file name has no parent, so the folder resolves against the working
/// directory rather than against the filesystem root. Once a collection is
/// active, its parent is already the managed directory; keep using that
/// sibling directory instead of creating a nested `collections/collections`
/// path on the next operation.
fn collections_root(project_path: &str) -> PathBuf {
    let parent = Path::new(project_path.trim())
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    if parent.file_name().and_then(|name| name.to_str()) == Some(COLLECTION_DIRECTORY) {
        parent
    } else {
        parent.join(COLLECTION_DIRECTORY)
    }
}

/// Lists the collections on disk plus the open document, newest name order.
///
/// The currently open project is always included even when it lives outside the
/// collections directory, so the menu never shows an empty list for a session
/// that plainly has scenes loaded.
pub(crate) fn discover_collections(project_path: &str) -> Vec<ProfileRow> {
    let current = Path::new(project_path.trim()).to_path_buf();
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(collections_root(project_path)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(COLLECTION_EXTENSION) {
                continue;
            }
            if let Some(row) = collection_row(&path, &current) {
                rows.push(row);
            }
        }
    }
    if !current.as_os_str().is_empty() && !rows.iter().any(|row| row.active) {
        if let Some(row) = collection_row(&current, &current) {
            rows.push(row);
        }
    }
    // A stable order keeps the menu from reshuffling between openings, which
    // the filesystem's own directory order does not guarantee.
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

fn collection_row(path: &Path, current: &Path) -> Option<ProfileRow> {
    let id = path.to_str()?;
    let name = path.file_stem().and_then(|stem| stem.to_str())?;
    Some(ProfileRow {
        id: id.into(),
        name: name.into(),
        active: path == current,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_project::ProjectCommand;

    #[test]
    fn a_collection_name_becomes_a_bounded_separator_free_file_name() {
        assert_eq!(
            collection_file_name("Evening show"),
            Some("Evening show.obsrproj".to_owned())
        );
        assert_eq!(
            collection_file_name("../../etc/passwd"),
            Some("etc-passwd.obsrproj".to_owned()),
            "path separators must never survive into the file name"
        );
        assert_eq!(collection_file_name("   "), None);
        assert_eq!(collection_file_name("///"), None);
    }

    #[test]
    fn a_long_collection_name_is_truncated_rather_than_rejected() {
        let name = "a".repeat(MAX_COLLECTION_NAME * 2);

        let file_name = collection_file_name(&name).expect("a long name is still usable");

        assert_eq!(
            file_name,
            format!("{}.{COLLECTION_EXTENSION}", "a".repeat(MAX_COLLECTION_NAME))
        );
    }

    #[test]
    fn a_long_unicode_collection_name_is_truncated_on_a_character_boundary() {
        let file_name =
            collection_file_name(&"é".repeat(MAX_COLLECTION_NAME)).expect("unicode name");
        let stem = file_name
            .strip_suffix(&format!(".{COLLECTION_EXTENSION}"))
            .expect("collection extension");

        assert!(stem.len() <= MAX_COLLECTION_NAME);
        assert!(stem.chars().all(|character| character == 'é'));
    }

    #[test]
    fn collections_live_beside_the_configured_project_file() {
        assert_eq!(
            collections_root("/home/user/studio/obs-rs-project.json"),
            PathBuf::from("/home/user/studio/collections")
        );
        // A bare file name has no parent, so the collections folder is resolved
        // against the working directory instead of the filesystem root.
        assert_eq!(
            collections_root("obs-rs-project.json"),
            PathBuf::from("./collections")
        );
    }

    #[test]
    fn collections_keep_one_root_after_switching_to_a_collection() {
        assert_eq!(
            collections_root("/home/user/studio/collections/evening.obsrproj"),
            PathBuf::from("/home/user/studio/collections")
        );
        assert_eq!(
            collections_root("collections/evening.obsrproj"),
            PathBuf::from("collections")
        );
    }

    #[test]
    fn creating_a_collection_preserves_the_previous_document_scene_selection() {
        let root = std::env::temp_dir().join(format!(
            "obs-rs-collection-create-selection-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("collection fixture directory");
        let current = root.join("current.obsrproj");
        let current_text = current.to_str().expect("current path");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));
        state.borrow_mut().set_project_selection_key(current_text);
        state
            .borrow_mut()
            .dispatch(UiCommand::SelectPreviewScene {
                id: "intermission".to_owned(),
            })
            .expect("preview selection");
        state
            .borrow_mut()
            .dispatch(UiCommand::SelectProgramScene {
                id: "program".to_owned(),
            })
            .expect("program selection");

        let created =
            create_collection(&state, current_text, "Fresh show").expect("collection creation");
        assert_eq!(
            created,
            root.join("collections").join("Fresh show.obsrproj")
        );
        assert_eq!(state.borrow().preview_scene(), Some("preview"));
        assert_eq!(state.borrow().program_scene(), Some("preview"));

        let current_store = project_store(current_text).expect("current store");
        state
            .borrow_mut()
            .load_project_for_key(&current_store, current_text)
            .expect("return to current collection");
        assert_eq!(state.borrow().preview_scene(), Some("intermission"));
        assert_eq!(state.borrow().program_scene(), Some("program"));

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn the_open_document_is_listed_even_without_a_collections_directory() {
        let rows = discover_collections("/nonexistent/studio/obs-rs-project.json");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "obs-rs-project");
        assert!(rows[0].active, "the open document is the active collection");
    }

    #[test]
    fn duplicating_a_collection_copies_the_current_project_document() {
        let root = std::env::temp_dir().join(format!(
            "obs-rs-collection-duplicate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("collection fixture directory");
        let current = root.join("current.obsrproj");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));

        let duplicate = duplicate_collection(
            &state,
            current.to_str().expect("current path"),
            "Broadcast copy",
        )
        .expect("collection duplicate");
        let current_document =
            std::fs::read_to_string(&current).expect("current collection was saved");
        let duplicate_document =
            std::fs::read_to_string(&duplicate).expect("duplicate collection was saved");

        assert_eq!(
            current_document, duplicate_document,
            "the duplicate must contain the same serialized project"
        );
        assert_eq!(
            state.borrow().project_document(),
            duplicate_document,
            "the active in-memory project remains the copied document"
        );
        assert_eq!(
            duplicate.file_name().and_then(|name| name.to_str()),
            Some("Broadcast copy.obsrproj")
        );

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn renaming_a_collection_moves_the_saved_document_and_updates_no_project_state() {
        let root =
            std::env::temp_dir().join(format!("obs-rs-collection-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("collection fixture directory");
        let current = root.join("current.obsrproj");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));

        let renamed = rename_collection(
            &state,
            current.to_str().expect("current path"),
            "Evening show",
        )
        .expect("collection rename");
        let document = std::fs::read_to_string(&renamed).expect("renamed collection was saved");

        assert!(!current.exists(), "the old collection path must be moved");
        assert_eq!(
            renamed,
            root.join("collections").join("Evening show.obsrproj")
        );
        assert_eq!(state.borrow().project_document(), document);

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn renaming_a_collection_never_overwrites_an_existing_target() {
        let root = std::env::temp_dir().join(format!(
            "obs-rs-collection-rename-conflict-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("collections")).expect("collection fixture directory");
        let current = root.join("current.obsrproj");
        let target = root.join("collections").join("Evening show.obsrproj");
        std::fs::write(&current, "current document").expect("current fixture");
        std::fs::write(&target, "keep this document").expect("target fixture");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));

        let error = rename_collection(
            &state,
            current.to_str().expect("current path"),
            "Evening show",
        )
        .expect_err("existing collections must not be overwritten");

        assert!(error.to_string().contains("already exists"));
        assert_eq!(
            std::fs::read_to_string(&current).expect("current fixture remains"),
            "current document"
        );
        assert_eq!(
            std::fs::read_to_string(target).expect("target fixture remains"),
            "keep this document"
        );

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn exporting_a_collection_writes_the_current_document_without_switching_it() {
        let root =
            std::env::temp_dir().join(format!("obs-rs-collection-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("collection fixture directory");
        let current = root.join("current.obsrproj");
        let target = root.join("portable").join("export.obsrproj");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneName {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                name: "Edited preview".to_owned(),
            }))
            .expect("edit current collection before export");
        assert!(state.borrow().is_dirty());
        let expected = state.borrow().project_document().clone();

        let (exported, bytes) = export_collection(&state, target.to_str().expect("export path"))
            .expect("collection export");

        assert_eq!(exported, target);
        assert_eq!(bytes, expected.len());
        assert_eq!(
            std::fs::read_to_string(&target).expect("exported document"),
            expected
        );
        assert_eq!(state.borrow().project_document(), expected);
        assert!(
            state.borrow().is_dirty(),
            "export must not mark the active project clean"
        );
        assert!(
            !current.exists(),
            "export must not create or switch the active path"
        );

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn importing_a_collection_copies_validated_document_and_switches_to_managed_copy() {
        let root =
            std::env::temp_dir().join(format!("obs-rs-collection-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source_directory = root.join("portable");
        std::fs::create_dir_all(&source_directory).expect("import fixture directory");
        let current = root.join("current.json");
        let source = source_directory.join("Evening show.obsrproj");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));
        let expected = state.borrow().project_document();
        std::fs::write(&source, &expected).expect("write external collection");

        let imported = import_collection(
            &state,
            current.to_str().expect("current path"),
            source.to_str().expect("source path"),
        )
        .expect("collection import");

        assert_eq!(
            imported,
            root.join("collections").join("Evening show.obsrproj")
        );
        assert_eq!(
            std::fs::read_to_string(&imported).expect("managed collection"),
            expected
        );
        assert_eq!(
            std::fs::read_to_string(&source).expect("source remains"),
            expected
        );
        assert_eq!(state.borrow().project_document(), expected);
        assert!(
            !state.borrow().is_dirty(),
            "an imported document starts clean"
        );
        assert!(
            !current.exists(),
            "import must not create the old active path"
        );

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn importing_a_collection_rejects_invalid_documents_without_creating_target() {
        let root = std::env::temp_dir().join(format!(
            "obs-rs-collection-import-invalid-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("import fixture directory");
        let source = root.join("broken.obsrproj");
        std::fs::write(&source, "not a project").expect("invalid collection fixture");
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));

        let error = import_collection(
            &state,
            root.join("current.json").to_str().expect("current path"),
            source.to_str().expect("source path"),
        )
        .expect_err("invalid collections must be rejected");

        assert!(error.to_string().contains("invalid") || error.to_string().contains("project"));
        assert!(!root.join("collections").exists());
        assert!(!state.borrow().is_dirty());

        std::fs::remove_dir_all(root).expect("remove collection fixture");
    }

    #[test]
    fn collection_export_rejects_non_collection_paths() {
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));

        let error = export_collection(&state, "portable.json")
            .expect_err("exports must keep the collection extension");

        assert!(error.to_string().contains(".obsrproj"));
    }
}
