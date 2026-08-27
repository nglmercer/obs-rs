use super::*;

/// Verifies the Sources-dock F2 path opens the existing rename modal for the
/// selected stable target and commits through the existing Rust command.
pub(super) fn exercise_source_dock_rename_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let source_id = "dock-keyboard-rename-source";
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            source: SourceSpec::new(
                source_id,
                "color_source",
                "Dock keyboard rename source",
                source_settings("color_source").expect("source defaults"),
            )
            .expect("dock keyboard rename source"),
        }))
        .expect("add dock keyboard rename source");
    refresh_ui(ui, state, surface);
    let original_platform_macos = ui.get_platform_macos();
    ui.set_platform_macos(false);
    focus_source_dock(ui);
    ui.invoke_select_source(source_id.into());
    refresh_ui(ui, state, surface);

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::F2.into(),
    });
    assert_eq!(ui.get_active_modal(), 12, "F2 opens source rename");
    assert_eq!(
        ui.get_source_name_draft(),
        "Dock keyboard rename source",
        "F2 loads the selected source name"
    );
    ui.set_active_modal(0);

    // OBS switches the dock rename key to Return on macOS. The same focus
    // boundary must reject F2 there, then accept Return without changing the
    // target-resolution path.
    ui.set_platform_macos(true);
    focus_source_dock(ui);
    ui.invoke_select_source(source_id.into());
    refresh_ui(ui, state, surface);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::F2.into(),
    });
    assert_eq!(ui.get_active_modal(), 0, "macOS does not bind F2 to rename");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    assert_eq!(
        ui.get_active_modal(),
        12,
        "Return opens source rename on macOS"
    );
    assert_eq!(
        ui.get_source_name_draft(),
        "Dock keyboard rename source",
        "Return loads the selected source name"
    );

    ui.set_source_name_draft("Renamed from Sources dock".into());
    ui.invoke_apply_source_name();
    assert_eq!(
        source_name(state, source_id).as_deref(),
        Some("Renamed from Sources dock"),
        "the platform rename key commits through the existing command"
    );
    ui.set_active_modal(0);

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: source_id.to_owned(),
        }))
        .expect("remove dock keyboard rename fixture source");
    refresh_ui(ui, state, surface);
    ui.set_platform_macos(original_platform_macos);
}

fn focus_source_dock(ui: &MainWindow) {
    let row = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .find(|row| row.size().height > 30.0)
        .expect("visible source row for rename focus");
    row.query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|area| area.size().width > 150.0 && area.size().height > 30.0)
        .find_first()
        .expect("source row rename focus target")
        .mock_single_click(PointerEventButton::Left);
}

fn source_name(state: &Rc<RefCell<DesktopState>>, id: &str) -> Option<String> {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source(id))
        .map(|source| source.name().to_owned())
}
