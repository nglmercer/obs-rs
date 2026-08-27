use super::*;

/// Drives the real Scene-dock DragArea/DropArea path. The project command
/// remains responsible for validating the active profile and final order;
/// this fixture only supplies before/after pointer drops and checks the
/// resulting persisted scene order.
pub(crate) fn exercise_scene_pointer_drag_and_drop(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);
    clear_scene_drag_fixture(state);
    for (id, name) in [
        ("drag-scene-a", "Drag scene A"),
        ("drag-scene-b", "Drag scene B"),
        ("drag-scene-c", "Drag scene C"),
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: SceneSpec::new(id, name).expect("drag scene"),
            }))
            .expect("add drag scene");
    }
    for (id, target_index) in [
        ("drag-scene-a", 0),
        ("drag-scene-b", 1),
        ("drag-scene-c", 2),
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveScene {
                profile: "live".to_owned(),
                scene: id.to_owned(),
                target_index,
            }))
            .expect("place drag scene in visible rows");
    }
    refresh_ui(ui, state, surface);

    drag_scene_row_to(ui, "drag-scene-c", "drag-scene-a", true);
    assert_eq!(
        scene_order(state),
        vec![
            "drag-scene-c",
            "drag-scene-a",
            "drag-scene-b",
            "intermission",
            "preview",
            "program",
        ]
    );
    assert_eq!(state.borrow().preview_scene(), Some("drag-scene-c"));

    drag_scene_row_to(ui, "drag-scene-c", "drag-scene-b", false);
    assert_eq!(
        scene_order(state),
        vec![
            "drag-scene-a",
            "drag-scene-b",
            "drag-scene-c",
            "intermission",
            "preview",
            "program",
        ]
    );
    assert_eq!(state.borrow().preview_scene(), Some("drag-scene-c"));

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectPreviewScene {
            id: "preview".to_owned(),
        })
        .expect("restore preview selection before cleanup");
    clear_scene_drag_fixture(state);
    refresh_ui(ui, state, surface);
}

fn clear_scene_drag_fixture(state: &Rc<RefCell<DesktopState>>) {
    for id in ["drag-scene-a", "drag-scene-b", "drag-scene-c"] {
        let _ = state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveScene {
                profile: "live".to_owned(),
                scene: id.to_owned(),
            }));
    }
}

fn scene_order(state: &Rc<RefCell<DesktopState>>) -> Vec<String> {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .expect("active profile")
        .scenes()
        .map(|scene| scene.id().to_string())
        .collect()
}

fn drag_scene_row_to(ui: &MainWindow, source: &str, target: &str, before: bool) {
    let source_row = scene_row_element(ui, source);
    let target_row = scene_row_element(ui, target);
    let target_position = target_row.absolute_position();
    let target_size = target_row.size();
    let target_y = if before {
        target_position.y + 8.0
    } else {
        target_position.y + target_size.height - 8.0
    };
    source_row
        .query_descendants()
        .match_inherits("DragArea")
        .find_first()
        .expect("scene row DragArea")
        .mock_drag(
            LogicalPosition::new(target_position.x + target_size.width / 2.0, target_y),
            PointerEventButton::Left,
        );
}

fn scene_row_element(ui: &MainWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, id)
        .filter(|row| row.size().height > 30.0)
        .find(|_| true)
        .expect("visible scene row")
}
