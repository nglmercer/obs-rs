use super::*;

/// Drives the real Slint DragArea/DropArea path for source reparenting. The
/// project command remains responsible for target resolution, lock checks,
/// ordering, and transactional mutation; this fixture only supplies pointer
/// gestures and verifies the resulting stable selection path.
pub(crate) fn exercise_source_pointer_drag_and_drop(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    clear_previous_source_drag_fixture(state);
    create_source_drag_fixture(state);
    refresh_ui(ui, state, surface);
    exercise_source_drag_order(ui, state);
    exercise_locked_source_drag(ui, state, surface);
    exercise_nested_context_targets(ui, state, surface);
    cleanup_source_drag_fixture(state);
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "background".to_owned(),
        })
        .expect("restore source selection after drag fixture");
    refresh_ui(ui, state, surface);
}

fn clear_previous_source_drag_fixture(state: &Rc<RefCell<DesktopState>>) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "keyboard-delete-locked".to_owned(),
            locked: false,
        }))
        .expect("unlock previous source fixture");
    for id in [
        "keyboard-delete-first",
        "keyboard-delete-second",
        "keyboard-delete-locked",
        "mouse-select-first",
        "mouse-select-second",
        "mouse-select-third",
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: id.to_owned(),
            }))
            .expect("remove previous source fixture");
    }
}

fn create_source_drag_fixture(state: &Rc<RefCell<DesktopState>>) {
    for id in ["drag-source-a", "drag-source-b", "drag-source-c"] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("drag source defaults"),
                )
                .expect("drag source"),
            }))
            .expect("add drag source");
    }
    let mut group =
        SceneItemSpec::for_group("drag-group", "Drag group").expect("drag destination group");
    group
        .group_mut()
        .expect("drag destination group body")
        .add_item(
            SceneItemSpec::new("drag-group-child", "drag-source-a")
                .expect("drag destination child"),
        )
        .expect("add drag destination child");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        }))
        .expect("add drag destination group");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::MoveSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "drag-group".to_owned(),
            target_index: 0,
        }))
        .expect("place drag destination group");
    for (item, target_index) in [
        ("drag-source-c", 1),
        ("drag-source-b", 2),
        ("drag-source-a", 3),
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveSceneItem {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: item.to_owned(),
                target_index,
            }))
            .expect("place drag source near the visible dock head");
    }
}

fn exercise_source_drag_order(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    drag_source_row_to(ui, "drag-source-c", "drag-group");
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["drag-group/drag-source-c"],
        "dropping on a group selects the reparented path"
    );
    assert_eq!(
        group_child_ids(state, "drag-group"),
        vec!["drag-source-c", "drag-group-child"],
        "a group drop inserts the item at the front"
    );

    drag_source_row_to(ui, "drag-source-b", "drag-group/drag-group-child");
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["drag-group/drag-source-b"],
        "dropping on a leaf selects the new nested path"
    );
    assert_eq!(
        group_child_ids(state, "drag-group"),
        vec!["drag-source-c", "drag-group-child", "drag-source-b"],
        "a center drop after a leaf preserves the requested order"
    );
}

fn exercise_locked_source_drag(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "drag-group".to_owned(),
            locked: true,
        }))
        .expect("lock drag destination");
    refresh_ui(ui, state, surface);
    drag_source_row_to(ui, "drag-source-a", "drag-group");
    assert_eq!(
        group_child_ids(state, "drag-group"),
        vec!["drag-source-c", "drag-group-child", "drag-source-b"],
        "a locked destination rejects pointer drops without mutation"
    );
}

fn exercise_nested_context_targets(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "drag-group".to_owned(),
            locked: false,
        }))
        .expect("unlock context target group");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "background".to_owned(),
        })
        .expect("select unrelated source before nested context action");
    refresh_ui(ui, state, surface);

    ui.invoke_remove_source("drag-group/drag-group-child".into());
    assert_eq!(
        group_child_ids(state, "drag-group"),
        vec!["drag-source-c", "drag-source-b"],
        "nested remove uses the clicked stable path instead of stale selection"
    );
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "removing a nested target preserves an unrelated canvas selection"
    );

    ui.invoke_open_source_rename("drag-group/drag-source-c".into());
    assert_eq!(
        ui.get_source_name_draft(),
        "drag-source-c",
        "nested rename resolves the clicked stable path"
    );
    ui.invoke_select_source("background".into());
    ui.set_source_name_draft("Nested drag source".into());
    ui.invoke_apply_source_name();
    assert!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.source("drag-source-c"))
            .is_some_and(|source| source.name() == "Nested drag source"),
        "nested rename commits to the clicked source definition"
    );
    ui.set_active_modal(0);
    refresh_ui(ui, state, surface);
}

fn cleanup_source_drag_fixture(state: &Rc<RefCell<DesktopState>>) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "drag-group".to_owned(),
            locked: false,
        }))
        .expect("unlock drag destination");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "drag-group".to_owned(),
        }))
        .expect("remove drag destination group");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "drag-source-a".to_owned(),
        }))
        .expect("remove drag source");
}

fn group_child_ids(state: &Rc<RefCell<DesktopState>>, group_id: &str) -> Vec<String> {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(group_id))
        .and_then(SceneItemSpec::group)
        .map(|group| {
            group
                .items()
                .iter()
                .map(|item| item.id().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn drag_source_row_to(ui: &MainWindow, source: &str, target: &str) {
    let source_row = source_row_element(ui, source);
    let target_row = source_row_element(ui, target);
    let target_position = target_row.absolute_position();
    let target_size = target_row.size();
    let target_center = LogicalPosition::new(
        target_position.x + target_size.width / 2.0,
        target_position.y + target_size.height / 2.0,
    );
    source_row
        .query_descendants()
        .match_inherits("DragArea")
        .find_first()
        .expect("source row DragArea")
        .mock_drag(target_center, PointerEventButton::Left);
}

fn source_row_element(ui: &MainWindow, target: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, target)
        .filter(|row| row.size().height > 30.0)
        .find(|_| true)
        .expect("visible source row")
}
