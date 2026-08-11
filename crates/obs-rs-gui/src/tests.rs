use super::i18n::catalog;
use super::refresh::transition_label_for_locale;
use super::settings::AppSettings;
use super::{
    capture_devices, initial_project, refresh_ui, source_settings, I18n, MainWindow, OutputRuntime,
    PreviewRenderer, SettingsWindow, SourcePropertiesWindow,
};
use obs_rs_media::{FrameRate, FrameTransition, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{encode_png, MemoryMuxer, PacketKind};
use obs_rs_project::{ProjectCommand, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

#[test]
fn gui_project_has_control_room_scenes() {
    let project = initial_project().expect("initial GUI project should validate");
    let profile = project
        .profiles()
        .next()
        .expect("GUI project has a profile");
    let scenes = profile
        .scenes()
        .map(|scene| scene.id().to_string())
        .collect::<Vec<_>>();
    assert_eq!(scenes, ["intermission", "preview", "program"]);
}

#[test]
fn transition_labels_are_user_facing() {
    assert_eq!(
        transition_label_for_locale(UiLocale::English, FrameTransition::Cut),
        "Cut"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::English,
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
        ),
        "Fade 500/1000"
    );
}

#[test]
fn gui_catalog_switches_complete_copy_between_supported_locales() {
    let english = catalog(UiLocale::English);
    let spanish = catalog(UiLocale::Spanish);
    assert_eq!(english.scenes_title, "Scenes");
    assert_eq!(spanish.scenes_title, "Escenas");
    assert_eq!(english.add_source, "Add source");
    assert_eq!(spanish.add_source, "Añadir fuente");
    assert_ne!(english.shortcuts, spanish.shortcuts);
}

#[test]
fn preview_renderer_uses_the_project_scene_sources() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let frame = renderer
        .render("preview")
        .expect("preview scene should render")
        .expect("preview scene should produce a frame");
    assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
}

#[test]
fn preview_renderer_composes_scene_transitions() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let frame = renderer
        .render_transition(
            "preview",
            "program",
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
        )
        .expect("transition should render")
        .expect("transition should produce a frame");
    assert_eq!(frame.pixel(0, 0), Some([0x18, 0x28, 0x38, 0xff]));
}

#[test]
fn preview_renderer_advances_animated_capture_sources() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let first = renderer
        .render("intermission")
        .expect("first pattern frame should render")
        .expect("pattern scene should produce a frame");
    let second = renderer
        .render("intermission")
        .expect("second pattern frame should render")
        .expect("pattern scene should produce a frame");
    assert_ne!(first.pixels(), second.pixels());
}

#[test]
fn preview_renderer_reuses_static_scene_composition() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    renderer
        .render("preview")
        .expect("static scene should render")
        .expect("static scene should produce a frame");
    let first = renderer.runtime.compositor_metrics();
    renderer
        .render("preview")
        .expect("cached static scene should render")
        .expect("cached static scene should produce a frame");
    let second = renderer.runtime.compositor_metrics();
    assert_eq!(second.render_calls(), first.render_calls());
    assert_eq!(second.source_requests(), first.source_requests());
}

#[test]
fn capture_source_defaults_have_a_real_selectable_device_id() {
    for kind in ["screen_capture", "window_capture", "camera_capture"] {
        let settings = source_settings(kind).expect("capture defaults");
        let device_id = settings.get("device_id").expect("device id");
        assert!(!device_id.is_empty(), "{kind} must have a device id");
        assert!(
            capture_devices(kind).iter().any(|(id, _)| id == device_id),
            "{kind} default must be in its device catalog"
        );
    }
}

#[test]
fn app_settings_round_trip_the_selected_audio_input() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-settings-{token}.txt"));
    let settings = AppSettings {
        audio_input_id: "pipewire-node-42".to_owned(),
        ..AppSettings::default()
    };
    settings.save(&path).expect("settings should save");
    assert_eq!(
        AppSettings::load(&path).audio_input_id,
        settings.audio_input_id
    );
    std::fs::remove_file(path).expect("remove settings fixture");
}

#[test]
fn app_settings_round_trip_the_window_layout() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-layout-{token}.txt"));
    let mut settings = AppSettings::default();
    settings.layout.panel_order = vec![4, 3, 2, 1, 0];
    settings.layout.show_mixer = false;
    settings.layout.view_mode = 0;
    settings.layout.dock_height = 320;
    settings.layout.panel_weights = vec![1.5, 0.8, 2.0, 1.0, 1.2];
    settings.layout.floating_panels = vec![2, 3];
    settings.restore_project = false;
    settings.save_project_on_exit = false;

    settings.save(&path).expect("settings should save");
    let reloaded = AppSettings::load(&path);

    assert_eq!(reloaded, settings);
    std::fs::remove_file(path).expect("remove settings fixture");
}

