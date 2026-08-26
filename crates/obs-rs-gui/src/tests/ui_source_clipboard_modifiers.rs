use super::*;

/// Verifies that modifier combinations remain available for configured
/// actions instead of falling through to the local clipboard shortcuts.
pub(super) fn exercise_source_clipboard_modifier_boundary(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let primary_id = "keyboard-clipboard-primary";
    let secondary_id = "keyboard-clipboard-secondary";
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    add_source(
        state,
        scene.as_str(),
        primary_id,
        "Keyboard clipboard primary",
    );
    add_source(
        state,
        scene.as_str(),
        secondary_id,
        "Keyboard clipboard secondary",
    );
    refresh_ui(ui, state, surface);
    focus_canvas(ui);
    select_source(state, surface, ui, primary_id);

    dispatch_modified_control_shortcut(ui, "A", Key::Shift);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec![primary_id],
        "Ctrl+Shift+A is not the local select-all shortcut"
    );
    dispatch_control_shortcut(ui, "C");
    assert!(
        state.borrow().can_paste_source(),
        "Ctrl+C copies the source"
    );
    assert_no_modified_paste(ui, state, Key::Shift, primary_id);
    assert_no_modified_paste(ui, state, Key::Alt, primary_id);

    select_source(state, surface, ui, secondary_id);
    dispatch_modified_control_shortcut(ui, "C", Key::Shift);
    dispatch_control_shortcut(ui, "V");
    refresh_ui(ui, state, surface);
    let modifier_pasted_id = state
        .borrow()
        .selected_source()
        .expect("Ctrl+V selects the pasted source after modified Ctrl+C")
        .to_owned();
    assert!(source_references(
        state,
        scene.as_str(),
        modifier_pasted_id.as_str(),
        primary_id,
    ));

    select_source(state, surface, ui, primary_id);
    dispatch_control_shortcut(ui, "V");
    refresh_ui(ui, state, surface);
    let pasted_id = state
        .borrow()
        .selected_source()
        .expect("Ctrl+V selects the regular pasted source")
        .to_owned();
    assert!(source_references(
        state,
        scene.as_str(),
        pasted_id.as_str(),
        primary_id,
    ));

    remove_items(
        state,
        scene.as_str(),
        [
            primary_id,
            secondary_id,
            modifier_pasted_id.as_str(),
            pasted_id.as_str(),
        ],
    );
    refresh_ui(ui, state, surface);
}

fn add_source(state: &Rc<RefCell<DesktopState>>, scene: &str, id: &str, name: &str) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: scene.to_owned(),
            source: SourceSpec::new(
                id,
                "color_source",
                name,
                source_settings("color_source").expect("source defaults"),
            )
            .expect("keyboard clipboard source"),
        }))
        .expect("add keyboard clipboard source");
}

fn select_source(
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    ui: &MainWindow,
    id: &str,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource { id: id.to_owned() })
        .expect("select keyboard clipboard source");
    refresh_ui(ui, state, surface);
}

fn source_references(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
    item_id: &str,
    source_id: &str,
) -> bool {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.item(item_id))
        .is_some_and(|item| item.source_id().as_str() == source_id)
}

fn assert_no_modified_paste(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    modifier: Key,
    selected_id: &str,
) {
    dispatch_modified_control_shortcut(ui, "V", modifier);
    assert_eq!(
        state.borrow().selected_source(),
        Some(selected_id),
        "modified Ctrl+V does not paste through the local shortcut"
    );
}

fn remove_items<const N: usize>(state: &Rc<RefCell<DesktopState>>, scene: &str, ids: [&str; N]) {
    for id in ids {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
                profile: "live".to_owned(),
                scene: scene.to_owned(),
                item: id.to_owned(),
            }))
            .expect("remove keyboard clipboard fixture item");
    }
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

fn dispatch_modified_control_shortcut(ui: &MainWindow, key: &str, modifier: Key) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: modifier.into(),
    });
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: modifier.into(),
    });
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
