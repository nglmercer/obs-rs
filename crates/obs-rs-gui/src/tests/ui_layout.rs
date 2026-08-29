use super::ui_settings_accessibility::{
    exercise_appearance_settings_control_accessibility,
    exercise_audio_settings_control_accessibility, exercise_general_settings_control_accessibility,
    exercise_settings_category_accessibility, exercise_settings_density_accessibility,
    exercise_video_fps_accessibility, exercise_video_resolution_accessibility,
    exercise_video_settings_control_accessibility,
};
use super::*;
use i_slint_backend_testing::AccessibleRole;

pub(super) fn exercise_layout_restore(ui: &MainWindow) {
    let mut stored = AppSettings::default();
    stored.layout.panel_order = vec![2, 4, 0, 1, 3, 5];
    stored.layout.dock_tree =
        DockNode::from_legacy(&stored.layout.panel_order, &stored.layout.panel_weights)
            .expect("test layout should have a valid dock tree");
    stored.layout.show_transitions = false;
    stored.layout.show_stats = false;
    stored.layout.view_mode = 0;
    stored.layout.dock_height = 300;

    stored.apply_layout(ui);

    assert_eq!(ui.get_view_mode(), 0);
    assert!(!ui.get_show_transitions());
    assert!(!ui.get_show_stats());
    assert!(ui.get_show_mixer());
    let order = ui.get_panel_order();
    assert_eq!(
        (0..order.row_count())
            .filter_map(|index| order.row_data(index))
            .collect::<Vec<_>>(),
        stored.layout.panel_order
    );

    let mut captured = AppSettings::default();
    captured.capture_layout(ui);
    assert_eq!(captured.layout, stored.layout);

    // Leave the window in its default layout for the snapshot tests that follow.
    AppSettings::default().apply_layout(ui);
}

/// Drives dock reordering, splitter resizing, and detaching a dock into its
/// own window through the real callbacks.
pub(super) fn exercise_dock_layout(
    ui: &MainWindow,
    controller: &Rc<crate::callbacks::docks::DockController>,
) {
    assert!(!ui.get_meters_paused());
    ui.invoke_toggle_meters_paused();
    assert!(ui.get_meters_paused(), "the mixer monitor pauses");
    ui.invoke_toggle_meters_paused();
    assert!(!ui.get_meters_paused(), "the mixer monitor resumes");

    exercise_dock_header_pointer_drag(ui, controller);
    exercise_dock_splitter_pointer_drag(ui, controller);

    // Reordering moves the dragged dock one place and leaves the rest alone.
    let before = read_order(ui);
    ui.invoke_move_panel(before[0], 1);
    let after = read_order(ui);
    assert_eq!(after[0], before[1]);
    assert_eq!(after[1], before[0]);
    assert_eq!(after[2..], before[2..]);

    // A splitter drag trades width between its two neighbours only.
    let before = read_weights(ui);
    ui.invoke_resize_panel(2, 160);
    let after = read_weights(ui);
    assert!(after[0] > before[0] || after[1] > before[1], "a dock grew");
    assert!(
        (after.iter().sum::<f32>() - before.iter().sum::<f32>()).abs() < 1e-4,
        "the row's total width must be preserved"
    );

    // A header drag resolves a pane target and paints a directional insertion
    // hint before the drop mutates the tree.
    ui.invoke_dock_drag_start(0, 0.99, 0.5);
    ui.invoke_dock_drag_moved(0, 0.99, 0.5);
    assert!(ui.get_dock_dragging());
    assert_eq!(ui.get_dock_drop_target(), 5);
    assert!(ui.get_dock_drop_zone() > 0);
    ui.invoke_dock_drag_end(0, 0.99, 0.5);
    assert!(!ui.get_dock_dragging());
    assert_eq!(read_order(ui).last().copied(), Some(0));

    let before_splitter = read_dock_splitters(ui)[0].boundary;
    ui.invoke_resize_dock_splitter(0, 100.0);
    assert!(read_dock_splitters(ui)[0].boundary > before_splitter);

    // Detaching opens a window for the dock and takes it out of the row.
    assert!(!controller.is_floating(2));
    ui.invoke_float_panel(2);
    assert!(controller.is_floating(2), "the mixer detached");
    assert!(read_floating(ui)[2], "the row must know the dock left it");
    let floating_geometry = controller.capture_floating_geometry();
    let mixer_geometry = floating_geometry
        .iter()
        .find(|geometry| geometry.panel == 2)
        .expect("the detached window geometry is captured");
    assert!(mixer_geometry.width >= 240);
    assert!(mixer_geometry.height >= 160);

    let floating_window = controller
        .floating_window(2)
        .and_then(|window| window.upgrade())
        .expect("the detached mixer window is reachable");
    floating_window
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
    assert!(
        !controller.is_floating(2),
        "native close re-docks the mixer"
    );
    assert!(!read_floating(ui)[2]);

    // Detaching again returns it to the row.
    ui.invoke_float_panel(2);
    assert!(controller.is_floating(2), "the mixer detaches again");
    ui.invoke_float_panel(2);
    assert!(!controller.is_floating(2), "the mixer re-docked");
    assert!(!read_floating(ui)[2]);

    // The tree callbacks drive the same pane projection used by the visible
    // workspace: tabbing keeps one region, selecting a tab changes its active
    // leaf, and a split creates a second bounded region.
    ui.invoke_tab_dock_with(4, 3);
    let panes = read_dock_panes(ui);
    assert!(panes.iter().any(|pane| pane.tab_count == 2));
    let controls_tab = ElementHandle::find_by_accessible_label(ui, "Controls")
        .find(|tab| {
            tab.accessible_role() == Some(AccessibleRole::Tab)
                && tab.size().width > 0.0
                && tab.size().height > 0.0
        })
        .expect("the dock tab exposes its accessible label and role");
    assert_eq!(controls_tab.accessible_enabled(), Some(true));
    controls_tab.invoke_accessible_default_action();
    ui.invoke_select_dock_tab(4);
    assert!(read_dock_panes(ui)
        .iter()
        .any(|pane| pane.panel_kind == 4 && pane.active));
    ui.invoke_split_dock_with(2, 4, 1, 500);
    assert_eq!(read_dock_panes(ui).len(), 6);
}

