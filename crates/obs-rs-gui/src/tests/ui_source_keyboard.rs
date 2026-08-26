use super::*;

/// Verifies the OBS source clipboard shortcuts through the real `MainWindow`
/// keyboard boundary. Ctrl+C uses the existing Rust clipboard and Ctrl+V
/// pastes a reference through the same project command as the context menu.
pub(super) fn exercise_source_clipboard_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let source_id = "keyboard-clipboard-source";
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: scene.clone(),
            source: SourceSpec::new(
                source_id,
                "color_source",
                "Keyboard clipboard source",
                source_settings("color_source").expect("source defaults"),
            )
            .expect("keyboard clipboard source"),
        }))
        .expect("add keyboard clipboard source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: source_id.to_owned(),
        })
        .expect("select keyboard clipboard source");
    refresh_ui(ui, state, surface);
    focus_canvas(ui);

    dispatch_control_shortcut(ui, "C");
    assert!(
        state.borrow().can_paste_source(),
        "Ctrl+C copies the source"
    );

    dispatch_control_shortcut(ui, "V");
    refresh_ui(ui, state, surface);
    let pasted_id = state
        .borrow()
        .selected_source()
        .expect("Ctrl+V selects the pasted source")
        .to_owned();
    assert_ne!(pasted_id, source_id, "Ctrl+V creates a new scene item");
    assert!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene.as_str()))
            .and_then(|scene| scene.item(pasted_id.as_str()))
            .is_some_and(|item| item.source_id().as_str() == source_id),
        "Ctrl+V uses the reference paste mode"
    );

    for item in [source_id, pasted_id.as_str()] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
                profile: "live".to_owned(),
                scene: scene.clone(),
                item: item.to_owned(),
            }))
            .expect("remove keyboard clipboard fixture item");
    }
    refresh_ui(ui, state, surface);
}

fn dispatch_control_shortcut(ui: &MainWindow, key: &str) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

fn focus_canvas(ui: &MainWindow) {
    ElementHandle::find_by_element_id(ui, "CanvasEditor::surface")
        .find(|canvas| canvas.size().width > 100.0 && canvas.size().height > 100.0)
        .expect("editable canvas focus target")
        .mock_single_click(PointerEventButton::Left);
}