#[test]
fn a_layout_that_lost_a_dock_falls_back_to_the_default_order() {
    let mut config = obs_rs_config::Config::new();
    config
        .set("layout_panel_order", "1,0,2,3")
        .expect("panel order key");
    config
        .set("layout_dock_height", "9999")
        .expect("dock height key");
    let document = config.serialize();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-layout-invalid-{token}.txt"));
    std::fs::write(&path, document).expect("write settings fixture");

    let settings = AppSettings::load(&path);

    let defaults = AppSettings::default();
    assert_eq!(settings.layout.panel_order, defaults.layout.panel_order);
    assert_eq!(settings.layout.dock_height, defaults.layout.dock_height);
    std::fs::remove_file(path).expect("remove settings fixture");
}

#[test]
fn output_runtime_switches_the_selected_audio_input_without_rebuilding_video() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    assert_eq!(output.audio_input_id(), None);
    output
        .set_audio_input_id(Some("missing-pipewire-input"))
        .expect("switch should fall back safely");
    assert_eq!(output.audio_input_id(), Some("missing-pipewire-input"));
    output
        .set_audio_input_id(None)
        .expect("automatic input should be accepted");
    assert_eq!(output.audio_input_id(), None);
}

#[test]
fn preview_renderer_rebuilds_after_project_edit() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("new-scene", "New scene").expect("scene"),
        })
        .expect("add scene");

    // A different revision is what tells the renderer to rebuild.
    assert!(renderer
        .sync_project(&project, 1)
        .expect("renderer should rebuild from the edited project"));
    // The same revision must not trigger another rebuild.
    assert!(!renderer
        .sync_project(&project, 1)
        .expect("unchanged revision is a no-op"));
    assert!(renderer
        .render("new-scene")
        .expect("empty scene should be renderable")
        .is_none());
}

#[test]
fn preview_renderer_honors_hidden_scene_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    project
        .apply(ProjectCommand::SetSourceVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            source: "background".to_owned(),
            visible: false,
        })
        .expect("hide source");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    assert!(renderer
        .render("preview")
        .expect("hidden scene should render")
        .is_none());
}

#[test]
fn output_runtime_finalizes_an_atomic_av_recording() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-gui-output-{token}.obsr"));
    let mut output = OutputRuntime::new(format);
    output
        .start_recording(final_path.to_str().expect("UTF-8 temp path"))
        .expect("recording should open");
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [20, 30, 40, 255]);
    output.push_frame(&frame);
    let bytes = output
        .finish_recording()
        .expect("recording should finalize");
    assert!(bytes > 0);
    let persisted = std::fs::read(&final_path).expect("recording should be persisted");
    assert_eq!(persisted.len(), bytes);
    let packets = MemoryMuxer::decode(&persisted).expect("packet recording should decode");
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Video));
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Audio));
    std::fs::remove_file(final_path).expect("remove output fixture");
}

#[test]
fn ui_layout_can_render_a_reference_snapshot() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            ..Default::default()
        },
    )))
    .expect("software testing backend should initialize");
    let ui = MainWindow::new().expect("GUI should instantiate in the testing backend");
    ui.set_project_path("obs-rs-project.txt".into());
    ui.set_diagnostics_path("obs-rs-diagnostics.obsrdg".into());
    ui.set_recording_path("obs-rs-recording.y4m".into());
    ui.set_streaming_address("127.0.0.1:9000".into());
    let project = initial_project().expect("initial project");
    let renderer = Rc::new(RefCell::new(
        PreviewRenderer::new(&project, 0).expect("preview renderer"),
    ));
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    refresh_ui(&ui, &state, &renderer);
    ui.show().expect("testing window should show");
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
    ui.hide().expect("testing window should hide");

    // The settings window is a second top-level window with its own globals, so
    // it is exercised here rather than in its own test: only one test may own
    // the platform backend.
    exercise_layout_restore(&ui);
    exercise_dock_layout(&ui, &state);
    render_every_settings_category();
    render_source_properties_window();
    render_monitor_window();
    exercise_add_source_window(&ui, &state, &renderer);
    exercise_capture_device_properties_window(&ui, &state, &renderer);
    exercise_monitor_selection(&ui, &state, &renderer);
    exercise_recording_controls(&ui, &state, &renderer);
}

