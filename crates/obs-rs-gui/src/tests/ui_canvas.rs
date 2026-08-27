use super::*;

/// Exercises the editable canvas through the testing backend's real pointer
/// event path. The starter background is hidden so the center of the canvas is
/// an empty drag origin; two bounded color sources then prove replacement and
/// Ctrl-additive drag-box selection. Middle-drag and Space+drag also exercise
/// transient pan, and wheel zoom exercises cursor-anchored viewport updates,
/// before the fixture restores the scene.
const CANVAS_POINTER_SOURCES: [&str; 2] = ["canvas-pointer-left", "canvas-pointer-right"];
const OVERLAPPING_CANVAS_POINTER_SOURCES: [&str; 2] =
    ["canvas-pointer-under", "canvas-pointer-top"];
const TRANSFORM_CANVAS_POINTER_SOURCE: &str = "canvas-pointer-transform";
const ROTATION_CANVAS_POINTER_SOURCE: &str = "canvas-pointer-rotation";
const CROP_CANVAS_POINTER_SOURCE: &str = "canvas-pointer-crop";

pub(super) fn exercise_canvas_pointer_fixture(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    prepare_canvas_pointer_scene(state, surface, ui);
    exercise_transform_handle(ui, state, surface);
    exercise_rotation_handle(ui, state, surface);
    exercise_crop_handle(ui, state, surface);
    exercise_drag_selection(ui, state);
    exercise_overlapping_selection(ui, state, surface);
    exercise_pan_and_zoom(ui);
    exercise_nested_canvas_pointer(ui, state, surface);
    restore_canvas_pointer_scene(ui, state, surface);
}

fn exercise_transform_handle(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            source: SourceSpec::new(
                TRANSFORM_CANVAS_POINTER_SOURCE,
                "color_source",
                TRANSFORM_CANVAS_POINTER_SOURCE,
                source_settings("color_source").expect("transform pointer defaults"),
            )
            .expect("transform pointer source"),
        }))
        .expect("add transform pointer source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: TRANSFORM_CANVAS_POINTER_SOURCE.to_owned(),
            transform: FrameTransform::new(400, 300, 650, 320, false, false, u8::MAX)
                .expect("transform pointer transform"),
        }))
        .expect("position transform pointer source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: TRANSFORM_CANVAS_POINTER_SOURCE.to_owned(),
        })
        .expect("select transform pointer source");
    refresh_ui(ui, state, surface);

    let before = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(TRANSFORM_CANVAS_POINTER_SOURCE))
        .expect("transform pointer item before drag")
        .transform();
    let handle_x = ui
        .get_item_handle_x()
        .row_data(4)
        .expect("bottom-right handle x");
    let handle_y = ui
        .get_item_handle_y()
        .row_data(4)
        .expect("bottom-right handle y");
    let canvas = canvas_surface(ui);
    let start = canvas_point(ui, &canvas, handle_x, handle_y);
    let end = LogicalPosition::new(start.x + 18.0, start.y + 12.0);
    drag_canvas_at(ui, start, end, PointerEventButton::Left);

    let after = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(TRANSFORM_CANVAS_POINTER_SOURCE))
        .expect("transform pointer item after drag")
        .transform();
    assert!(
        after.scale_x_milli() > before.scale_x_milli()
            || after.scale_y_milli() > before.scale_y_milli(),
        "the real bottom-right handle drag resizes the selected source"
    );

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: vec![TRANSFORM_CANVAS_POINTER_SOURCE.to_owned()],
        }))
        .expect("remove transform pointer source");
    refresh_ui(ui, state, surface);
}

