use super::*;

/// Renders the dedicated Open-project mode and verifies that it is distinct
/// from the project configuration and Save As modes at the window boundary.
pub(super) fn exercise_project_open_dialog(ui: &MainWindow) {
    ui.set_project_path("obs-rs-open-project.obsrproj".into());
    ui.set_project_dialog_mode(4);
    ui.set_active_modal(1);
    assert_eq!(ui.get_project_dialog_mode(), 4);
    assert_eq!(ui.get_active_modal(), 1);

    ui.show()
        .expect("testing window should show the Open dialog");
    let snapshot = ui
        .window()
        .take_snapshot()
        .expect("Open project dialog should render");
    assert!(snapshot.width() > 0 && snapshot.height() > 0);
    ui.hide()
        .expect("testing window should hide after Open dialog");

    ui.set_collection_transfer_path("obs-rs-collection.obsrproj".into());
    for mode in [1, 2] {
        ui.set_project_dialog_mode(mode);
        ui.set_active_modal(1);
        ui.show()
            .expect("testing window should show the collection dialog");
        let snapshot = ui
            .window()
            .take_snapshot()
            .expect("collection dialog should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
        ui.hide()
            .expect("testing window should hide after collection dialog");
    }

    ui.set_project_dialog_mode(0);
    ui.set_active_modal(0);
}

pub(super) fn close_project_open_dialog(ui: &MainWindow) {
    assert_eq!(ui.get_project_dialog_mode(), 4);
    assert_eq!(ui.get_active_modal(), 1);
    ui.set_project_dialog_mode(0);
    ui.set_active_modal(0);
    ui.invoke_select_preview("preview".into());
}

pub(super) fn exercise_project_recovery_dialog(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let root = std::env::temp_dir().join(format!(
        "obs-rs-project-recovery-dialog-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("recovery fixture directory");
    let project_path = root.join("recovery.obsrproj");
    let project_text = project_path.to_string_lossy().into_owned();
    let store = crate::project_store(&project_text).expect("recovery store");
    let document = state.borrow().project_document().clone();
    std::fs::write(store.temp_path(), &document).expect("temporary recovery document");

    ui.set_project_path(project_text.into());
    ui.set_active_modal(0);
    ui.invoke_recover_project();
    assert_eq!(ui.get_active_modal(), 14);
    assert!(ui
        .get_status_message()
        .contains(store.temp_path().to_string_lossy().as_ref()));
    ui.show()
        .expect("testing window should show the recovery dialog");
    let snapshot = ui
        .window()
        .take_snapshot()
        .expect("project recovery dialog should render");
    assert!(snapshot.width() > 0 && snapshot.height() > 0);
    ui.hide()
        .expect("testing window should hide after recovery dialog");

    ui.set_active_modal(0);
    assert!(
        store.temp_path().is_file(),
        "cancel keeps the recovery file"
    );
    ui.invoke_recover_project();
    assert_eq!(ui.get_active_modal(), 14);

    std::fs::write(store.temp_path(), "not a project").expect("invalid recovery document");
    ui.invoke_recover_project();
    assert_eq!(ui.get_active_modal(), 14);
    assert!(ui.get_status_message().contains("Recovery failed"));

    std::fs::write(store.temp_path(), &document).expect("restore valid recovery document");
    ui.invoke_recover_project();
    assert_eq!(ui.get_active_modal(), 0);
    assert!(state.borrow().is_dirty());

    std::fs::remove_dir_all(root).expect("remove recovery fixture");
}
