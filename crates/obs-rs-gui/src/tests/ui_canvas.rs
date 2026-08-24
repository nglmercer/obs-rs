use super::*;

/// Exercises the editable canvas through the testing backend's real pointer
/// event path. The starter background is hidden so the center of the canvas is
/// an empty drag origin; two bounded color sources then prove replacement and
/// Ctrl-additive drag-box selection. Middle-drag and Space+drag also exercise
/// transient pan before the fixture restores the scene.
pub(super) fn exercise_canvas_pointer_fixture(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let source_ids = ["canvas-pointer-left", "canvas-pointer-right"];
    for id in source_ids {
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

    let initial_pan = (ui.get_canvas_pan_x(), ui.get_canvas_pan_y());
    let canvas = canvas_surface(ui);
    let center = canvas.absolute_position();
    let center = LogicalPosition::new(
        center.x + canvas.size().width / 2.0,
        center.y + canvas.size().height / 2.0,
    );
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

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            items: source_ids.iter().map(ToString::to_string).collect(),
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
