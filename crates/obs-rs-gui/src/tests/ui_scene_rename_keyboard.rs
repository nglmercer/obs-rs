use super::*;

/// Verifies that the focused Scenes dock opens the existing scene properties
/// modal for the selected preview scene and commits its name through Rust.
pub(super) fn exercise_scene_dock_rename_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let original_name = ui.get_scene_name().to_string();
    let original_platform_macos = ui.get_platform_macos();
    refresh_ui(ui, state, surface);
    ui.set_platform_macos(true);
    focus_scene_dock(ui);

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::F2.into(),
    });
    assert_eq!(ui.get_active_modal(), 0, "macOS does not bind F2 to rename");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(
        ui.get_active_modal(),
        5,
        "Return opens scene properties on macOS"
    );
    assert_eq!(
        ui.get_scene_name(),
        original_name,
        "Return loads the selected preview scene name"
    );

    ui.set_scene_name("Renamed from Scenes dock".into());
    ui.invoke_rename_scene();
    assert_eq!(
        scene_name(state, "preview").as_deref(),
        Some("Renamed from Scenes dock"),
        "F2 commits through the existing scene properties command"
    );
    ui.set_active_modal(0);

    // Restore the shared fixture so later UI checks do not inherit this
    // keyboard-specific edit. The normal refresh path also keeps transition
    // fields synchronized before the second modal submission.
    refresh_ui(ui, state, surface);
    ui.set_active_modal(5);
    ui.set_scene_name(original_name.into());
    ui.invoke_rename_scene();
    ui.set_active_modal(0);
    refresh_ui(ui, state, surface);
    ui.set_platform_macos(original_platform_macos);
}

fn focus_scene_dock(ui: &MainWindow) {
    let row = ElementHandle::find_by_accessible_label(ui, "preview")
        .find(|row| row.size().height > 30.0)
        .expect("visible scene row for rename focus");
    row.query_descendants()
        .match_inherits("TouchArea")
        .find_first()
        .expect("scene row rename focus target")
        .mock_single_click(PointerEventButton::Left);
}

fn scene_name(state: &Rc<RefCell<DesktopState>>, id: &str) -> Option<String> {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(id))
        .map(|scene| scene.name().to_owned())
}