fn exercise_rotation_handle(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            source: SourceSpec::new(
                ROTATION_CANVAS_POINTER_SOURCE,
                "color_source",
                ROTATION_CANVAS_POINTER_SOURCE,
                source_settings("color_source").expect("rotation pointer defaults"),
            )
            .expect("rotation pointer source"),
        }))
        .expect("add rotation pointer source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: ROTATION_CANVAS_POINTER_SOURCE.to_owned(),
            transform: FrameTransform::new(400, 300, 650, 320, false, false, u8::MAX)
                .expect("rotation pointer transform"),
        }))
        .expect("position rotation pointer source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: ROTATION_CANVAS_POINTER_SOURCE.to_owned(),
        })
        .expect("select rotation pointer source");
    refresh_ui(ui, state, surface);
    assert!(
        ui.get_rotation_handle_active(),
        "a selected unlocked source exposes the rotation handle"
    );

    let before = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(ROTATION_CANVAS_POINTER_SOURCE))
        .expect("rotation pointer item before drag")
        .transform();
    let rotation_x = ui.get_rotation_handle_x();
    let rotation_y = ui.get_rotation_handle_y();
    let center_x = ui.get_item_x() + ui.get_item_width() / 2;
    let center_y = ui.get_item_y() + ui.get_item_height() / 2;
    let canvas = canvas_surface(ui);
    let start = canvas_point(ui, &canvas, rotation_x, rotation_y);
    let end = canvas_point(ui, &canvas, center_x + ui.get_item_width() / 2, center_y);
    drag_canvas_at(ui, start, end, PointerEventButton::Left);

    let after = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(ROTATION_CANVAS_POINTER_SOURCE))
        .expect("rotation pointer item after drag")
        .transform();
    assert_ne!(
        after.rotation_milli_degrees(),
        before.rotation_milli_degrees(),
        "the real rotation handle drag changes the selected source angle"
    );

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: vec![ROTATION_CANVAS_POINTER_SOURCE.to_owned()],
        }))
        .expect("remove rotation pointer source");
    refresh_ui(ui, state, surface);
}

fn exercise_crop_handle(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            source: SourceSpec::new(
                CROP_CANVAS_POINTER_SOURCE,
                "color_source",
                CROP_CANVAS_POINTER_SOURCE,
                source_settings("color_source").expect("crop pointer defaults"),
            )
            .expect("crop pointer source"),
        }))
        .expect("add crop pointer source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: CROP_CANVAS_POINTER_SOURCE.to_owned(),
            transform: FrameTransform::new(400, 300, 650, 320, false, false, u8::MAX)
                .expect("crop pointer transform"),
        }))
        .expect("position crop pointer source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: CROP_CANVAS_POINTER_SOURCE.to_owned(),
        })
        .expect("select crop pointer source");
    refresh_ui(ui, state, surface);

    let before = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(CROP_CANVAS_POINTER_SOURCE))
        .expect("crop pointer item before drag")
        .transform();
    let handle_x = ui
        .get_item_handle_x()
        .row_data(7)
        .expect("left-middle crop handle x");
    let handle_y = ui
        .get_item_handle_y()
        .row_data(7)
        .expect("left-middle crop handle y");
    let canvas = canvas_surface(ui);
    let start = canvas_point(ui, &canvas, handle_x, handle_y);
    let end = LogicalPosition::new(start.x + 18.0, start.y);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Alt.into(),
    });
    drag_canvas_at(ui, start, end, PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Alt.into(),
    });

    let after = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item(CROP_CANVAS_POINTER_SOURCE))
        .expect("crop pointer item after drag")
        .transform();
    assert!(
        after.crop_left() > before.crop_left(),
        "Alt plus the left-middle handle crops the source edge"
    );
    assert_eq!(
        after.scale_x_milli(),
        before.scale_x_milli(),
        "Alt crop does not resize the scene item horizontally"
    );

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: vec![CROP_CANVAS_POINTER_SOURCE.to_owned()],
        }))
        .expect("remove crop pointer source");
    refresh_ui(ui, state, surface);
}

