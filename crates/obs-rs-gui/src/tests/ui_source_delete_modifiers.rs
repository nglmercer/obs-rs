use super::*;

/// Verifies that source removal is an unmodified action on both keyboard
/// focus surfaces. Shift remains available for other canvas/list workflows.
pub(super) fn exercise_source_delete_modifier_boundary(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let cases = [
        ("canvas-delete-modifier", Key::Delete, true),
        ("dock-backspace-modifier", Key::Backspace, false),
    ];
    for (id, key, canvas) in cases {
        add_source(state, id);
        refresh_ui(ui, state, surface);
        if canvas {
            focus_canvas(ui);
        } else {
            focus_source_row(ui, id);
        }
        ui.invoke_select_source(id.into());
        refresh_ui(ui, state, surface);

        dispatch_shifted(ui, key);
        assert!(
            source_exists(state, id),
            "Shift+Delete must not remove a source"
        );

        dispatch_plain(ui, key);
        assert!(
            !source_exists(state, id),
            "the unmodified source-removal key must remove the source"
        );
    }
}

fn add_source(state: &Rc<RefCell<DesktopState>>, id: &str) {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            source: SourceSpec::new(
                id,
                "color_source",
                id,
                source_settings("color_source").expect("source defaults"),
            )
            .expect("modifier source"),
        }))
        .expect("add modifier source");
}

fn source_exists(state: &Rc<RefCell<DesktopState>>, id: &str) -> bool {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| scene.item(id).is_some())
}

fn focus_canvas(ui: &MainWindow) {
    ElementHandle::find_by_element_id(ui, "CanvasEditor::surface")
        .find(|canvas| canvas.size().width > 100.0 && canvas.size().height > 100.0)
        .expect("editable canvas focus target")
        .mock_single_click(PointerEventButton::Left);
}

fn focus_source_row(ui: &MainWindow, id: &str) {
    ElementHandle::find_by_accessible_label(ui, id)
        .find(|row| row.size().height > 30.0)
        .expect("visible source row for modifier boundary")
        .query_descendants()
        .match_inherits("TouchArea")
        .find_first()
        .expect("source row modifier focus target")
        .mock_single_click(PointerEventButton::Left);
}

fn dispatch_shifted(ui: &MainWindow, key: Key) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    dispatch_plain(ui, key);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
}

fn dispatch_plain(ui: &MainWindow, key: Key) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}