/// Drives a dock header through the testing backend's actual pointer path.
///
/// The direct callback checks below remain useful for the legacy move/resize
/// projections, but this verifies that a visible header starts a drag, updates
/// the directional target while pressed, and commits the resulting tree on
/// release.
fn exercise_dock_header_pointer_drag(
    ui: &MainWindow,
    controller: &Rc<crate::callbacks::docks::DockController>,
) {
    let headers = ElementHandle::find_by_element_type_name(ui, "DockHeader")
        .filter(|header| header.size().width > 100.0 && header.size().height >= 20.0)
        .collect::<Vec<_>>();
    assert_eq!(
        headers.len(),
        6,
        "the default layout exposes six dock headers"
    );
    let before = read_order(ui);
    assert_eq!(
        before.len(),
        headers.len(),
        "dock headers mirror the pane order"
    );

    let source = headers[0].absolute_position();
    let source_size = headers[0].size();
    let target = headers
        .last()
        .expect("the default layout has a final dock header");
    let target_position = target.absolute_position();
    let target_size = target.size();
    let start = LogicalPosition::new(
        source.x + source_size.width / 2.0,
        source.y + source_size.height / 2.0,
    );
    // The right quarter is a directional insertion zone, not a tab target.
    let drop = LogicalPosition::new(
        target_position.x + target_size.width * 0.9,
        target_position.y + target_size.height / 2.0,
    );

    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    assert!(ui.get_dock_dragging(), "the header starts a dock drag");
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: drop });
    assert_eq!(
        ui.get_dock_drop_target(),
        *before.last().expect("the final pane has a dock kind"),
        "the pointer resolves the final pane as the drop target"
    );
    assert_eq!(
        ui.get_dock_drop_zone(),
        2,
        "the pointer resolves the right zone"
    );
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: drop,
        button: PointerEventButton::Left,
    });

    assert!(
        !ui.get_dock_dragging(),
        "the header clears its drag state on release"
    );
    assert_eq!(
        read_order(ui).last().copied(),
        before.first().copied(),
        "the real header drop moves the source dock after the target"
    );

    let default = AppSettings::default();
    let default_tree =
        DockNode::from_legacy(&default.layout.panel_order, &default.layout.panel_weights)
            .expect("the default dock tree is valid");
    controller.replace_tree(&default_tree, ui);
}

