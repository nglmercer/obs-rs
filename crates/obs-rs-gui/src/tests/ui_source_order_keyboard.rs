use super::*;

/// Verifies OBS source-order shortcuts through both the editable canvas and
/// the focused Sources dock. The callbacks reuse the existing ordered project
/// command, so each key press remains one undoable scene edit.
pub(super) fn exercise_source_order_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let ids = [
        "keyboard-order-first",
        "keyboard-order-middle",
        "keyboard-order-last",
    ];
    for id in ids {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: scene.clone(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("source defaults"),
                )
                .expect("keyboard order source"),
            }))
            .expect("add keyboard order source");
    }
    refresh_ui(ui, state, surface);

    focus_canvas(ui);
    select_source_and_refresh(ui, state, surface, ids[1]);
    dispatch_control_shortcut(ui, Key::UpArrow.into());
    assert_tail_order(state, &scene, [ids[1], ids[0], ids[2]]);

    dispatch_control_shortcut(ui, Key::DownArrow.into());
    assert_tail_order(state, &scene, ids);

    focus_source_dock(ui);
    select_source_and_refresh(ui, state, surface, ids[1]);
    dispatch_control_shortcut(ui, Key::Home.into());
    assert_first_item(state, &scene, ids[1]);

    dispatch_control_shortcut(ui, Key::End.into());
    assert_last_item(state, &scene, ids[1]);

    for id in ids {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
                profile: "live".to_owned(),
                scene: scene.clone(),
                item: id.to_owned(),
            }))
            .expect("remove keyboard order fixture item");
    }
    refresh_ui(ui, state, surface);
}

fn select_source_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    id: &str,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource { id: id.to_owned() })
        .expect("select keyboard order source");
    refresh_ui(ui, state, surface);
}

fn assert_tail_order<const N: usize>(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
    expected: [&str; N],
) {
    let actual = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .map(|scene| {
            scene
                .items()
                .iter()
                .rev()
                .take(N)
                .map(|item| item.id().to_string())
                .collect::<Vec<_>>()
        })
        .expect("preview scene for keyboard order assertion");
    let expected = expected
        .into_iter()
        .rev()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "keyboard source order");
}

fn assert_first_item(state: &Rc<RefCell<DesktopState>>, scene_id: &str, expected: &str) {
    let actual = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.items().first())
        .map(|item| item.id().to_string());
    assert_eq!(actual.as_deref(), Some(expected), "Ctrl+Home source order");
}

fn assert_last_item(state: &Rc<RefCell<DesktopState>>, scene_id: &str, expected: &str) {
    let actual = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.items().last())
        .map(|item| item.id().to_string());
    assert_eq!(actual.as_deref(), Some(expected), "Ctrl+End source order");
}

fn dispatch_control_shortcut(ui: &MainWindow, key: slint::SharedString) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key });
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