/// Pushes a stored layout into the real window and reads it back, which is the
/// round trip a restart performs.
fn exercise_layout_restore(ui: &MainWindow) {
    let mut stored = AppSettings::default();
    stored.layout.panel_order = vec![2, 4, 0, 1, 3];
    stored.layout.show_transitions = false;
    stored.layout.view_mode = 0;
    stored.layout.dock_height = 300;

    stored.apply_layout(ui);

    assert_eq!(ui.get_view_mode(), 0);
    assert!(!ui.get_show_transitions());
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
fn exercise_dock_layout(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let controller = crate::install_dock_callbacks(ui, state);

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

    // Detaching opens a window for the dock and takes it out of the row.
    assert!(!controller.is_floating(2));
    ui.invoke_float_panel(2);
    assert!(controller.is_floating(2), "the mixer detached");
    assert!(read_floating(ui)[2], "the row must know the dock left it");

    // Detaching again returns it to the row.
    ui.invoke_float_panel(2);
    assert!(!controller.is_floating(2), "the mixer re-docked");
    assert!(!read_floating(ui)[2]);
}

fn read_order(ui: &MainWindow) -> Vec<i32> {
    let model = ui.get_panel_order();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

fn read_weights(ui: &MainWindow) -> Vec<f32> {
    let model = ui.get_panel_weights();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

fn read_floating(ui: &MainWindow) -> Vec<bool> {
    let model = ui.get_panel_floating();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

/// Renders the display picker in both locales with a two-monitor layout, so a
/// broken map binding or a missing catalog field fails the suite.
fn render_monitor_window() {
    let window = crate::MonitorWindow::new().expect("monitor window should instantiate");
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

/// Drives the display picker end to end: opening it for an X11 screen source,
/// accepting the whole-desktop choice, and confirming the project records it.
fn exercise_monitor_selection(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let settings = source_settings("x11_screen_capture").expect("x11 defaults");
    let source = SourceSpec::new("gui-screen", "x11_screen_capture", "GUI screen", settings)
        .expect("screen source");
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
    refresh_ui(ui, state, renderer);
    assert!(
        ui.get_selected_source_is_screen(),
        "an X11 screen source must offer the display picker"
    );

    let controller =
        crate::install_monitor_window(ui, state, renderer).expect("monitor controller");
    ui.invoke_open_monitor_window();
    let window = crate::callbacks::monitor::MonitorController::window(&controller);
    // The whole-desktop choice is the one available on every host, including a
    // CI machine with no display server.
    window.set_capture_whole_desktop(true);
    window.invoke_accept_monitor();

    let state = state.borrow();
    let source = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene.as_str()))
        .and_then(|scene| scene.source("gui-screen"))
        .expect("screen source persisted");
    assert_eq!(
        source.settings().get("monitor"),
        Some(""),
        "the display choice must reach the project command"
    );
}

/// Renders each settings category so a page that fails to lay out — an empty
/// model, a binding loop, a missing catalog field — fails the suite.
fn render_every_settings_category() {
    let window = SettingsWindow::new().expect("settings window should instantiate");
    crate::callbacks::populate_settings_models(&window);
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
    window.hide().expect("settings window should hide");
}

/// Renders the properties window in both locales so a missing catalog field or
/// broken layout fails the suite.
fn render_source_properties_window() {
    let window = SourcePropertiesWindow::new().expect("properties window should instantiate");
    window.set_source_name("Background".into());
    window.set_source_kind("color_source".into());
    window.set_source_settings("color=#405070FF\nheight=360\nwidth=640\n".into());
    window.set_property_rows(ModelRc::new(VecModel::from(crate::properties::rows(
        "color_source",
        "color=#405070FF\nheight=360\nwidth=640\n",
        UiLocale::English,
    ))));
    window.set_source_transform("1000,1000,0,0,0,0,255".into());
    window.show().expect("properties window should show");
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        let snapshot = window
            .window()
            .take_snapshot()
            .expect("properties window should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
    }
    window.hide().expect("properties window should hide");
}

/// Drives the Add Source window the way a user would: pick a kind, create a
/// source, then copy an existing one into the current scene.
fn exercise_add_source_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let controller = crate::install_add_source_window(ui, state, renderer)
        .expect("add source window should instantiate");
    let window = crate::callbacks::add_source_window(&controller);
    window.show().expect("add source window should show");

    // Every registered kind must produce a renderable page.
    for kind in renderer.borrow().runtime.source_kinds() {
        crate::callbacks::populate_add_source_window(&controller, state, renderer, kind.as_str());
        assert!(
            window
                .window()
                .take_snapshot()
                .expect("kind page should render")
                .width()
                > 0
        );
        assert!(window.get_can_create(), "a real kind offers creation");
    }

    let scene = state
        .borrow()
        .preview_scene()
        .expect("a preview scene is selected")
        .to_owned();
    crate::callbacks::populate_add_source_window(&controller, state, renderer, "color_source");

    let before = scene_source_count(state, &scene);
    window.invoke_create_source();
    assert_eq!(
        scene_source_count(state, &scene),
        before + 1,
        "create adds exactly one source to the current scene"
    );

    // A source the current scene already shows is never offered: adding it
    // again would only produce a second identical row. The fixture's scenes all
    // hold an identically named background, so a distinct source is planted in
    // another scene to have something that *can* be added.
    let donor = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| {
            profile
                .scenes()
                .map(|value| value.id().as_str().to_owned())
                .find(|value| *value != scene)
        })
        .expect("the project has a second scene");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: donor.clone(),
            source: SourceSpec::new(
                "overlay",
                "color_source",
                "Overlay",
                source_settings("color_source").expect("colour defaults"),
            )
            .expect("overlay source"),
        }))
        .expect("plant a source in another scene");

    crate::callbacks::populate_add_source_window(&controller, state, renderer, "color_source");
    let candidate = window
        .get_candidates()
        .iter()
        .find(|row| row.name == "Overlay")
        .expect("the planted source is offered");
    assert_ne!(
        candidate.scene.as_str(),
        scene.as_str(),
        "candidates never come from the target scene"
    );
    window.invoke_toggle_candidate(candidate.id.clone());
    assert_eq!(window.get_selected_count(), 1);
    let before = scene_source_count(state, &scene);
    window.invoke_add_selected();
    assert_eq!(
        scene_source_count(state, &scene),
        before + 1,
        "adding one existing source copies exactly one spec"
    );
    assert_eq!(
        window.get_selected_count(),
        0,
        "the selection is cleared once it has been added"
    );

    // Once it is in the scene, the same source is no longer a candidate.
    crate::callbacks::populate_add_source_window(&controller, state, renderer, "color_source");
    assert!(
        !window
            .get_candidates()
            .iter()
            .any(|row| row.id == candidate.id),
        "a source that is already in the scene must not be offered again"
    );

    // "Recently added" lists existing sources only, so it offers no creation.
    crate::callbacks::populate_add_source_window(&controller, state, renderer, "@recent");
    assert!(!window.get_can_create());

    window.hide().expect("add source window should hide");
}

