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

use crate::{
    callbacks::docks::DockController, dispatch_and_refresh, initial_project, project_store,
    refresh_ui, MainWindow, PreviewSurface, ProfileRow, ProjectorWindow,
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
}

impl ProjectorController {
    /// Returns whether a program projector needs the program canvas rendered.
    ///
    /// Single-canvas editing skips the program render to save a full-size
    /// composite per frame, so an open program projector has to ask for it back.
    pub(crate) fn wants_program(&self) -> bool {
        self.program.borrow().is_some()
    }

    /// Returns whether a preview projector needs the preview feed rendered.
    pub(crate) fn wants_preview(&self) -> bool {
        self.preview.borrow().is_some()
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
    }

    /// Repaints open projectors when the studio theme changes.
    pub(crate) fn set_tokens(&self, tokens: &crate::ThemeTokens) {
        for window in [&self.program, &self.preview] {
            if let Some(window) = window.borrow().as_ref() {
                window.global::<crate::Palette>().set_tokens(tokens.clone());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_open(&self, program: bool) -> bool {
        self.slot(program).borrow().is_some()
    }

    const fn slot(&self, program: bool) -> &RefCell<Option<ProjectorWindow>> {
        if program {
            &self.program
        } else {
            &self.preview
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
    let state = Rc::clone(state);
    let projectors = Rc::clone(projectors);
    ui.on_open_projector(move |program| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Selecting an open projector again closes it, so the menu entry is a
        // toggle rather than a way to stack duplicate windows.
        if projectors.slot(program).borrow().is_some() {
            close_projector(&projectors, program);
            return;
        }
        match open_projector(&ui, &state, &projectors, program) {
            Ok(window) => {
                *projectors.slot(program).borrow_mut() = Some(window);
                projectors.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Projector: {error}").into()),
        }
    });
}

fn open_projector(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
    program: bool,
) -> Result<ProjectorWindow, slint::PlatformError> {
    let window = ProjectorWindow::new()?;
    let locale = state.borrow().locale();
    window
        .global::<crate::I18n>()
        .set_text(crate::i18n::catalog(locale));
    window
        .global::<crate::Palette>()
        .set_tokens(ui.global::<crate::Palette>().get_tokens());
    window.set_feed_label(crate::i18n::with_catalog(locale, |text| {
        if program {
            text.program.clone()
        } else {
            text.preview.clone()
        }
    }));
    window.set_source_image(if program {
        ui.get_program_image()
    } else {
        ui.get_preview_image()
    });

    let projectors = Rc::clone(projectors);
    window.on_close_requested(move || close_projector(&projectors, program));

    window.show()?;
    Ok(window)
}

fn close_projector(projectors: &Rc<ProjectorController>, program: bool) {
    if let Some(window) = projectors.slot(program).borrow_mut().take() {
        let _ = window.hide();
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
            select_state.borrow_mut().load_project(&store)?;
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
        state.borrow_mut().save_project(&store)?;
    }
    let path_text = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("the collection path is not valid UTF-8"))?
        .to_owned();
    *state.borrow_mut() = DesktopState::new(initial_project()?);
    let store = project_store(&path_text)?;
    state.borrow_mut().save_project(&store)?;
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
    let path = PathBuf::from(target.trim());
    if path.as_os_str().is_empty()
        || path.file_name().is_none()
        || path.extension().and_then(|extension| extension.to_str()) != Some(COLLECTION_EXTENSION)
    {
        return Err(std::io::Error::other(format!(
            "export path must name a .{COLLECTION_EXTENSION} file"
        ))
        .into());
    }
    Ok(path)
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
    fn collection_export_rejects_non_collection_paths() {
        let state = Rc::new(RefCell::new(DesktopState::new(
            initial_project().expect("initial project"),
        )));

        let error = export_collection(&state, "portable.json")
            .expect_err("exports must keep the collection extension");

        assert!(error.to_string().contains(".obsrproj"));
    }
}
