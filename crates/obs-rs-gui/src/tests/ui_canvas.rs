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

pub(super) fn exercise_canvas_pointer_fixture(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    prepare_canvas_pointer_scene(state, surface, ui);
    exercise_transform_handle(ui, state, surface);
    exercise_rotation_handle(ui, state, surface);
    exercise_drag_selection(ui, state);
    exercise_overlapping_selection(ui, state, surface);
    exercise_pan_and_zoom(ui);
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
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: end });
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