fn prepare_canvas_pointer_scene(
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    ui: &MainWindow,
) {
    for id in CANVAS_POINTER_SOURCES {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("canvas pointer defaults"),
                )
                .expect("canvas pointer source"),
            }))
            .expect("add canvas pointer source");
    }
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background".to_owned(),
            visible: false,
        }))
        .expect("hide starter canvas background");
    for (id, x, y) in [
        ("canvas-pointer-left", 200, 100),
        ("canvas-pointer-right", 1_300, 700),
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: id.to_owned(),
                transform: FrameTransform::new(250, 250, x, y, false, false, u8::MAX)
                    .expect("canvas pointer transform"),
            }))
            .expect("position canvas pointer source");
    }
    refresh_ui(ui, state, surface);
}

fn exercise_drag_selection(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let canvas = canvas_surface(ui);
    canvas.mock_drag(canvas_point(ui, &canvas, 80, 80), PointerEventButton::Left);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-pointer-left"],
        "a blank-space drag selects the intersecting source"
    );

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    let canvas = canvas_surface(ui);
    canvas.mock_drag(
        canvas_point(ui, &canvas, 1_880, 1_040),
        PointerEventButton::Left,
    );
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-pointer-left", "canvas-pointer-right"],
        "Ctrl drag adds the second intersecting source"
    );
}

fn exercise_overlapping_selection(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    for id in OVERLAPPING_CANVAS_POINTER_SOURCES {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("overlap pointer defaults"),
                )
                .expect("overlap pointer source"),
            }))
            .expect("add overlap pointer source");
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: id.to_owned(),
                transform: FrameTransform::new(250, 250, 400, 300, false, false, u8::MAX)
                    .expect("overlap pointer transform"),
            }))
            .expect("position overlap pointer source");
    }
    refresh_ui(ui, state, surface);
    let canvas = canvas_surface(ui);
    let point = canvas_point(ui, &canvas, 520, 420);
    click_canvas_at(ui, point);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-pointer-top"],
        "plain canvas click selects the topmost overlapping source"
    );
    click_canvas_at(ui, point);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-pointer-under"],
        "plain canvas click walks below an already-selected hit"
    );
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    click_canvas_at(ui, point);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-pointer-under", "canvas-pointer-top"],
        "Ctrl canvas click toggles the topmost overlapping source"
    );
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: OVERLAPPING_CANVAS_POINTER_SOURCES
                .iter()
                .map(ToString::to_string)
                .collect(),
        }))
        .expect("remove overlap pointer sources");
    refresh_ui(ui, state, surface);
}

fn exercise_pan_and_zoom(ui: &MainWindow) {
    let initial_pan = (ui.get_canvas_pan_x(), ui.get_canvas_pan_y());
    let canvas = canvas_surface(ui);
    let center = canvas_center(&canvas);
    canvas.mock_drag(
        LogicalPosition::new(center.x + 24.0, center.y - 12.0),
        PointerEventButton::Middle,
    );
    let middle_pan = (ui.get_canvas_pan_x(), ui.get_canvas_pan_y());
    assert_ne!(middle_pan, initial_pan, "middle drag pans the canvas");

    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
    let canvas = canvas_surface(ui);
    let center = canvas.absolute_position();
    let center = LogicalPosition::new(
        center.x + canvas.size().width / 2.0,
        center.y + canvas.size().height / 2.0,
    );
    canvas.mock_drag(
        LogicalPosition::new(center.x - 18.0, center.y + 14.0),
        PointerEventButton::Left,
    );
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
    let space_pan = (ui.get_canvas_pan_x(), ui.get_canvas_pan_y());
    assert_ne!(space_pan, middle_pan, "Space+drag pans the canvas");
    ui.invoke_canvas_pan_dragged(
        initial_pan.0.saturating_sub(space_pan.0),
        initial_pan.1.saturating_sub(space_pan.1),
    );
    assert_eq!(
        (ui.get_canvas_pan_x(), ui.get_canvas_pan_y()),
        initial_pan,
        "pointer pan is transient and restored for the remaining fixture"
    );

    let initial_zoom = ui.get_canvas_zoom();
    let zoom_pan = (ui.get_canvas_pan_x(), ui.get_canvas_pan_y());
    canvas_surface(ui).scroll(0.0, 1.0);
    assert_ne!(
        ui.get_canvas_zoom(),
        initial_zoom,
        "wheel scroll changes the continuous canvas zoom"
    );
    ui.invoke_canvas_zoom_changed(initial_zoom);
    let after_zoom_restore = (ui.get_canvas_pan_x(), ui.get_canvas_pan_y());
    ui.invoke_canvas_pan_dragged(
        zoom_pan.0.saturating_sub(after_zoom_restore.0),
        zoom_pan.1.saturating_sub(after_zoom_restore.1),
    );
    assert_eq!(
        (
            ui.get_canvas_zoom(),
            ui.get_canvas_pan_x(),
            ui.get_canvas_pan_y()
        ),
        (initial_zoom, zoom_pan.0, zoom_pan.1),
        "wheel zoom state is restored for the remaining fixture"
    );
}

