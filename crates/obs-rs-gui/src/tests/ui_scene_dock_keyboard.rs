use super::*;

/// Verifies that the focused Scenes dock maps OBS's two scene-removal keys to
/// the existing Rust callback and keeps the current preview selection valid.
pub(super) fn exercise_scene_dock_delete_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    for (id, key) in [
        ("dock-keyboard-delete-scene", Key::Delete),
        ("dock-keyboard-backspace-scene", Key::Backspace),
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: SceneSpec::new(id, id).expect("dock keyboard scene"),
            }))
            .expect("add dock keyboard scene");
        refresh_ui(ui, state, surface);

        ui.invoke_select_preview(id.into());
        refresh_ui(ui, state, surface);
        focus_scene_row(ui, id);

        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });

        assert!(
            !scene_exists(state, id),
            "Scenes dock removal key must remove the selected scene"
        );
        assert_ne!(
            state.borrow().preview_scene(),
            Some(id),
            "removing the preview scene must restore a valid fallback"
        );
    }
}

fn focus_scene_row(ui: &MainWindow, id: &str) {
    let row = ElementHandle::find_by_accessible_label(ui, id)
        .find(|row| row.size().height > 30.0)
        .expect("visible scene row for keyboard removal");
    row.query_descendants()
        .match_inherits("TouchArea")
        .find_first()
        .expect("scene row keyboard removal focus target")
        .mock_single_click(PointerEventButton::Left);
}

fn scene_exists(state: &Rc<RefCell<DesktopState>>, id: &str) -> bool {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .is_some_and(|profile| profile.scene(id).is_some())
}
