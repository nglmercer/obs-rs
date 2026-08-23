#[allow(
    clippy::wildcard_imports,
    reason = "menu submodules share the callback boundary namespace"
)]
use super::*;

pub(super) fn install_collections(
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

pub(super) fn install_rename_collection(
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

pub(super) fn install_export_collection(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
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

pub(super) fn install_import_collection(
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
pub(super) fn create_collection(
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
pub(super) fn duplicate_collection(
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
pub(super) fn rename_collection(
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
pub(super) fn export_collection(
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

pub(super) fn collection_transfer_path(target: &str) -> Result<PathBuf, Box<dyn Error>> {
    collection_path(target, "export")
}

pub(super) fn collection_path(target: &str, operation: &str) -> Result<PathBuf, Box<dyn Error>> {
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
pub(super) fn import_collection(
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
pub(super) fn collection_file_name(name: &str) -> Option<String> {
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
pub(super) fn collections_root(project_path: &str) -> PathBuf {
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

pub(super) fn collection_row(path: &Path, current: &Path) -> Option<ProfileRow> {
    let id = path.to_str()?;
    let name = path.file_stem().and_then(|stem| stem.to_str())?;
    Some(ProfileRow {
        id: id.into(),
        name: name.into(),
        active: path == current,
    })
}
