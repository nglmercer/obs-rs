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

    exercise_scene_properties(ui, state, &original_name);
    exercise_scene_creation(ui, state, surface);
    exercise_source_rename(ui, state, surface);

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

fn exercise_scene_properties(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>, original: &str) {
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
        scene_name(state, "preview").as_deref(),
        Some(original),
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
        scene_name(state, "preview").as_deref(),
        Some(original),
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
}

fn exercise_scene_creation(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let added_scene = "modal-enter-scene";
    ui.set_active_modal(2);
    ui.set_new_scene_id(added_scene.into());
    ui.set_new_scene_name("Added from modal Enter".into());
    ui.window()
        .take_snapshot()
        .expect("scene dialog should render before Enter dispatch");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(ui.get_active_modal(), 0, "scene Enter closes the dialog");
    assert_eq!(
        scene_name(state, added_scene).as_deref(),
        Some("Added from modal Enter"),
        "scene Enter invokes the existing add-scene callback"
    );
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: added_scene.to_owned(),
        }))
        .expect("remove modal keyboard fixture scene");
    refresh_ui(ui, state, surface);
}

fn exercise_source_rename(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let source_id = "background";
    let original_source_name = source_name(state, source_id).expect("background source name");
    ui.set_active_modal(12);
    ui.set_source_rename_target(source_id.into());
    ui.set_source_name_draft("Renamed from modal Enter".into());
    ui.window()
        .take_snapshot()
        .expect("source rename dialog should render before Enter dispatch");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(ui.get_active_modal(), 0, "source Enter closes the dialog");
    assert_eq!(
        source_name(state, source_id).as_deref(),
        Some("Renamed from modal Enter"),
        "source Enter invokes the existing rename callback"
    );
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSourceName {
            profile: "live".to_owned(),
            source: source_id.to_owned(),
            name: original_source_name,
        }))
        .expect("restore modal keyboard fixture source");
    refresh_ui(ui, state, surface);
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

fn source_name(state: &Rc<RefCell<DesktopState>>, id: &str) -> Option<String> {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source(id))
        .map(|source| source.name().to_owned())
}
