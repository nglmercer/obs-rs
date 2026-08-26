use super::*;
use super::{ui_layout, ui_navigation, ui_output, ui_project_open, ui_slideshow, ui_sources};

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
    let studio_shortcut = Shortcut::parse("Ctrl+Shift+S")
        .expect("studio-mode shortcut syntax")
        .expect("studio-mode shortcut binding");
    let visibility_shortcut = Shortcut::parse("Ctrl+Shift+V")
        .expect("selected-source visibility shortcut syntax")
        .expect("selected-source visibility shortcut binding");
    let lock_shortcut = Shortcut::parse("Ctrl+Shift+L")
        .expect("selected-source lock shortcut syntax")
        .expect("selected-source lock shortcut binding");
    let projector_shortcut = Shortcut::parse("Ctrl+Shift+P")
        .expect("selected-source projector shortcut syntax")
        .expect("selected-source projector shortcut binding");
    let scene_projector_shortcut = Shortcut::parse("Ctrl+Shift+R")
        .expect("preview-scene projector shortcut syntax")
        .expect("preview-scene projector shortcut binding");
    let push_to_talk_shortcut = Shortcut::parse("T")
        .expect("push-to-talk shortcut syntax")
        .expect("push-to-talk shortcut binding");
    let push_to_mute_shortcut = Shortcut::parse("U")
        .expect("push-to-mute shortcut syntax")
        .expect("push-to-mute shortcut binding");
    state
        .borrow_mut()
        .replace_shortcuts(&[
            (shortcut, UiAction::Undo),
            (cut_shortcut, UiAction::CutTransition),
            (previous_shortcut, UiAction::PreviousPreviewScene),
            (next_shortcut, UiAction::NextPreviewScene),
            (studio_shortcut, UiAction::ToggleStudioMode),
            (
                visibility_shortcut,
                UiAction::ToggleSelectedSourceVisibility,
            ),
            (lock_shortcut, UiAction::ToggleSelectedSourceLock),
            (projector_shortcut, UiAction::ToggleSelectedSourceProjector),
            (
                scene_projector_shortcut,
                UiAction::TogglePreviewSceneProjector,
            ),
            (push_to_talk_shortcut, UiAction::PushToTalkMicrophone),
            (push_to_mute_shortcut, UiAction::PushToMuteMicrophone),
        ])
        .expect("shortcut table");
    crate::callbacks::install_shortcut_callbacks(&ui, &state);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+z".into()), 6);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+t".into()), 15);
    assert_eq!(ui.invoke_trigger_shortcut("f6".into()), 16);
    assert_eq!(ui.invoke_trigger_shortcut("f7".into()), 17);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+shift+s".into()), 18);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+shift+v".into()), 19);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+shift+l".into()), 20);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+shift+p".into()), 21);
    assert_eq!(ui.invoke_trigger_shortcut("ctrl+shift+r".into()), 22);
    assert_eq!(ui.invoke_trigger_shortcut("t".into()), 23);
    assert_eq!(ui.invoke_trigger_shortcut("u".into()), 24);
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
    let push_events = Rc::new(RefCell::new(Vec::<(String, bool)>::new()));
    let push_events_callback = Rc::clone(&push_events);
    ui.on_set_mixer_muted(move |id, muted| {
        push_events_callback
            .borrow_mut()
            .push((id.to_string(), muted));
    });
    ui.set_view_mode(1);
    ElementHandle::find_by_element_id(&ui, "CanvasEditor::surface")
        .find(|canvas| canvas.size().width > 100.0 && canvas.size().height > 100.0)
        .expect("editable canvas focus target")
        .mock_single_click(PointerEventButton::Left);
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "T".into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "T".into() });
    assert_eq!(
        push_events.borrow().as_slice(),
        [("mic".to_owned(), false), ("mic".to_owned(), true)]
    );
    push_events.borrow_mut().clear();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "T".into() });
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(false));
    assert_eq!(
        push_events.borrow().as_slice(),
        [("mic".to_owned(), false), ("mic".to_owned(), true)],
        "window deactivation must release a held push-to-talk action"
    );
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "T".into() });
    assert_eq!(
        push_events.borrow().as_slice(),
        [("mic".to_owned(), false), ("mic".to_owned(), true)],
        "a delayed key release must not restore a second time"
    );
    push_events.borrow_mut().clear();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "U".into() });
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(false));
    assert_eq!(
        push_events.borrow().as_slice(),
        [("mic".to_owned(), true), ("mic".to_owned(), false)],
        "window deactivation must release a held push-to-mute action"
    );
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    ui.invoke_toggle_studio_mode();
    assert_eq!(
        ui.get_view_mode(),
        0,
        "Studio Mode callback enters Studio view"
    );
    ui.invoke_toggle_studio_mode();
    assert_eq!(
        ui.get_view_mode(),
        1,
        "Studio Mode callback returns to canvas view"
    );
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
    ui_source_keyboard::exercise_source_clipboard_keyboard(&ui, &state, &surface);
    ui_sources::exercise_source_mouse_selection(&ui, &state, &surface);
    ui_sources::exercise_source_pointer_drag_and_drop(&ui, &state, &surface);
    ui_layout::render_monitor_window();
    ui_sources::exercise_add_source_window(&ui, &state, &surface);
    ui_sources::exercise_capture_device_properties_window(&ui, &state, &surface);
    ui_layout::exercise_monitor_selection(&ui, &state, &surface);
    ui_output::exercise_recording_controls(&ui, &state, &surface);
    ui_navigation::exercise_menu_actions(&ui, &state, &surface, &docks);
    ui_navigation::exercise_group_source_callbacks(&ui, &state, &surface);
    ui_project_open::exercise_project_open_dialog(&ui);
    ui_project_open::exercise_project_recovery_dialog(&ui, &state);
    ui_navigation::exercise_context_menus(&ui, &state, &surface);
    ui_sources::exercise_image_source_file_picker(&ui, &state, &surface);
    ui_slideshow::exercise_slideshow_directory_picker(&ui, &state, &surface);
}