fn exercise_nested_canvas_pointer(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    setup_nested_canvas_pointer_scene(state);
    refresh_ui(ui, state, surface);
    exercise_nested_group_handle(ui, state);
    exercise_nested_scene_reference_handle(ui, state);
    cleanup_nested_canvas_pointer_scene(state);
    refresh_ui(ui, state, surface);
}

fn setup_nested_canvas_pointer_scene(state: &Rc<RefCell<DesktopState>>) {
    let mut group = SceneItemSpec::for_group("canvas-nested-group", "Canvas nested group")
        .expect("nested canvas group");
    let mut group_child =
        SceneItemSpec::new("canvas-nested-group-child", "background").expect("nested group child");
    group_child.set_transform(
        FrameTransform::new(250, 250, 100, 100, false, false, u8::MAX)
            .expect("nested group child transform"),
    );
    group.set_transform(
        FrameTransform::new(400, 400, 300, 200, false, false, u8::MAX)
            .expect("nested group transform"),
    );
    group
        .group_mut()
        .expect("nested group body")
        .add_item(group_child)
        .expect("attach nested group child");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        }))
        .expect("add nested canvas group");

    let mut child_scene =
        SceneSpec::new("canvas-nested-scene", "Canvas nested scene").expect("nested canvas scene");
    let mut reference_leaf = SceneItemSpec::new("canvas-nested-reference-leaf", "background")
        .expect("nested Scene-reference leaf");
    reference_leaf.set_transform(
        FrameTransform::new(250, 250, 100, 100, false, false, u8::MAX)
            .expect("nested Scene-reference leaf transform"),
    );
    child_scene
        .add_item(reference_leaf)
        .expect("attach nested Scene-reference leaf");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child_scene,
        }))
        .expect("add nested canvas scene");
    let mut reference = SceneItemSpec::for_scene("canvas-nested-reference", "canvas-nested-scene")
        .expect("nested canvas reference");
    reference.set_transform(
        FrameTransform::new(400, 400, 800, 650, false, false, u8::MAX)
            .expect("nested canvas reference transform"),
    );
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: reference,
        }))
        .expect("add nested canvas reference");
}

