use super::*;

/// Verifies that the active modal owns Escape/Enter and prevents an unfinished
/// draft from reaching the project or the main-window shortcut boundary.
pub(super) fn exercise_modal_keyboard_boundary(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    ui.invoke_select_preview("preview".into());
    refresh_ui(ui, state, surface);
    let original_name = scene_name(state, "preview").expect("preview scene name");
    ui.show()
        .expect("modal keyboard fixture window should show");

    ui.set_active_modal(5);
    ui.set_scene_name("Canceled modal edit".into());
    ui.window()
        .take_snapshot()
        .expect("active modal should render before key dispatch");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    assert_eq!(ui.get_active_modal(), 0, "Escape closes the active modal");
    assert_eq!(
        scene_name(state, "preview"),
        Some(original_name.clone()),
        "Escape does not commit the transient draft"
    );

    ui.set_active_modal(5);
    ui.set_scene_name("Modified Enter draft".into());
    ui.window()
        .take_snapshot()
        .expect("active modal should render before modified Enter dispatch");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(
        ui.get_active_modal(),
        5,
        "modified Enter does not accept the active modal"
    );
    assert_eq!(
        scene_name(state, "preview"),
        Some(original_name.clone()),
        "modified Enter does not commit the transient draft"
    );
    ui.set_active_modal(0);

    ui.set_active_modal(5);
    ui.set_scene_name("Committed with Enter".into());
    ui.window()
        .take_snapshot()
        .expect("active modal should render before Enter dispatch");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(ui.get_active_modal(), 0, "Enter accepts the active modal");
    assert_eq!(
        scene_name(state, "preview").as_deref(),
        Some("Committed with Enter"),
        "Enter uses the existing scene-properties callback"
    );

    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_owned();
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneName {
            profile: profile.to_string(),
            scene: "preview".to_owned(),
            name: original_name,
        }))
        .expect("restore modal keyboard fixture scene");
    refresh_ui(ui, state, surface);
    ui.hide()
        .expect("modal keyboard fixture window should hide");
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