/// Drives one visible vertical splitter through the testing backend. The
/// splitter owns only a bounded tree ratio, so the fixture checks that the
/// pointer gesture changes a boundary without changing the pane count.
fn exercise_dock_splitter_pointer_drag(
    ui: &MainWindow,
    controller: &Rc<crate::callbacks::docks::DockController>,
) {
    let splitters = ElementHandle::find_by_element_type_name(ui, "VerticalSplitter")
        .filter(|splitter| splitter.size().width >= 4.0 && splitter.size().height > 100.0)
        .collect::<Vec<_>>();
    assert!(
        !splitters.is_empty(),
        "the default layout exposes a vertical splitter"
    );
    let before = read_dock_splitters(ui);
    assert!(!before.is_empty(), "the Rust projection exposes a splitter");

    let position = splitters[0].absolute_position();
    let size = splitters[0].size();
    let start = LogicalPosition::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    let end = LogicalPosition::new(start.x + 40.0, start.y);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: end });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });

    let after = read_dock_splitters(ui);
    assert_eq!(
        after.len(),
        before.len(),
        "splitter drag keeps the tree shape"
    );
    assert!(
        after
            .iter()
            .zip(before.iter())
            .any(|(after, before)| (after.boundary - before.boundary).abs() > 0.0001),
        "the real splitter drag changes a bounded tree boundary"
    );

    let default = AppSettings::default();
    let default_tree =
        DockNode::from_legacy(&default.layout.panel_order, &default.layout.panel_weights)
            .expect("the default dock tree is valid");
    controller.replace_tree(&default_tree, ui);
}

