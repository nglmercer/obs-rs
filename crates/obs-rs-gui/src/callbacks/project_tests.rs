use super::*;

fn fixture_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "obs-rs-project-save-as-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("Save As fixture directory");
    root
}

#[test]
fn save_as_publishes_the_new_document_and_marks_the_session_clean() {
    let root = fixture_root("publish");
    let current = root.join("current.obsrproj");
    let target = root.join("saved-as.obsrproj");
    let current_text = current.to_string_lossy().into_owned();
    let target_text = target.to_string_lossy().into_owned();
    let state = Rc::new(RefCell::new(DesktopState::new(
        crate::initial_project().expect("initial project"),
    )));
    let document = state.borrow().project_document();

    let bytes = save_project_as_document(&state, &current_text, &target_text)
        .expect("Save As should publish a new file");

    assert_eq!(bytes, document.len());
    assert!(!state.borrow().is_dirty());
    assert_eq!(
        std::fs::read_to_string(&target).expect("saved document"),
        document
    );
    assert!(project_store(&target_text)
        .expect("target store")
        .load()
        .is_ok());

    std::fs::remove_dir_all(root).expect("remove Save As fixture");
}

#[test]
fn save_as_rejects_an_existing_different_target_without_touching_it() {
    let root = fixture_root("conflict");
    let current = root.join("current.obsrproj");
    let target = root.join("existing.obsrproj");
    let current_text = current.to_string_lossy().into_owned();
    let target_text = target.to_string_lossy().into_owned();
    let sentinel = "do not overwrite this document";
    std::fs::write(&target, sentinel).expect("existing target");
    let state = Rc::new(RefCell::new(DesktopState::new(
        crate::initial_project().expect("initial project"),
    )));

    let error = save_project_as_document(&state, &current_text, &target_text)
        .expect_err("Save As must reject an existing different target");

    assert!(error.to_string().contains("already exists"));
    assert_eq!(
        std::fs::read_to_string(&target).expect("existing target remains"),
        sentinel
    );
    assert!(!state.borrow().is_dirty());

    std::fs::remove_dir_all(root).expect("remove Save As conflict fixture");
}

#[test]
fn project_paths_accept_current_and_legacy_extensions_only() {
    let root = fixture_root("extensions");
    let current = root.join("current.obsrproj");
    let legacy = root.join("legacy.JSON");
    let unsupported = root.join("notes.txt");

    assert_eq!(
        validate_project_path(current.to_str().expect("current path")).expect("current extension"),
        current
    );
    assert_eq!(
        validate_project_path(legacy.to_str().expect("legacy path")).expect("legacy extension"),
        legacy
    );
    let error = project_store(unsupported.to_str().expect("unsupported path"))
        .err()
        .expect("unsupported project extension");
    assert!(error.to_string().contains(".obsrproj"));
    assert!(!unsupported.exists());

    std::fs::remove_dir_all(root).expect("remove extension fixture");
}

#[test]
fn save_as_rejects_unsupported_project_extensions_before_writing() {
    let root = fixture_root("unsupported-extension");
    let current = root.join("current.obsrproj");
    let target = root.join("saved.txt");
    let state = Rc::new(RefCell::new(DesktopState::new(
        crate::initial_project().expect("initial project"),
    )));

    let error = save_project_as_document(
        &state,
        current.to_str().expect("current path"),
        target.to_str().expect("target path"),
    )
    .expect_err("unsupported Save As extension");

    assert!(error.to_string().contains(".obsrproj"));
    assert!(!target.exists());
    assert!(!state.borrow().is_dirty());

    std::fs::remove_dir_all(root).expect("remove unsupported extension fixture");
}