fn exercise_nested_group_handle(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let canvas = canvas_surface(ui);
    let body_start = canvas_point(ui, &canvas, 436, 294);
    click_canvas_at(ui, body_start);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-nested-group/canvas-nested-group-child"],
        "canvas pointer selects a nested group leaf by its stable path"
    );
    assert!(
        ui.get_item_active(),
        "nested group leaf exposes an active overlay"
    );
    assert!(!ui.get_item_locked(), "nested group leaf remains editable");
    let group_parent_before = canvas_target_transform(state, "canvas-nested-group");
    let group_child_before_body =
        canvas_target_transform(state, "canvas-nested-group/canvas-nested-group-child");
    drag_canvas_at(
        ui,
        body_start,
        LogicalPosition::new(body_start.x + 18.0, body_start.y + 12.0),
        PointerEventButton::Left,
    );
    let group_parent_after_body = canvas_target_transform(state, "canvas-nested-group");
    let group_child_after_body =
        canvas_target_transform(state, "canvas-nested-group/canvas-nested-group-child");
    assert_ne!(
        group_child_after_body, group_child_before_body,
        "nested group body drag updates the local child transform"
    );
    assert_eq!(
        group_parent_after_body, group_parent_before,
        "nested group body drag preserves the container transform"
    );
    let group_child_before_handle =
        canvas_target_transform(state, "canvas-nested-group/canvas-nested-group-child");
    let group_handle_x = ui
        .get_item_handle_x()
        .row_data(4)
        .expect("nested group bottom-right handle x");
    let group_handle_y = ui
        .get_item_handle_y()
        .row_data(4)
        .expect("nested group bottom-right handle y");
    let canvas = canvas_surface(ui);
    drag_canvas_at(
        ui,
        canvas_point(ui, &canvas, group_handle_x, group_handle_y),
        LogicalPosition::new(
            canvas_point(ui, &canvas, group_handle_x, group_handle_y).x + 18.0,
            canvas_point(ui, &canvas, group_handle_x, group_handle_y).y + 12.0,
        ),
        PointerEventButton::Left,
    );
    let group_parent_after = canvas_target_transform(state, "canvas-nested-group");
    let group_child_after =
        canvas_target_transform(state, "canvas-nested-group/canvas-nested-group-child");
    assert_ne!(
        group_child_after, group_child_before_handle,
        "nested group handle drag updates the local child transform"
    );
    assert_eq!(
        group_parent_after, group_parent_before,
        "nested group pointer drag preserves the container transform"
    );
}

fn exercise_nested_scene_reference_handle(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let canvas = canvas_surface(ui);
    let body_start = canvas_point(ui, &canvas, 936, 744);
    click_canvas_at(ui, body_start);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["canvas-nested-reference/canvas-nested-reference-leaf"],
        "canvas pointer selects a Scene-reference leaf by its stable path"
    );
    let reference_parent_before = canvas_target_transform(state, "canvas-nested-reference");
    let reference_leaf_before_body = canvas_target_transform(
        state,
        "canvas-nested-reference/canvas-nested-reference-leaf",
    );
    drag_canvas_at(
        ui,
        body_start,
        LogicalPosition::new(body_start.x + 18.0, body_start.y + 12.0),
        PointerEventButton::Left,
    );
    let reference_parent_after_body = canvas_target_transform(state, "canvas-nested-reference");
    let reference_leaf_after_body = canvas_target_transform(
        state,
        "canvas-nested-reference/canvas-nested-reference-leaf",
    );
    assert_ne!(
        reference_leaf_after_body, reference_leaf_before_body,
        "Scene-reference body drag updates the owning scene leaf"
    );
    assert_eq!(
        reference_parent_after_body, reference_parent_before,
        "Scene-reference body drag preserves the parent reference transform"
    );
    let reference_leaf_before_handle = canvas_target_transform(
        state,
        "canvas-nested-reference/canvas-nested-reference-leaf",
    );
    let reference_handle_x = ui
        .get_item_handle_x()
        .row_data(4)
        .expect("Scene-reference bottom-right handle x");
    let reference_handle_y = ui
        .get_item_handle_y()
        .row_data(4)
        .expect("Scene-reference bottom-right handle y");
    let canvas = canvas_surface(ui);
    drag_canvas_at(
        ui,
        canvas_point(ui, &canvas, reference_handle_x, reference_handle_y),
        LogicalPosition::new(
            canvas_point(ui, &canvas, reference_handle_x, reference_handle_y).x + 18.0,
            canvas_point(ui, &canvas, reference_handle_x, reference_handle_y).y + 12.0,
        ),
        PointerEventButton::Left,
    );
    let reference_parent_after = canvas_target_transform(state, "canvas-nested-reference");
    let reference_leaf_after = canvas_target_transform(
        state,
        "canvas-nested-reference/canvas-nested-reference-leaf",
    );
    assert_ne!(
        reference_leaf_after, reference_leaf_before_handle,
        "Scene-reference handle drag updates the owning scene leaf"
    );
    assert_eq!(
        reference_parent_after, reference_parent_before,
        "Scene-reference pointer drag preserves the parent reference transform"
    );
}