pub(super) fn read_order(ui: &MainWindow) -> Vec<i32> {
    let model = ui.get_panel_order();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

pub(super) fn read_weights(ui: &MainWindow) -> Vec<f32> {
    let model = ui.get_panel_weights();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

pub(super) fn read_floating(ui: &MainWindow) -> Vec<bool> {
    let model = ui.get_panel_floating();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

pub(super) fn read_dock_panes(ui: &MainWindow) -> Vec<crate::DockPane> {
    let model = ui.get_dock_panes();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

pub(super) fn read_dock_splitters(ui: &MainWindow) -> Vec<crate::DockSplitter> {
    let model = ui.get_dock_splitters();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

/// Renders the display picker in both locales with a two-monitor layout, so a
/// broken map binding or a missing catalog field fails the suite.
pub(super) fn render_monitor_window() {
    let window = crate::MonitorWindow::new().expect("monitor window should instantiate");
    let selected = Rc::new(RefCell::new(String::new()));
    let selected_callback = Rc::clone(&selected);
    window.on_select_monitor(move |id| {
        *selected_callback.borrow_mut() = id.to_string();
    });
    window.set_source_name("x11_screen_capture".into());
    window.set_monitor_rows(ModelRc::new(VecModel::from(vec![
        crate::MonitorRow {
            id: "DP-1".into(),
            name: "DP-1".into(),
            geometry: "1920x1080 at 0,0".into(),
            primary: true,
            selected: true,
            normalized_x: 0.0,
            normalized_y: 0.0,
            normalized_width: 0.6,
            normalized_height: 1.0,
        },
        crate::MonitorRow {
            id: "HDMI-1".into(),
            name: "HDMI-1".into(),
            geometry: "1280x1024 at 1920,0".into(),
            primary: false,
            selected: false,
            normalized_x: 0.6,
            normalized_y: 0.0,
            normalized_width: 0.4,
            normalized_height: 0.94,
        },
    ])));
    window.set_selected_id("DP-1".into());
    window.show().expect("monitor window should show");
    window
        .global::<I18n>()
        .set_text(crate::i18n::catalog(UiLocale::English));
    exercise_monitor_list_accessibility(&window, &selected);
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        let snapshot = window
            .window()
            .take_snapshot()
            .expect("monitor window should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
    }
    window.hide().expect("monitor window should hide");
}

/// Verifies that the monitor list exposes stable display identities while
/// retaining selection ownership in the existing standalone-window callback.
fn exercise_monitor_list_accessibility(
    window: &crate::MonitorWindow,
    selected: &Rc<RefCell<String>>,
) {
    let primary = ElementHandle::find_by_accessible_label(window, "DP-1")
        .find(|row| {
            row.accessible_role() == Some(AccessibleRole::ListItem)
                && row.size().width > 100.0
                && row.size().height > 30.0
        })
        .expect("the primary monitor row is accessible");
    assert_eq!(
        primary.accessible_description().as_deref(),
        Some("DP-1 · 1920x1080 at 0,0 · primary")
    );
    assert_eq!(primary.accessible_enabled(), Some(true));
    assert_eq!(primary.accessible_item_selectable(), Some(true));
    assert_eq!(primary.accessible_item_selected(), Some(true));
    assert_eq!(primary.accessible_item_index(), Some(0));
    assert_eq!(primary.accessible_item_count(), Some(2));

    let secondary = ElementHandle::find_by_accessible_label(window, "HDMI-1")
        .find(|row| {
            row.accessible_role() == Some(AccessibleRole::ListItem)
                && row.size().width > 100.0
                && row.size().height > 30.0
        })
        .expect("the secondary monitor row is accessible");
    assert_eq!(
        secondary.accessible_description().as_deref(),
        Some("HDMI-1 · 1280x1024 at 1920,0")
    );
    assert_eq!(secondary.accessible_enabled(), Some(true));
    assert_eq!(secondary.accessible_item_selectable(), Some(true));
    assert_eq!(secondary.accessible_item_selected(), Some(false));
    assert_eq!(secondary.accessible_item_index(), Some(1));
    assert_eq!(secondary.accessible_item_count(), Some(2));

    secondary.invoke_accessible_default_action();
    assert_eq!(selected.borrow().as_str(), "HDMI-1");
}

/// Verifies that the first-run setup window owns its Escape boundary without
/// stealing modified Escape from controls that may use it themselves.
pub(super) fn render_setup_window() {
    let window = crate::SetupWindow::new().expect("setup window should instantiate");
    let closed = Rc::new(RefCell::new(false));
    let close_state = Rc::clone(&closed);
    let close_window = window.as_weak();
    window.on_close_requested(move || {
        *close_state.borrow_mut() = true;
        if let Some(window) = close_window.upgrade() {
            let _ = window.hide();
        }
    });
    crate::callbacks::setup::install_native_close(&window);
    window.show().expect("setup window should show");
    window.invoke_focus_keyboard_boundary();
    window
        .window()
        .take_snapshot()
        .expect("setup window should render before Escape dispatch");

    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    assert!(
        !*closed.borrow(),
        "modified Escape stays inside the setup UI"
    );
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });

    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    assert!(*closed.borrow(), "plain Escape closes the setup wizard");

    *closed.borrow_mut() = false;
    window.show().expect("setup window should reopen");
    window
        .window()
        .take_snapshot()
        .expect("setup window should render before native close dispatch");
    window.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(
        *closed.borrow(),
        "native close reaches the setup close callback"
    );
    assert!(
        !window.window().is_visible(),
        "native close hides the setup wizard"
    );
}

/// Drives the display picker end to end: opening it for a platform screen
/// source, accepting the automatic/whole-desktop choice, and confirming the
/// project records it.
fn exercise_monitor_keyboard_boundary(ui: &MainWindow, window: &crate::MonitorWindow) {
    let expected_whole_desktop = cfg!(target_os = "windows");
    assert_eq!(
        window.get_capture_whole_desktop(),
        expected_whole_desktop,
        "the picker starts from the persisted platform display selection"
    );
    // Escape must discard the in-window selection. The whole-desktop choice is
    // available on every host, including a CI machine with no display server.
    window.set_capture_whole_desktop(true);
    window
        .window()
        .take_snapshot()
        .expect("monitor window should render before Escape dispatch");
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });

    ui.invoke_open_monitor_window();
    assert_eq!(
        window.get_capture_whole_desktop(),
        expected_whole_desktop,
        "Escape discards the monitor picker draft"
    );
    window.set_capture_whole_desktop(true);
    window
        .window()
        .take_snapshot()
        .expect("monitor window should render before native close dispatch");
    window.window().dispatch_event(WindowEvent::CloseRequested);

    ui.invoke_open_monitor_window();
    assert_eq!(
        window.get_capture_whole_desktop(),
        expected_whole_desktop,
        "native window close discards the monitor picker draft"
    );
    window.set_capture_whole_desktop(true);
    window
        .window()
        .take_snapshot()
        .expect("monitor window should render before Enter dispatch");
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
}

#[cfg(target_os = "windows")]
fn assert_windows_screen_property_rows(window: &crate::SourcePropertiesWindow) {
    let property_keys = (0..window.get_property_rows().row_count())
        .filter_map(|index| window.get_property_rows().row_data(index))
        .map(|row| row.key.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        property_keys,
        ["capture_cursor", "capture_border", "width", "height"],
        "Windows display selection must be exposed by the dedicated picker, not a duplicate combo box"
    );
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) fn exercise_monitor_selection(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let kind = if cfg!(target_os = "windows") {
        "screen_capture"
    } else {
        "x11_screen_capture"
    };
    let settings = source_settings(kind).expect("screen defaults");
    #[cfg(target_os = "linux")]
    let settings = {
        let mut settings = settings;
        settings
            .set("monitor", "DP-1")
            .expect("monitor draft fixture");
        settings
    };
    let source =
        SourceSpec::new("gui-screen", kind, "GUI screen", settings).expect("screen source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: scene.clone(),
            source,
        }))
        .expect("add screen source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "gui-screen".to_owned(),
        })
        .expect("select screen source");
    refresh_ui(ui, state, surface);
    assert!(
        ui.get_selected_source_is_screen(),
        "a screen source must offer the display picker"
    );

    let controller = crate::install_monitor_window(ui, state, surface).expect("monitor controller");
    ui.invoke_open_monitor_window();
    let window = crate::callbacks::monitor::MonitorController::window(&controller);
    exercise_monitor_keyboard_boundary(ui, window);

    let state_ref = state.borrow();
    let source = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("gui-screen"))
        .expect("screen source persisted");
    assert_eq!(
        source.settings().get("monitor"),
        Some(""),
        "Enter must apply the display choice through the project command"
    );
    drop(state_ref);

    // A nested screen item must keep its stable path all the way through the
    // properties dialog and into the monitor picker, even when another root
    // source becomes selected while the dialog is being opened.
    let mut group = SceneItemSpec::for_group("screen-group", "Screen group").expect("screen group");
    group
        .group_mut()
        .expect("screen group target")
        .add_item(SceneItemSpec::for_source("gui-screen").expect("nested screen item"))
        .expect("nested screen attach");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene,
            item: group,
        }))
        .expect("add nested screen item");
    refresh_ui(ui, state, surface);
    ui.invoke_select_source("background".into());
    let properties =
        crate::install_source_properties_window_with_monitor(ui, state, surface, Some(&controller))
            .expect("target-aware properties controller");
    ui.invoke_open_source_properties_for("screen-group/gui-screen".into());
    let properties_window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&properties);
    assert!(
        properties_window.get_monitor_visible(),
        "nested screen properties keep the target-aware monitor picker"
    );
    #[cfg(target_os = "windows")]
    assert_windows_screen_property_rows(properties_window);
    properties_window.invoke_open_monitor_window();
    let monitor_window = crate::callbacks::monitor::MonitorController::window(&controller);
    assert_eq!(
        monitor_window.get_source_name(),
        "GUI screen",
        "the monitor picker follows the nested source rather than the selected root"
    );
    monitor_window.set_capture_whole_desktop(true);
    monitor_window.invoke_accept_monitor();
}

