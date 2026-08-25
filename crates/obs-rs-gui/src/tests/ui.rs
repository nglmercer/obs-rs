use super::*;
use super::{ui_layout, ui_navigation, ui_output, ui_sources};

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one deterministic GUI fixture exercises the persisted shell and dock surfaces"
)]
fn ui_layout_can_render_a_reference_snapshot() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            ..Default::default()
        },
    )))
    .expect("software testing backend should initialize");
    let ui = MainWindow::new().expect("GUI should instantiate in the testing backend");
    ui.set_project_path("obs-rs-project.json".into());
    ui.set_diagnostics_path("obs-rs-diagnostics.obsrdg".into());
    ui.set_recording_path("obs-rs-recording.y4m".into());
    ui.set_streaming_address("127.0.0.1:9000".into());
    let project = initial_project().expect("initial project");
    let surface = Rc::new(RefCell::new(
        PreviewSurface::new(&project, 0).expect("preview surface"),
    ));
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    let shortcut = Shortcut::parse("Ctrl+Z")
        .expect("shortcut syntax")
        .expect("shortcut binding");
    let cut_shortcut = Shortcut::parse("Ctrl+T")
        .expect("cut shortcut syntax")
        .expect("cut shortcut binding");
    let previous_shortcut = Shortcut::parse("F6")
        .expect("previous-scene shortcut syntax")
        .expect("previous-scene shortcut binding");
    let next_shortcut = Shortcut::parse("F7")
        .expect("next-scene shortcut syntax")
        .expect("next-scene shortcut binding");
    state
        .borrow_mut()
        .replace_shortcuts(&[
            (shortcut, UiAction::Undo),
            (cut_shortcut, UiAction::CutTransition),
            (previous_shortcut, UiAction::PreviousPreviewScene),
            (next_shortcut, UiAction::NextPreviewScene),
        ])
        .expect("shortcut table");
    crate::callbacks::install_shortcut_callbacks(&ui, &state);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+z".into()), 6);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+t".into()), 15);
    assert_eq!(ui.invoke_trigger_shortcut("f6".into()), 16);
    assert_eq!(ui.invoke_trigger_shortcut("f7".into()), 17);
    assert_eq!(ui.invoke_trigger_shortcut("Ctrl+X".into()), 0);
    let persisted_tree = DockNode::Split {
        axis: crate::dock_tree::DockAxis::Vertical,
        ratio_milli: 600,
        first: Box::new(DockNode::Tabs {
            docks: vec![1, 0],
            active: 1,
        }),
        second: Box::new(DockNode::Split {
            axis: crate::dock_tree::DockAxis::Horizontal,
            ratio_milli: 400,
            first: Box::new(DockNode::Dock(2)),
            second: Box::new(DockNode::Tabs {
                docks: vec![3, 4, 5],
                active: 0,
            }),
        }),
    };
    let docks = crate::install_dock_callbacks_with_layout(&ui, &state, Some(&persisted_tree), &[]);
    assert!(ui_layout::read_dock_panes(&ui)
        .iter()
        .any(|pane| pane.tab_count == 2));
    assert_eq!(ui_layout::read_dock_splitters(&ui).len(), 2);
    let default = AppSettings::default();
    let default_tree =
        DockNode::from_legacy(&default.layout.panel_order, &default.layout.panel_weights)
            .expect("default dock tree");
    docks.replace_tree(&default_tree, &ui);
    let canvas = install_canvas_callbacks(&ui, &state, &surface);
    ui.invoke_canvas_zoom_changed(100);
    assert_eq!(canvas.canvas_state().zoom().ui_value(), 100);
    ui.invoke_canvas_zoom_step(1);
    assert_eq!(canvas.canvas_state().zoom().ui_value(), 200);
    ui.invoke_canvas_zoom_changed(0);
    assert_eq!(canvas.canvas_state().zoom().ui_value(), 0);
    ui.invoke_canvas_pan_dragged(24, -12);
    assert_eq!(canvas.canvas_state().pan(), (24, -12));
    assert_eq!(ui.get_canvas_pan_x(), 24);
    assert_eq!(ui.get_canvas_pan_y(), -12);
    let before_nudge = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("background"))
        .expect("initial selected item")
        .transform();
    ui.invoke_canvas_nudged(3, -2);
    let after_nudge = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("background"))
        .expect("nudged selected item")
        .transform();
    assert_eq!(after_nudge.translate_x(), before_nudge.translate_x() + 3);
    assert_eq!(after_nudge.translate_y(), before_nudge.translate_y() - 2);
    refresh_ui(&ui, &state, &surface);
    ui.set_status_message("stats status".into());
    ui.set_capture_capabilities("stats capture capabilities".into());
    ui.set_preview_metrics("stats preview metrics".into());
    ui.set_output_metrics("stats output metrics".into());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&ui, "StatsDock").count(),
        1,
        "the built-in Stats dock is rendered from the shared diagnostics properties"
    );
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "preview").any(|row| row.size().height > 30.0)
    );
    ui.set_view_mode(2);
    ui.set_multiview_status("Output: recording idle · stream idle".into());
    ui.set_multiview_metrics("frames=12 · dropped=1 · audio blocks=24 · queued=0 B".into());
    ui.set_multiview_audio_db(-12.0);
    ui.set_show_safe_areas(true);
    ui.show().expect("testing window should show");
    let multiview_snapshot = ui
        .window()
        .take_snapshot()
        .expect("multiview overlay should render");
    assert!(multiview_snapshot.width() > 0 && multiview_snapshot.height() > 0);
    ui.set_view_mode(1);
    ui_navigation::exercise_navbar_popup(&ui);
    ui.set_pending_discard(5);
    let discard_snapshot = ui
        .window()
        .take_snapshot()
        .expect("discard dialog should render with its three actions");
    assert!(discard_snapshot.width() > 0 && discard_snapshot.height() > 0);
    ui.set_pending_discard(0);
    let snapshot = ui
        .window()
        .take_snapshot()
        .expect("testing backend should render a snapshot");
    let format = VideoFormat::new(
        snapshot.width(),
        snapshot.height(),
        FrameRate::new(60, 1).expect("snapshot frame rate"),
    )
    .expect("snapshot dimensions");
    let frame = VideoFrame::new(format, Timestamp::ZERO, snapshot.as_bytes().to_vec())
        .expect("snapshot RGBA data");
    let path = std::env::temp_dir().join("obs-rs-gui-reference-snapshot.png");
    std::fs::write(&path, encode_png(&frame).expect("encode snapshot")).expect("write snapshot");
    assert!(path.metadata().expect("snapshot metadata").len() > 0);
    std::fs::remove_file(path).expect("remove snapshot");

    ui_canvas::exercise_canvas_pointer_fixture(&ui, &state, &surface);
    ui_scene_drag_drop::exercise_scene_pointer_drag_and_drop(&ui, &state, &surface);
    ui.hide().expect("testing window should hide");

    // The settings window is a second top-level window with its own globals, so
    // it is exercised here rather than in its own test: only one test may own
    // the platform backend.
    ui_layout::exercise_layout_restore(&ui);
    ui_layout::exercise_dock_layout(&ui, &docks);
    ui_layout::render_every_settings_category();
    ui_layout::exercise_settings_commit(&ui, &state, &surface, &canvas);
    ui_sources::render_source_properties_window();
    ui_sources::render_source_filters_window(&ui, &state, &surface);
    ui_sources::exercise_source_transform_window(&ui, &state, &surface);
    ui_sources::exercise_source_keyboard_delete(&ui, &state, &surface);
    ui_sources::exercise_multi_source_keyboard_delete(&ui, &state, &surface);
    ui_sources::exercise_source_mouse_selection(&ui, &state, &surface);
    ui_sources::exercise_source_pointer_drag_and_drop(&ui, &state, &surface);
    ui_layout::render_monitor_window();
    ui_sources::exercise_add_source_window(&ui, &state, &surface);
    ui_sources::exercise_capture_device_properties_window(&ui, &state, &surface);
    ui_layout::exercise_monitor_selection(&ui, &state, &surface);
    ui_output::exercise_recording_controls(&ui, &state, &surface);
    ui_navigation::exercise_menu_actions(&ui, &state, &surface, &docks);
    ui_navigation::exercise_group_source_callbacks(&ui, &state, &surface);
    ui_navigation::exercise_context_menus(&ui, &state, &surface);
}