fn cleanup_nested_canvas_pointer_scene(state: &Rc<RefCell<DesktopState>>) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: vec![
                "canvas-nested-group".to_owned(),
                "canvas-nested-reference".to_owned(),
            ],
        }))
        .expect("remove nested canvas roots");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "canvas-nested-scene".to_owned(),
        }))
        .expect("remove nested canvas scene");
}

fn canvas_target_transform(state: &Rc<RefCell<DesktopState>>, target: &str) -> FrameTransform {
    let state = state.borrow();
    let profile = state
        .project_session()
        .project()
        .active_profile_spec()
        .expect("active profile for nested canvas target");
    crate::callbacks::canvas::canvas_item_for_target(profile, "preview", target)
        .expect("nested canvas target")
        .transform()
}

fn restore_canvas_pointer_scene(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: CANVAS_POINTER_SOURCES
                .iter()
                .map(ToString::to_string)
                .collect(),
        }))
        .expect("remove canvas pointer sources");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background".to_owned(),
            visible: true,
        }))
        .expect("restore starter canvas background");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "background".to_owned(),
        })
        .expect("restore canvas selection");
    refresh_ui(ui, state, surface);
}

fn canvas_surface(ui: &MainWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "CanvasEditor::surface")
        .find(|canvas| canvas.size().width > 100.0 && canvas.size().height > 100.0)
        .expect("editable canvas surface")
}

fn canvas_center(canvas: &ElementHandle) -> LogicalPosition {
    let origin = canvas.absolute_position();
    let size = canvas.size();
    LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

fn click_canvas_at(ui: &MainWindow, position: LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "testing coordinates are bounded desktop/canvas dimensions"
)]
fn drag_canvas_at(
    ui: &MainWindow,
    start: LogicalPosition,
    end: LogicalPosition,
    button: PointerEventButton,
) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button,
    });
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let distance = (dx * dx + dy * dy).sqrt();
    if distance > f32::EPSILON {
        let steps = ((distance / 5.0).ceil() as usize).max(2);
        for step in 1..steps {
            let progress = step as f32 / steps as f32;
            let position = LogicalPosition::new(start.x + dx * progress, start.y + dy * progress);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
            ui.window()
                .dispatch_event(WindowEvent::PointerMoved { position });
        }
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position: end });
    }
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button,
    });
}

#[allow(
    clippy::cast_precision_loss,
    reason = "testing coordinates are bounded desktop/canvas dimensions"
)]
fn canvas_point(ui: &MainWindow, canvas: &ElementHandle, x: i32, y: i32) -> LogicalPosition {
    let origin = canvas.absolute_position();
    let size = canvas.size();
    let canvas_width = ui.get_canvas_width().max(1) as f32;
    let canvas_height = ui.get_canvas_height().max(1) as f32;
    let fit_scale = (size.width / canvas_width).min(size.height / canvas_height);
    let view_scale = if ui.get_canvas_zoom() == 0 {
        fit_scale
    } else {
        ui.get_canvas_zoom() as f32 / 100.0
    };
    let view_x =
        (size.width - canvas_width * view_scale) / 2.0 + ui.get_canvas_pan_x() as f32 * view_scale;
    let view_y = (size.height - canvas_height * view_scale) / 2.0
        + ui.get_canvas_pan_y() as f32 * view_scale;
    LogicalPosition::new(
        origin.x + view_x + x as f32 * view_scale,
        origin.y + view_y + y as f32 * view_scale,
    )
}