/// Drives the real settings controller through Apply, Cancel, and OK.
///
/// The draft semantics are the whole point of the window, so they are checked
/// against the controller rather than against a hand-built stand-in: Apply
/// persists and clears the dirty flag, Cancel discards every draft including
/// the live-previewed appearance, OK persists and closes, and a field that
/// fails validation commits nothing at all.
pub(super) fn exercise_settings_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    canvas: &Rc<crate::callbacks::CanvasController>,
) {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-settings-window-{token}.toml"));
    let format = surface.borrow().format;
    let output = Rc::new(RefCell::new(OutputRuntime::new(format)));
    let docks = crate::install_dock_callbacks(ui, state);
    let projectors = crate::install_menu_callbacks(ui, state, surface, &docks);
    let controller = crate::install_settings_window(
        ui,
        state,
        surface,
        &output,
        AppSettings::default(),
        path.clone(),
        &crate::PeerWindows {
            add_source: crate::install_add_source_window(ui, state, surface)
                .expect("add source controller"),
            properties: crate::install_source_properties_window(ui, state, surface)
                .expect("properties controller"),
            filters: crate::install_source_filters_window(ui, state, surface)
                .expect("filters controller"),
            transform: crate::install_source_transform_window(ui, state, surface)
                .expect("transform controller"),
            monitor: crate::install_monitor_window(ui, state, surface).expect("monitor controller"),
            docks,
            projectors,
            canvas: Rc::clone(canvas),
        },
    )
    .expect("settings controller should install");
    let window = controller.window();

    // Opening the window fills the draft from the committed document, so a
    // freshly opened window has nothing to apply.
    ui.invoke_open_settings_window();
    assert!(!window.get_dirty(), "a freshly loaded draft is not dirty");

    exercise_settings_category_accessibility(window);
    exercise_general_settings_control_accessibility(window);
    exercise_appearance_settings_control_accessibility(window);
    exercise_settings_density_accessibility(window);
    exercise_audio_settings_control_accessibility(window);
    exercise_video_settings_control_accessibility(window);
    exercise_video_resolution_accessibility(window);
    exercise_video_fps_accessibility(window);
    exercise_mixer_settings_navigation(ui, window);
    exercise_stream_server_selection(window);

    exercise_settings_apply_and_validation(window, &controller, &path);
    exercise_settings_keyboard_boundary(ui, window, &controller, &path);
    std::fs::remove_file(&path).expect("remove settings fixture");
}

