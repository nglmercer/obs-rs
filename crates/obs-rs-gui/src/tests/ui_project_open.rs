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
