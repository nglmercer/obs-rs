#[allow(
    clippy::wildcard_imports,
    reason = "menu tests use the callback namespace as their fixture API"
)]
use super::*;
use obs_rs_project::ProjectCommand;

fn monitor(id: &str, x: i32, y: i32, width: u32, height: u32) -> MonitorChoice {
    MonitorChoice {
        id: id.to_owned(),
        name: id.to_owned(),
        x,
        y,
        width,
        height,
        primary: x == 0 && y == 0,
    }
}

#[test]
fn projector_window_center_resolves_to_one_monitor_without_crossing_bounds() {
    let monitors = [
        monitor("DP-1", 0, 0, 1_920, 1_080),
        monitor("HDMI-1", 1_920, 0, 2_560, 1_440),
    ];

    assert_eq!(
        monitor_containing_point(&monitors, 1_000, 500).map(|monitor| monitor.id.as_str()),
        Some("DP-1")
    );
    assert_eq!(
        monitor_containing_point(&monitors, 2_500, 700).map(|monitor| monitor.id.as_str()),
        Some("HDMI-1")
    );
    assert_eq!(monitor_containing_point(&monitors, 4_480, 700), None);
}

#[test]
fn projector_monitor_rows_preserve_identity_and_desktop_arrangement() {
    let monitors = [
        monitor("DP-1", -1_920, 0, 1_920, 1_080),
        monitor("HDMI-1", 0, -200, 2_560, 1_440),
    ];

    let rows = projector_monitor_rows_for(&monitors, Some("HDMI-1"));

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id.as_str(), "DP-1");
    assert!(!rows[0].selected);
    assert!(rows[1].selected);
    assert!(rows[0].normalized_x.abs() < f32::EPSILON);
    assert!(rows[1].normalized_y.abs() < f32::EPSILON);
    assert!(rows.iter().all(|row| {
        row.normalized_width > 0.0
            && row.normalized_height > 0.0
            && row.normalized_width <= 1.0
            && row.normalized_height <= 1.0
    }));
}

#[test]
fn stale_projector_monitor_selection_falls_back_to_primary_row() {
    let monitors = [
        monitor("DP-1", 0, 0, 1_920, 1_080),
        monitor("HDMI-1", 1_920, 0, 2_560, 1_440),
    ];

    let rows = projector_monitor_rows_for(&monitors, Some("gone"));

    assert!(rows[0].selected);
    assert!(!rows[1].selected);
}

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
    let file_name = collection_file_name(&"é".repeat(MAX_COLLECTION_NAME)).expect("unicode name");
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
    let current_document = std::fs::read_to_string(&current).expect("current collection was saved");
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