fn exercise_settings_apply_and_validation(
    window: &SettingsWindow,
    controller: &crate::callbacks::settings::SettingsController,
    path: &std::path::Path,
) {
    // Apply: every draft is persisted and the button goes quiet again.
    window.set_density_index(3);
    window.set_font_size(16.0);
    window.set_style_index(2);
    window.invoke_edit_output_resolution("1280x720".into());
    window.set_scale_filter_index(2);
    window.set_recording_quality_index(2);
    window.set_recording_filename_without_spaces(true);
    window.set_snap_distance(24);
    window.set_show_safe_areas(true);
    window.set_dirty(true);
    window.invoke_apply_settings();

    assert!(
        !window.get_dirty(),
        "Apply clears the unapplied-changes flag"
    );
    let committed = controller.committed();
    assert_eq!(
        committed.density,
        crate::settings_model::UiDensity::Comfortable
    );
    assert_eq!(committed.font_size, 16);
    assert_eq!(committed.style, crate::settings_model::UiStyle::Contrast);
    assert_eq!(committed.video.output_width, 1_280);
    assert_eq!(committed.video.scale_filter, ScaleFilter::Lanczos);
    assert!(committed.recording_filename_without_spaces);
    assert_eq!(committed.canvas_snap_distance, 24);
    assert!(committed.show_safe_areas);
    assert_eq!(AppSettings::load(path), committed, "Apply writes the file");

    // A field that cannot be parsed stops the commit entirely: nothing else on
    // the page may reach the document behind an invalid value.
    window.invoke_edit_base_resolution("not-a-resolution".into());
    window.set_font_size(9.0);
    window.set_dirty(true);
    window.invoke_apply_settings();

    assert!(
        window.get_dirty(),
        "a rejected commit leaves the changes unapplied"
    );
    assert_eq!(window.get_category(), 5, "the invalid page is brought up");
    assert!(!window.get_base_resolution_valid(), "the row stays marked");
    assert_eq!(
        controller.committed().font_size,
        16,
        "an unrelated field must not be committed behind an invalid one"
    );
}

