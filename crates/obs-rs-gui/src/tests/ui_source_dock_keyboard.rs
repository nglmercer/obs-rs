use super::*;

/// Verifies clipboard and select-all shortcuts while the Sources dock owns
/// keyboard focus rather than the main canvas.
pub(super) fn exercise_source_dock_clipboard_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let source_id = "dock-keyboard-clipboard-source";
    add_source(state, scene.as_str(), source_id);
    refresh_ui(ui, state, surface);
    focus_source_dock(ui);
    select_source(state, surface, ui, source_id);

    dispatch_control_shortcut(ui, "C");
    assert!(
        state.borrow().can_paste_source(),
        "Sources dock Ctrl+C copies"
    );
    dispatch_control_shortcut(ui, "V");
    refresh_ui(ui, state, surface);
    let pasted_id = state
        .borrow()
        .selected_source()
        .expect("Sources dock Ctrl+V selects the pasted source")
        .to_owned();
    assert_ne!(pasted_id, source_id);
    assert!(source_references(
        state,
        scene.as_str(),
        pasted_id.as_str(),
        source_id
    ));

    dispatch_control_shortcut(ui, "A");
    assert!(
        state.borrow().is_source_selected(source_id)
            && state.borrow().is_source_selected(pasted_id.as_str()),
        "Sources dock Ctrl+A selects all visible sources"
    );
    for item in [source_id, pasted_id.as_str()] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
                profile: "live".to_owned(),
                scene: scene.clone(),
                item: item.to_owned(),
            }))
            .expect("remove dock keyboard clipboard fixture item");
    }
    refresh_ui(ui, state, surface);
}

fn add_source(state: &Rc<RefCell<DesktopState>>, scene: &str, id: &str) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: scene.to_owned(),
            source: SourceSpec::new(
                id,
                "color_source",
                "Dock keyboard clipboard source",
                source_settings("color_source").expect("source defaults"),
            )
            .expect("dock keyboard clipboard source"),
        }))
        .expect("add dock keyboard clipboard source");
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
        .expect("select dock keyboard clipboard source");
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

fn focus_source_dock(ui: &MainWindow) {
    let row = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .find(|row| row.size().height > 30.0)
        .expect("visible source row for keyboard focus");
    row.query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|area| area.size().width > 150.0 && area.size().height > 30.0)
        .find_first()
        .expect("source row keyboard focus target")
        .mock_single_click(PointerEventButton::Left);
}