/// Verifies the complete screen/camera source-properties path: selecting a
/// camera source, changing its device in the `ComboBox` callback, and accepting
/// the draft writes the selected stable device ID back into the project.
fn exercise_capture_device_properties_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let mut settings = source_settings("camera_capture").expect("camera defaults");
    settings
        .set("device_id", "camera-0")
        .expect("portable camera selection");
    let source = SourceSpec::new("gui-camera", "camera_capture", "GUI camera", settings)
        .expect("camera source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: scene.clone(),
            source,
        }))
        .expect("add camera source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "gui-camera".to_owned(),
        })
        .expect("select camera source");
    refresh_ui(ui, state, renderer);

    let controller = crate::install_source_properties_window(ui, state, renderer)
        .expect("properties controller");
    ui.invoke_open_source_properties_window();
    let window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&controller);
    // The camera kind renders a device drop-down as its first typed row.
    let device_row = window
        .get_property_rows()
        .row_data(0)
        .expect("the camera form has a device row");
    assert_eq!(device_row.key, "device_id");
    assert!(device_row.choices.row_count() >= 1);
    window.invoke_edit_property(device_row.key.clone(), "0".into());
    assert!(window.get_source_settings().contains("device_id="));
    window.invoke_accept_properties();

    let state = state.borrow();
    let source = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene.as_str()))
        .and_then(|scene| scene.source("gui-camera"))
        .expect("camera source persisted");
    assert_eq!(
        source.settings().get("device_id"),
        Some("camera-0"),
        "ComboBox selection must reach the project command"
    );
}

/// Drives the actual `MainWindow` recording callback instead of calling the
/// output wrapper directly, then verifies the resulting file contains both
/// media kinds.
fn exercise_recording_controls(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-callback-{token}.obsr"));
    ui.set_recording_path(path.to_string_lossy().into_owned().into());
    let output = Rc::new(RefCell::new(OutputRuntime::new(renderer.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, renderer, &output);

    ui.invoke_toggle_recording();
    assert!(
        state.borrow().recording(),
        "Record button must start the state"
    );
    let frame = renderer
        .borrow_mut()
        .render("program")
        .expect("program frame")
        .expect("program scene frame");
    crate::callbacks::push_program_frame(ui, Some(frame), &output);
    ui.invoke_toggle_recording();
    assert!(
        !state.borrow().recording(),
        "Record button must stop the state"
    );

    let bytes = std::fs::read(&path).expect("GUI recording file");
    assert!(!bytes.is_empty());
    let packets = MemoryMuxer::decode(&bytes).expect("GUI recording container");
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Video));
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Audio));
    std::fs::remove_file(path).expect("remove GUI recording fixture");
}

fn scene_source_count(state: &Rc<RefCell<DesktopState>>, scene: &str) -> usize {
    let state = state.borrow();
    let session = state.project_session();
    let project = session.project();
    let count = project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().find(|value| value.id().as_str() == scene))
        .map_or(0, |scene| scene.sources().len());
    count
}