fn exercise_settings_keyboard_boundary(
    ui: &MainWindow,
    window: &SettingsWindow,
    controller: &crate::callbacks::settings::SettingsController,
    path: &std::path::Path,
) {
    // Cancel discards every draft, including the appearance that was already
    // previewed onto the live windows.
    window
        .window()
        .take_snapshot()
        .expect("settings window should render before Escape dispatch");
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    assert!(!window.get_dirty());
    assert_eq!(controller.committed().font_size, 16);

    // Native window-manager close has the same discard semantics as Escape.
    ui.invoke_open_settings_window();
    window.set_font_size(18.0);
    window.set_dirty(true);
    window
        .window()
        .take_snapshot()
        .expect("settings window should render before native close dispatch");
    window.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(!window.get_dirty(), "native close restores the draft");
    ui.invoke_open_settings_window();
    assert!(
        (window.get_font_size() - 16.0).abs() < f32::EPSILON,
        "native close discards edits"
    );

    // OK persists and closes.
    window.set_recording_quality_index(0);
    window.set_dirty(true);
    window
        .window()
        .take_snapshot()
        .expect("settings window should render before Ctrl+Enter dispatch");
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert!(!window.get_dirty(), "OK applies before it closes");
    assert_eq!(
        controller.committed().recording_quality,
        crate::settings_model::RecordingQuality::SameAsStream
    );
    assert_eq!(
        AppSettings::load(path).recording_quality,
        crate::settings_model::RecordingQuality::SameAsStream
    );
}

/// Verifies that the contextual Mixer action targets the Audio page.
fn exercise_mixer_settings_navigation(ui: &MainWindow, window: &SettingsWindow) {
    window.set_category(8);
    ui.invoke_open_audio_settings_window();
    assert_eq!(
        window.get_category(),
        4,
        "mixer settings open the Audio page"
    );
}

fn exercise_stream_server_selection(window: &SettingsWindow) {
    let twitch_index = RTMP_SERVICE_PRESETS
        .iter()
        .position(|preset| preset.id() == "twitch")
        .expect("Twitch service preset");
    let twitch_index = i32::try_from(twitch_index).expect("service index");
    window.set_rtmp_service_index(twitch_index);
    window.invoke_select_stream_service(twitch_index);
    assert_eq!(window.get_rtmp_server_names().row_count(), 46);
    window.invoke_select_stream_server(1);
    assert_eq!(window.get_rtmp_server(), "live-sel.twitch.tv/app");
}

/// Renders each settings category so a page that fails to lay out — an empty
/// model, a binding loop, a missing catalog field — fails the suite.
pub(super) fn render_every_settings_category() {
    let window = SettingsWindow::new().expect("settings window should instantiate");
    crate::callbacks::populate_settings_models(&window);
    assert_eq!(window.get_rtmp_service_names().row_count(), 82);
    window.show().expect("settings window should show");
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        for category in 0..9 {
            window.set_category(category);
            let snapshot = window
                .window()
                .take_snapshot()
                .expect("settings category should render");
            assert!(
                snapshot.width() > 0 && snapshot.height() > 0,
                "settings category {category} rendered an empty surface"
            );
        }
    }

    // Density, font size, and the wider Spanish labels are the three things
    // that can break the shared geometry, so the three redesigned pages are
    // rendered against all of them rather than only at the default.
    window
        .global::<I18n>()
        .set_text(crate::i18n::catalog(UiLocale::Spanish));
    for density in crate::settings_model::UiDensity::ALL {
        for font_size in [
            *crate::settings_model::FONT_SIZE_RANGE.start(),
            *crate::settings_model::FONT_SIZE_RANGE.end(),
        ] {
            window
                .global::<crate::Metrics>()
                .set_ui(crate::settings_model::metrics(density, font_size));
            for category in [1, 3, 5] {
                window.set_category(category);
                let snapshot = window
                    .window()
                    .take_snapshot()
                    .expect("settings category should render at every density");
                assert!(
                    snapshot.width() > 0 && snapshot.height() > 0,
                    "category {category} rendered empty at {density:?}/{font_size}"
                );
            }
        }
    }
    window.hide().expect("settings window should hide");
}
