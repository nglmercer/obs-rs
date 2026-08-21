use super::dock_tree::DockNode;
use super::i18n::catalog;
use super::output::stream_protocol_label;
use super::preview::PreviewRenderer;
use super::refresh::{peak_db, transition_label_for_locale};
use super::settings::AppSettings;
use super::{
    capture_devices, initial_project, install_canvas_callbacks, refresh_ui, source_settings, I18n,
    MainWindow, OutputRuntime, PreviewSurface, SettingsWindow, SourcePropertiesWindow,
};
use i_slint_backend_testing::ElementHandle;
use obs_rs_media::{
    FrameRate, FrameTransform, FrameTransition, ScaleFilter, Timestamp, VideoFormat, VideoFrame,
};
use obs_rs_output::{encode_png, MemoryMuxer, PacketKind};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{ProjectCommand, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

#[test]
fn stream_protocol_status_uses_redacted_scheme_labels() {
    let cases = [
        ("srt://media.example:9000?passphrase=top-secret", "SRT"),
        ("rtmp://media.example/live/top-secret", "RTMP"),
        ("rtmps://media.example/live/top-secret", "RTMPS"),
        ("ws://127.0.0.1:9000/private", "OBSR-WebSocket"),
        ("127.0.0.1:9000", "OBSR-TCP"),
    ];
    for (endpoint, expected) in cases {
        let label = stream_protocol_label(endpoint);
        assert_eq!(label, expected);
        assert!(!label.contains("top-secret"));
        assert!(!label.contains("media.example"));
    }
}

#[test]
fn output_runtime_exposes_backend_protocol_capabilities() {
    let format = initial_project()
        .expect("project")
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let output = OutputRuntime::new(format);
    assert!(output.capabilities().protocols().iter().any(|capability| {
        capability.protocol() == obs_rs_engine::ProductionProtocol::Reference
            && capability.available()
    }));
}

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
fn mixer_peak_meter_uses_a_bounded_dbfs_scale() {
    assert!((peak_db(0) - -60.0).abs() < f32::EPSILON);
    assert!((peak_db(1_000) - 0.0).abs() < f32::EPSILON);
    assert!((peak_db(500) - -6.020_600_3).abs() < 0.001);
    assert!(
        (peak_db(1) - -60.0).abs() < f32::EPSILON,
        "the visible meter has a -60 dB floor"
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
fn the_desktop_channel_names_its_monitor_or_admits_it_is_silent() {
    let format = VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);

    // Whether a playback monitor exists depends on the machine running the
    // suite, so the invariant under test is that the two answers stay
    // distinguishable: a named device, or nothing at all. What must never
    // happen is a channel that claims a device it is not reading.
    match output.desktop_audio_name() {
        Some(name) => assert!(!name.trim().is_empty(), "a captured monitor has a name"),
        None => assert!(
            output
                .diagnostics_document()
                .contains("desktop_audio_active=false"),
            "a silent desktop channel says so in diagnostics"
        ),
    }
}

#[test]
fn a_selected_audio_input_survives_disappearing_from_the_graph() {
    let format = VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    // An ID no graph will ever contain stands in for a device that has been
    // unplugged since it was chosen.
    let missing = "pipewire-node-999999";

    let entries = output.audio_input_entries(missing);

    let selected = entries
        .iter()
        .find(|entry| entry.id == missing)
        .expect("a missing selection must still be offered, not silently dropped");
    assert!(
        !selected.available,
        "the entry has to say the device is not connected"
    );
    assert!(
        entries.iter().filter(|entry| entry.id == missing).count() == 1,
        "the placeholder must not duplicate a device discovery already found"
    );

    // The automatic route is always resolvable, so it is never reported missing.
    assert!(output.audio_input_entries("").is_empty() || output.audio_input_available());
}

#[test]
fn a_canvas_change_applies_immediately_while_the_output_is_idle() {
    let (state, output) = canvas_fixture();
    let wider = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");

    crate::callbacks::settings::apply_video_format(&state, &output, wider)
        .expect("an idle session applies the change at once");

    assert_eq!(profile_video_format(&state), wider);
    assert_eq!(output.borrow().video_format(), wider);
}

#[test]
fn a_canvas_change_staged_during_output_applies_at_the_next_idle_tick() {
    let (state, output) = canvas_fixture();
    let original = profile_video_format(&state);
    let wider = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");

    output.borrow_mut().stage_video_format(wider);
    assert!(output.borrow().has_staged_video_format());
    assert_eq!(
        profile_video_format(&state),
        original,
        "staging must not touch the project while the output is running"
    );

    let applied = crate::callbacks::settings::apply_staged_video_format(&state, &output)
        .expect("a staged change is pending")
        .expect("the idle boundary applies it");

    assert_eq!(applied, wider);
    assert_eq!(profile_video_format(&state), wider);
    assert!(
        !output.borrow().has_staged_video_format(),
        "a staged change is applied exactly once"
    );
    assert!(
        crate::callbacks::settings::apply_staged_video_format(&state, &output).is_none(),
        "an idle tick with nothing staged must do no work"
    );
}

#[test]
fn the_encoders_receive_the_scaled_output_resolution() {
    let canvas = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(canvas);

    output
        .set_output_scaling(64, 36, ScaleFilter::Bicubic)
        .expect("an idle session rescales at once");

    assert_eq!(output.encoded_output_format().width(), 64);
    assert_eq!(output.encoded_output_format().height(), 36);
    assert_eq!(
        output.encoded_output_format().frame_rate().numerator(),
        30,
        "scaling changes geometry, never pacing"
    );
    assert!(
        !output.accepts_raw_frames(),
        "packed frames cannot be resampled, so the RGBA path has to be used"
    );

    // A canvas frame must be resampled rather than rejected: a format drop here
    // would mean the recording silently contained nothing.
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-scaled-{token}.obsr"));
    output
        .start_recording(&path.to_string_lossy())
        .expect("recording should open");
    output.push_frame(&VideoFrame::solid(canvas, Timestamp::ZERO, [9, 9, 9, 255]));
    let bytes = output
        .finish_recording()
        .expect("recording should finalize");

    assert!(bytes > 0, "the scaled frame has to reach the muxer");
    assert!(
        !output.output_metrics().contains("format_drops=1"),
        "a canvas frame must not be dropped while the output is scaled"
    );
    std::fs::remove_file(path).expect("remove scaled fixture");
}

#[test]
fn an_output_scaling_change_staged_during_output_applies_at_the_next_idle_tick() {
    let (_state, output) = canvas_fixture();
    let original = output.borrow().encoded_output_format();

    output
        .borrow_mut()
        .stage_output_scaling(32, 18, ScaleFilter::Lanczos);
    assert_eq!(
        output.borrow().encoded_output_format(),
        original,
        "staging must not touch the encoders while the output is running"
    );

    let (width, height) = crate::callbacks::settings::apply_staged_output_scaling(&output)
        .expect("a staged change is pending")
        .expect("the idle boundary applies it");

    assert_eq!((width, height), (32, 18));
    assert_eq!(output.borrow().encoded_output_format().width(), 32);
    assert!(
        crate::callbacks::settings::apply_staged_output_scaling(&output).is_none(),
        "an idle tick with nothing staged must do no work"
    );
}

#[test]
fn a_canvas_change_the_engine_rejects_is_rolled_back() {
    let (state, output) = canvas_fixture();
    let original = profile_video_format(&state);
    let wider = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");
    // An open recording makes the engine refuse a rebuild, which is the
    // failure the rollback exists for.
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-rollback-{token}.obsr"));
    output
        .borrow_mut()
        .start_recording(&path.to_string_lossy())
        .expect("open a recording");

    let error = crate::callbacks::settings::apply_video_format(&state, &output, wider)
        .expect_err("the engine refuses to rebuild under an open output");

    assert!(error.contains("restored"), "the message says what happened");
    assert_eq!(
        profile_video_format(&state),
        original,
        "the project must not keep a canvas the engine never adopted"
    );
    assert_eq!(output.borrow().video_format(), original);
    output.borrow_mut().abort_recording();
}

#[test]
#[ignore = "requires a live Wayland or X11 compositor for MainWindow::new"]
fn a_failed_output_clears_the_claim_and_offers_guided_recovery() {
    let ui = MainWindow::new().expect("window");
    let (state, output) = canvas_fixture();
    // The desktop optimistically believes it is streaming; the engine never
    // connected. Reconciling is what turns that mismatch into an answer the
    // operator can act on rather than a control stuck in the on position.
    state
        .borrow_mut()
        .dispatch(obs_rs_ui::UiCommand::StartStreaming)
        .expect("claim streaming");
    ui.set_streaming(true);
    output
        .borrow_mut()
        .start_streaming("127.0.0.1:1")
        .expect("the non-blocking start request is accepted");

    // The worker publishes its snapshot after replying, so the failure can be
    // one tick behind the rejected connect. In the app that only delays the
    // dialog by a frame; here it has to be waited for.
    for _ in 0..100 {
        if output.borrow().lifecycles().1 == obs_rs_engine::OutputLifecycle::Failed {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    crate::callbacks::reconcile_output_lifecycle(&ui, &state, &output);

    assert!(!ui.get_streaming(), "the control stops claiming an output");
    assert!(!state.borrow().streaming());
    assert_eq!(
        ui.get_active_modal(),
        11,
        "a failure opens the recovery dialog rather than only logging a status line"
    );
    assert!(!ui.get_status_message().is_empty());
}

/// Builds a desktop state and an output runtime that agree on the canvas.
fn canvas_fixture() -> (Rc<RefCell<DesktopState>>, Rc<RefCell<OutputRuntime>>) {
    let project = initial_project().expect("initial project");
    let format = project
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    (state, Rc::new(RefCell::new(OutputRuntime::new(format))))
}

fn profile_video_format(state: &Rc<RefCell<DesktopState>>) -> VideoFormat {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .expect("profile")
        .video_format()
}

#[test]
fn app_settings_round_trip_the_selected_audio_input() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-settings-{token}.toml"));
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
    let path = std::env::temp_dir().join(format!("obs-rs-gui-layout-{token}.toml"));
    let mut settings = AppSettings::default();
    settings.layout.panel_order = vec![4, 3, 2, 1, 0];
    settings.layout.show_mixer = false;
    settings.layout.view_mode = 0;
    settings.layout.dock_height = 320;
    settings.layout.panel_weights = vec![1.5, 0.8, 2.0, 1.0, 1.2];
    settings.layout.dock_tree =
        DockNode::from_legacy(&settings.layout.panel_order, &settings.layout.panel_weights)
            .expect("test layout should have a valid dock tree");
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
    let path = std::env::temp_dir().join(format!("obs-rs-gui-layout-invalid-{token}.toml"));
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

    // A different revision is what tells the renderer to apply the change.
    assert!(renderer
        .sync_project(&project, 1)
        .expect("renderer should apply the edited project"));
    // The same revision must not trigger another sync.
    assert!(!renderer
        .sync_project(&project, 1)
        .expect("unchanged revision is a no-op"));
    assert!(renderer
        .render("new-scene")
        .expect("empty scene should be renderable")
        .is_none());
}

#[test]
fn a_transform_commits_to_the_item_it_started_on() {
    let mut project = initial_project().expect("initial GUI project should validate");
    // The preview scene needs a second item for a selection change to have
    // anywhere to go.
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: obs_rs_project::SceneItemSpec::new("overlay", "pattern").expect("item"),
        })
        .expect("add a second item");
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    let scene = state
        .borrow()
        .preview_scene()
        .expect("a preview scene is selected")
        .to_owned();
    let items = {
        let state = state.borrow();
        let session = state.project_session();
        session
            .project()
            .active_profile_spec()
            .expect("profile")
            .scene(scene.as_str())
            .expect("scene")
            .items()
            .iter()
            .map(|item| item.id().as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(items.len() > 1, "the fixture needs two items to confuse");

    let target = crate::source_target(&state.borrow(), &items[0]).expect("target");
    // The gesture started on the first item; the selection has since moved on,
    // which is what a dock click during a drag does.
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: items[1].clone(),
        })
        .expect("select the other item");

    assert_eq!(target.item, items[0]);
    assert_ne!(
        crate::selected_target(&state.borrow())
            .expect("selection")
            .item,
        target.item,
        "the pinned target must not follow the selection"
    );
}

#[test]
fn moving_a_source_does_not_recreate_the_scene_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let before = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };

    // A canvas drag is a stream of these. Not one of them may recreate a
    // source: for a camera or a screen cast, recreating is reopening.
    for step in 1..=25_u64 {
        project
            .apply(ProjectCommand::SetSceneItemTransform {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: "background".to_owned(),
                transform: FrameTransform::new(
                    500,
                    500,
                    i32::try_from(step).expect("step"),
                    0,
                    false,
                    false,
                    255,
                )
                .expect("transform"),
            })
            .expect("move source");
        assert!(renderer
            .sync_project(&project, step)
            .expect("renderer should apply the move"));
    }

    let after = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };
    assert_eq!(before, after, "a move must not rebuild the runtime sources");
}

#[test]
fn repeated_scene_item_references_share_the_runtime_source() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let source_count = renderer.runtime.source_count();
    let transform = FrameTransform::new(500, 500, 120, 40, false, false, 128).expect("transform");

    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: obs_rs_project::SceneItemSpec::new("background-copy", "background")
                .expect("reference item"),
        })
        .expect("add reference item");
    project
        .apply(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background-copy".to_owned(),
            transform,
        })
        .expect("transform reference item");
    renderer
        .sync_project(&project, 1)
        .expect("renderer should apply the duplicate reference");

    assert_eq!(renderer.runtime.source_count(), source_count);
    let scene_sources = renderer
        .runtime
        .scene_sources("preview")
        .expect("preview scene is live");
    assert_eq!(scene_sources.len(), 2);
    assert_eq!(scene_sources[0], scene_sources[1]);
    assert_eq!(
        renderer.runtime.scene_item_ids("preview"),
        Some(vec!["background".to_owned(), "background-copy".to_owned()])
    );
    assert_eq!(
        renderer.runtime.scene_item_transform("preview", 1),
        Some(transform)
    );

    let layers = renderer
        .runtime
        .render_scene_layers(
            "preview",
            &VideoRequest::new(Timestamp::ZERO, renderer.format),
        )
        .expect("duplicate scene should render");
    assert_eq!(layers.len(), 2);
    assert_eq!(
        renderer
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
fn nested_scene_references_render_without_reopening_shared_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let child_transform =
        FrameTransform::new(1_500, 800, 10, -4, false, false, 200).expect("child transform");
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    let mut child_item =
        obs_rs_project::SceneItemSpec::for_source("background").expect("child item");
    child_item.set_transform(child_transform);
    child.add_item(child_item).expect("child item attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("parent", "Parent").expect("parent scene"),
        })
        .expect("add parent scene");
    let parent_transform =
        FrameTransform::new(2_000, 1_500, 20, 30, false, false, 128).expect("parent transform");
    let mut nested =
        obs_rs_project::SceneItemSpec::for_scene("child-item", "child").expect("nested item");
    nested.set_transform(parent_transform);
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "parent".to_owned(),
            item: nested,
        })
        .expect("add nested item");

    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let source_count = renderer.runtime.source_count();
    assert_eq!(
        renderer
            .runtime
            .scene_sources("parent")
            .expect("parent scene is live")
            .len(),
        1
    );
    assert_eq!(renderer.runtime.source_count(), source_count);
    assert_eq!(
        renderer.runtime.scene_item_transform("parent", 0),
        Some(
            child_transform
                .compose_simple(parent_transform)
                .expect("compose")
        )
    );

    let layers = renderer
        .runtime
        .render_scene_layers(
            "parent",
            &VideoRequest::new(Timestamp::ZERO, renderer.format),
        )
        .expect("nested scene should render");
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].item_id(), "child-item/background");
    assert_eq!(
        renderer
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
fn group_items_render_without_reopening_shared_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut group = obs_rs_project::SceneItemSpec::for_group("group", "Group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(obs_rs_project::SceneItemSpec::for_source("background").expect("group child"))
        .expect("group child attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        })
        .expect("add group");

    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let source_count = renderer.runtime.source_count();
    assert_eq!(
        renderer
            .runtime
            .scene_sources("preview")
            .expect("preview scene")
            .len(),
        2
    );
    assert_eq!(renderer.runtime.source_count(), source_count);
    let layers = renderer
        .runtime
        .render_scene_layers(
            "preview",
            &VideoRequest::new(Timestamp::ZERO, renderer.format),
        )
        .expect("group should render");
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].item_id(), "background");
    assert_eq!(layers[1].item_id(), "group/background");
    assert_eq!(
        renderer
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
fn hiding_a_source_keeps_the_others_running() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let before = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };

    project
        .apply(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background".to_owned(),
            visible: false,
        })
        .expect("hide source");
    renderer
        .sync_project(&project, 1)
        .expect("renderer should apply the visibility change");

    let after = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };
    // Hiding detaches the item from the scene; the source definition and every
    // other device in the project stay exactly as they were.
    assert_eq!(before, after);
    assert!(renderer
        .render("preview")
        .expect("hidden scene should render")
        .is_none());
}

#[test]
fn preview_renderer_honors_hidden_scene_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    project
        .apply(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background".to_owned(),
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
    let persisted_tree = DockNode::Split {
        axis: super::dock_tree::DockAxis::Vertical,
        ratio_milli: 600,
        first: Box::new(DockNode::Tabs {
            docks: vec![1, 0],
            active: 1,
        }),
        second: Box::new(DockNode::Split {
            axis: super::dock_tree::DockAxis::Horizontal,
            ratio_milli: 400,
            first: Box::new(DockNode::Dock(2)),
            second: Box::new(DockNode::Tabs {
                docks: vec![3, 4],
                active: 0,
            }),
        }),
    };
    let docks = crate::install_dock_callbacks_with_layout(&ui, &state, Some(&persisted_tree), &[]);
    assert!(read_dock_panes(&ui).iter().any(|pane| pane.tab_count == 2));
    assert_eq!(read_dock_splitters(&ui).len(), 2);
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
    ui.show().expect("testing window should show");
    exercise_navbar_popup(&ui);
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
    exercise_dock_layout(&ui, &docks);
    render_every_settings_category();
    exercise_settings_commit(&ui, &state, &surface);
    render_source_properties_window();
    render_source_filters_window(&ui, &state, &surface);
    exercise_source_transform_window(&ui, &state, &surface);
    render_monitor_window();
    exercise_add_source_window(&ui, &state, &surface);
    exercise_capture_device_properties_window(&ui, &state, &surface);
    exercise_monitor_selection(&ui, &state, &surface);
    exercise_recording_controls(&ui, &state, &surface);
    exercise_menu_actions(&ui, &state, &surface, &docks);
    exercise_group_source_callbacks(&ui, &state, &surface);
    exercise_context_menus(&ui, &state, &surface);
}

#[allow(
    clippy::too_many_lines,
    reason = "one integration fixture exercises the complete nested source workflow"
)]
fn exercise_group_source_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);
    let mut group =
        obs_rs_project::SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(obs_rs_project::SceneItemSpec::for_source("background").expect("first child"))
        .expect("first child attach");
    group
        .group_mut()
        .expect("group target")
        .add_item(obs_rs_project::SceneItemSpec::for_source("pattern").expect("second child"))
        .expect("second child attach");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        }))
        .expect("add group to preview");
    refresh_ui(ui, state, surface);
    assert!(ui
        .get_source_rows()
        .iter()
        .any(|row| row.target == "overlay-group/background"));

    let transform = crate::install_source_transform_window(ui, state, surface)
        .expect("nested transform window should instantiate");
    ui.invoke_open_source_transform_for("overlay-group/background".into());
    let transform_window = crate::callbacks::source_transform::source_transform_window(&transform);
    assert_eq!(transform_window.get_source_name(), "Background");
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "opening a nested transform must not replace canvas selection"
    );
    transform_window.set_position_x(37);
    transform_window.set_position_y(-9);
    transform_window.set_item_opacity(190);
    transform_window.invoke_accept_transform();
    let nested_transform = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(obs_rs_project::SceneItemSpec::group)
        .and_then(|group| {
            group
                .items()
                .iter()
                .find(|item| item.id().as_str() == "background")
        })
        .map(obs_rs_project::SceneItemSpec::transform)
        .expect("nested transform should be committed to the child");
    assert_eq!(nested_transform.translate_x(), 37);
    assert_eq!(nested_transform.translate_y(), -9);
    assert_eq!(nested_transform.opacity(), 190);

    let properties = crate::install_source_properties_window(ui, state, surface)
        .expect("nested properties window should instantiate");
    ui.invoke_open_source_properties_for("overlay-group/background".into());
    let properties_window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&properties);
    assert_eq!(properties_window.get_source_name(), "Background");
    assert_eq!(properties_window.get_source_kind(), "color_source");
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "opening nested properties must not replace canvas selection"
    );
    properties_window.invoke_edit_property("width".into(), "800".into());
    properties_window.invoke_accept_properties();
    assert_eq!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.source("background"))
            .and_then(|source| source.settings().get("width")),
        Some("800")
    );

    let filters = crate::install_source_filters_window(ui, state, surface)
        .expect("nested filters window should instantiate");
    ui.invoke_open_source_filters_for("overlay-group/background".into());
    let filters_window = crate::callbacks::source_filters::source_filters_window(&filters);
    assert_eq!(filters_window.get_source_name(), "Background");
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "opening a nested filter target must not replace canvas selection"
    );
    filters_window.invoke_add_filter("opacity".into());
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("background"))
        .is_some_and(|source| {
            source
                .filters()
                .iter()
                .any(|filter| filter.kind().as_str() == "opacity")
        }));
    filters_window.invoke_close_window();

    ui.invoke_toggle_source_visibility("overlay-group/background".into());
    ui.invoke_move_source_to("overlay-group/background".into(), 1);
    ui.invoke_flip_source("overlay-group/background".into(), true);
    ui.invoke_duplicate_source("overlay-group/background".into());
    ui.invoke_toggle_source_locked("overlay-group/background".into());
    ui.invoke_remove_source("overlay-group/pattern".into());

    let state = state.borrow();
    let group = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(obs_rs_project::SceneItemSpec::group)
        .expect("group after UI callbacks");
    assert_eq!(
        group.items().len(),
        2,
        "the nested remove callback removes one child after duplication"
    );
    assert_eq!(
        group.items()[0].source_id().as_str(),
        "background",
        "the group move callback must use the group-local order"
    );
    assert!(!group.items()[0].visible());
    assert!(group.items()[0].locked());
    assert!(group.items()[0].transform().flip_x());
    assert_eq!(group.items()[1].source_id().as_str(), "background_copy");
}

/// Opens the File menu through its actual pointer target and proves its popup
/// participates in hit testing outside the navbar's 26px bounds.
fn exercise_navbar_popup(ui: &MainWindow) {
    let file_button = ElementHandle::find_by_element_id(ui, "AppNavbar::file-button")
        .next()
        .expect("File menu button is discoverable");
    file_button.mock_single_click(PointerEventButton::Left);

    let entries = ElementHandle::find_by_element_type_name(ui, "MenuEntry").collect::<Vec<_>>();
    assert_eq!(entries.len(), 7, "the complete File popup is visible");
    entries[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        ElementHandle::find_by_element_type_name(ui, "MenuEntry").count(),
        0,
        "selecting an entry closes the popup"
    );
}

/// Drives the menu-bar actions through the real callbacks.
///
/// The bar's previous failure mode was an entry that dispatched a string
/// nothing handled, so this asserts each action changes observable state rather
/// than only that it can be invoked.
fn exercise_menu_actions(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    docks: &Rc<crate::callbacks::docks::DockController>,
) {
    let projectors = crate::install_menu_callbacks(ui, state, surface, docks);

    // The exercises before this one have already edited the project, so the
    // history starts from a known-empty state rather than from their leftovers.
    ui.invoke_new_project();
    assert!(!ui.get_can_undo(), "a fresh document has nothing to undo");
    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_string();
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneName {
            profile,
            scene: "preview".to_owned(),
            name: "Renamed in test".to_owned(),
        }))
        .expect("rename the preview scene");
    refresh_ui(ui, state, surface);
    assert!(ui.get_can_undo(), "an edit becomes an undoable step");

    ui.invoke_undo_edit();
    assert!(
        !ui.get_can_undo() && ui.get_can_redo(),
        "undo consumes the step and offers it back as a redo"
    );
    ui.invoke_redo_edit();
    assert!(ui.get_can_undo());

    // Starting another new project clears the history along with the document.
    ui.invoke_new_project();
    assert!(
        !ui.get_can_undo() && !ui.get_can_redo(),
        "undo must not reach across a new document"
    );

    // A projector is a toggle, not a way to stack duplicate windows.
    assert!(!projectors.is_open(true));
    ui.invoke_open_projector(true);
    assert!(projectors.is_open(true), "the program projector opened");
    assert!(!projectors.is_open(false), "only one feed was requested");
    ui.invoke_open_projector(true);
    assert!(!projectors.is_open(true), "selecting it again closed it");

    // Resetting the layout restores the shipped arrangement whatever the row
    // was dragged into.
    let reversed = vec![4, 3, 2, 1, 0];
    ui.set_panel_order(ModelRc::new(VecModel::from(reversed.clone())));
    ui.set_show_mixer(false);
    ui.invoke_reset_dock_layout();
    assert_ne!(read_order(ui), reversed, "the reset changed the row");
    assert_eq!(read_order(ui), AppSettings::default().layout.panel_order);
    assert!(
        ui.get_show_mixer(),
        "a hidden dock comes back with the reset"
    );

    // The menu models the About and Scene Collection entries read are populated.
    assert!(!ui.get_app_version().is_empty());
    assert!(!ui.get_app_platform().is_empty());
    assert!(
        ui.get_collection_rows().row_count() >= 1,
        "the open document is always listed as a collection"
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the integration fixture exercises one complete context-menu workflow"
)]
fn exercise_context_menus(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);
    ui.invoke_new_project();
    ui.invoke_select_preview("preview".into());
    let profile = "live".to_owned();
    for id in ["middle", "foreground"] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: profile.clone(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("source defaults"),
                )
                .expect("source"),
            }))
            .expect("add source");
    }
    refresh_ui(ui, state, surface);

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
            profile: profile.clone(),
            scene: "preview".to_owned(),
            item: "foreground".to_owned(),
            transform: FrameTransform::new(500, 250, 100, 50, false, false, 255)
                .expect("source transform"),
        }))
        .expect("position source for transform command");
    refresh_ui(ui, state, surface);
    ui.invoke_transform_source("foreground".into(), "center-screen".into());
    let centered = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("foreground"))
        .expect("centered source")
        .transform();
    assert_eq!((centered.translate_x(), centered.translate_y()), (160, 135));

    let rows = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>();
    println!(
        "source rows model={} handles={:?}",
        ui.get_source_rows().row_count(),
        ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
            .map(|row| (row.size(), row.absolute_position(), row.id()))
            .collect::<Vec<_>>()
    );
    assert_eq!(rows.len(), 3);
    let row_target = rows[1]
        .query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|target| target.size().width > 150.0 && target.size().height > 30.0)
        .find_first()
        .expect("source row hit target");
    println!(
        "right click target={:?} id={:?} selected-before={:?}",
        row_target.size(),
        row_target.id(),
        state.borrow().selected_source()
    );
    let position = row_target.absolute_position();
    let size = row_target.size();
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(
            position.x + size.width / 2.0,
            position.y + size.height / 2.0,
        ),
        button: PointerEventButton::Right,
    });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    for _ in 0..6 {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::UpArrow.into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::UpArrow.into(),
        });
    }
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    println!(
        "after keyboard duplicate sources={:?}",
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .map(|scene| scene
                .sources()
                .iter()
                .map(|source| source.id().to_string())
                .collect::<Vec<_>>())
    );
    println!("selected-after={:?}", state.borrow().selected_source());
    let entries = ElementHandle::find_by_element_type_name(ui, "MenuEntry").collect::<Vec<_>>();
    println!(
        "source menu entries: {:?}",
        entries
            .iter()
            .map(|entry| {
                (
                    entry.type_name().map(|value| value.to_string()),
                    entry.id().map(|value| value.to_string()),
                    entry.size(),
                    entry.absolute_position(),
                    entry.computed_opacity(),
                    entry.accessible_label().map(|value| value.to_string()),
                    entry.accessible_enabled(),
                    entry.accessible_checked(),
                )
            })
            .collect::<Vec<_>>()
    );
    for type_name in [
        "MenuEntry",
        "MenuItem",
        "MenuItemBase",
        "MenuFrame",
        "ContextMenuInternal",
    ] {
        println!(
            "{} count={}",
            type_name,
            ElementHandle::find_by_element_type_name(ui, type_name).count()
        );
    }
    for type_name in [
        "PopupMenuImpl",
        "FocusScope",
        "MenuFrameBase",
        "Text",
        "TouchArea",
        "Window",
    ] {
        println!(
            "{} count={}",
            type_name,
            ElementHandle::find_by_element_type_name(ui, type_name).count()
        );
    }
    println!(
        "context ids={:?} context types={:?}",
        ElementHandle::find_by_element_id(ui, "SourceContextMenuArea::context-menu")
            .map(|element| (element.type_name(), element.id(), element.size()))
            .collect::<Vec<_>>(),
        ElementHandle::find_by_element_type_name(ui, "ContextMenuArea")
            .map(|element| (element.type_name(), element.id(), element.size()))
            .collect::<Vec<_>>()
    );
    println!(
        "compact buttons={:?}",
        ElementHandle::find_by_element_type_name(ui, "CompactButton")
            .map(|button| (
                button.size(),
                button.absolute_position(),
                button.accessible_label()
            ))
            .collect::<Vec<_>>()
    );
    let more = ElementHandle::find_by_element_type_name(ui, "CompactButton")
        .find(|button| {
            let position = button.absolute_position();
            position.x > 180.0 && position.x < 320.0 && position.y > 800.0
        })
        .expect("source more button");
    more.mock_single_click(PointerEventButton::Left);
    println!(
        "after more menu entries={:?}",
        ElementHandle::find_by_element_type_name(ui, "MenuEntry")
            .map(|entry| (
                entry.type_name(),
                entry.id(),
                entry.size(),
                entry.absolute_position()
            ))
            .collect::<Vec<_>>()
    );
    let context = ElementHandle::find_by_element_type_name(ui, "ContextMenuArea")
        .find(|element| {
            let position = element.absolute_position();
            element.size().height > 30.0 && position.y > 680.0 && position.y < 720.0
        })
        .expect("source context area");
    context.mock_single_click(PointerEventButton::Right);
    println!(
        "after context right menu entries={:?}",
        ElementHandle::find_by_element_type_name(ui, "MenuEntry")
            .map(|entry| (
                entry.type_name(),
                entry.id(),
                entry.size(),
                entry.absolute_position()
            ))
            .collect::<Vec<_>>()
    );
}

/// Pushes a stored layout into the real window and reads it back, which is the
/// round trip a restart performs.
fn exercise_layout_restore(ui: &MainWindow) {
    let mut stored = AppSettings::default();
    stored.layout.panel_order = vec![2, 4, 0, 1, 3];
    stored.layout.dock_tree =
        DockNode::from_legacy(&stored.layout.panel_order, &stored.layout.panel_weights)
            .expect("test layout should have a valid dock tree");
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
fn exercise_dock_layout(ui: &MainWindow, controller: &Rc<crate::callbacks::docks::DockController>) {
    assert!(!ui.get_meters_paused());
    ui.invoke_toggle_meters_paused();
    assert!(ui.get_meters_paused(), "the mixer monitor pauses");
    ui.invoke_toggle_meters_paused();
    assert!(!ui.get_meters_paused(), "the mixer monitor resumes");

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
    assert_eq!(ui.get_dock_drop_target(), 4);
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

    // Detaching again returns it to the row.
    ui.invoke_float_panel(2);
    assert!(!controller.is_floating(2), "the mixer re-docked");
    assert!(!read_floating(ui)[2]);

    // The tree callbacks drive the same pane projection used by the visible
    // workspace: tabbing keeps one region, selecting a tab changes its active
    // leaf, and a split creates a second bounded region.
    ui.invoke_tab_dock_with(4, 3);
    let panes = read_dock_panes(ui);
    assert!(panes.iter().any(|pane| pane.tab_count == 2));
    ui.invoke_select_dock_tab(4);
    assert!(read_dock_panes(ui)
        .iter()
        .any(|pane| pane.panel_kind == 4 && pane.active));
    ui.invoke_split_dock_with(2, 4, 1, 500);
    assert_eq!(read_dock_panes(ui).len(), 5);
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

fn read_dock_panes(ui: &MainWindow) -> Vec<crate::DockPane> {
    let model = ui.get_dock_panes();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

fn read_dock_splitters(ui: &MainWindow) -> Vec<crate::DockSplitter> {
    let model = ui.get_dock_splitters();
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
    surface: &Rc<RefCell<PreviewSurface>>,
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
    refresh_ui(ui, state, surface);
    assert!(
        ui.get_selected_source_is_screen(),
        "an X11 screen source must offer the display picker"
    );

    let controller = crate::install_monitor_window(ui, state, surface).expect("monitor controller");
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
        .and_then(|profile| profile.source("gui-screen"))
        .expect("screen source persisted");
    assert_eq!(
        source.settings().get("monitor"),
        Some(""),
        "the display choice must reach the project command"
    );
}

/// Drives the real settings controller through Apply, Cancel, and OK.
///
/// The draft semantics are the whole point of the window, so they are checked
/// against the controller rather than against a hand-built stand-in: Apply
/// persists and clears the dirty flag, Cancel discards every draft including
/// the live-previewed appearance, OK persists and closes, and a field that
/// fails validation commits nothing at all.
fn exercise_settings_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
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
        },
    )
    .expect("settings controller should install");
    let window = controller.window();

    // Opening the window fills the draft from the committed document, so a
    // freshly opened window has nothing to apply.
    ui.invoke_open_settings_window();
    assert!(!window.get_dirty(), "a freshly loaded draft is not dirty");

    // Apply: every draft is persisted and the button goes quiet again.
    window.set_density_index(3);
    window.set_font_size(16.0);
    window.set_style_index(2);
    window.invoke_edit_output_resolution("1280x720".into());
    window.set_scale_filter_index(2);
    window.set_recording_quality_index(2);
    window.set_recording_filename_without_spaces(true);
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
    assert_eq!(AppSettings::load(&path), committed, "Apply writes the file");

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

    // Cancel discards every draft, including the appearance that was already
    // previewed onto the live windows.
    window.invoke_cancel_settings();
    assert!(!window.get_dirty());
    assert_eq!(controller.committed().font_size, 16);

    // OK persists and closes.
    ui.invoke_open_settings_window();
    window.set_recording_quality_index(0);
    window.set_dirty(true);
    window.invoke_accept_settings();
    assert!(!window.get_dirty(), "OK applies before it closes");
    assert_eq!(
        controller.committed().recording_quality,
        crate::settings_model::RecordingQuality::SameAsStream
    );
    assert_eq!(
        AppSettings::load(&path).recording_quality,
        crate::settings_model::RecordingQuality::SameAsStream
    );
    std::fs::remove_file(&path).expect("remove settings fixture");
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

/// Renders the properties window in both locales so a missing catalog field or
/// broken layout fails the suite.
fn render_source_properties_window() {
    let window = SourcePropertiesWindow::new().expect("properties window should instantiate");
    window.set_source_name("Background".into());
    window.set_source_kind("color_source".into());
    window.set_source_settings("color = \"#405070FF\"\nheight = 360\nwidth = 640\n".into());
    window.set_property_rows(ModelRc::new(VecModel::from(crate::properties::rows(
        "color_source",
        "color = \"#405070FF\"\nheight = 360\nwidth = 640\n",
        UiLocale::English,
    ))));
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

/// Exercises the standalone filter list through its project-command callbacks.
#[allow(
    clippy::too_many_lines,
    reason = "the GUI fixture keeps the complete ordered filter-window workflow together"
)]
fn render_source_filters_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "background".to_owned(),
        })
        .expect("background source should be selectable");
    refresh_ui(ui, state, surface);

    let controller = crate::install_source_filters_window(ui, state, surface)
        .expect("filters window should instantiate");
    ui.invoke_open_source_filters_window();
    let window = crate::callbacks::source_filters::source_filters_window(&controller);
    assert_eq!(window.get_source_name(), "Background");
    window.show().expect("filters window should show");
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        let snapshot = window
            .window()
            .take_snapshot()
            .expect("filters window should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
    }

    window.invoke_add_filter("brightness".into());
    window.invoke_add_filter("grayscale".into());
    let selected_id = window.get_selected_filter_id();
    assert_eq!(selected_id, "grayscale");
    window.invoke_rename_filter("Scene grayscale".into());
    window.invoke_move_filter(-1);
    window.invoke_select_filter("brightness".into());
    window.invoke_toggle_filter();
    window.invoke_edit_property("milli".into(), "450".into());

    window.invoke_add_filter("color_correction".into());
    window.invoke_edit_property("gamma".into(), "1000".into());
    window.invoke_edit_property("opacity".into(), "900".into());
    window.invoke_add_filter("color_multiply_add".into());
    window.invoke_edit_property("multiply_red".into(), "220".into());
    window.invoke_edit_property("add_blue".into(), "12".into());
    window.invoke_add_filter("luma_key".into());
    window.invoke_edit_property("luma_min".into(), "250".into());
    window.invoke_add_filter("color_key".into());
    window.invoke_edit_property("similarity".into(), "200".into());
    window.invoke_add_filter("chroma_key".into());
    window.invoke_edit_property("spill".into(), "140".into());
    window.invoke_add_filter("sharpen".into());
    window.invoke_edit_property("sharpness".into(), "120".into());
    window.invoke_add_filter("scroll".into());
    window.invoke_edit_property("speed_x".into(), "120".into());
    window.invoke_edit_property("speed_y".into(), "-80".into());
    window.invoke_edit_property("loop".into(), "false".into());
    window.invoke_add_filter("render_delay".into());
    window.invoke_edit_property("milliseconds".into(), "100".into());
    window.invoke_add_filter("noise_gate".into());
    window.invoke_edit_property("open_threshold_db_milli".into(), "-26000".into());
    window.invoke_edit_property("close_threshold_db_milli".into(), "-32000".into());

    let state_ref = state.borrow();
    let source = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("background"))
        .expect("background source after filter edits");
    assert_eq!(source.filters().len(), 11);
    assert_eq!(source.filters()[0].id().as_str(), "grayscale");
    assert_eq!(source.filters()[0].name(), "Scene grayscale");
    let brightness = source
        .filters()
        .iter()
        .find(|filter| filter.id().as_str() == "brightness")
        .expect("brightness filter");
    assert!(!brightness.enabled());
    assert_eq!(brightness.settings().get("milli"), Some("450"));
    let color_correction = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "color_correction")
        .expect("color correction filter");
    assert_eq!(color_correction.settings().get("gamma"), Some("1000"));
    assert_eq!(color_correction.settings().get("opacity"), Some("900"));
    let color_multiply_add = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "color_multiply_add")
        .expect("color multiply/add filter");
    assert_eq!(
        color_multiply_add.settings().get("multiply_red"),
        Some("220")
    );
    assert_eq!(color_multiply_add.settings().get("add_blue"), Some("12"));
    let luma_key = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "luma_key")
        .expect("luma key filter");
    assert_eq!(luma_key.settings().get("luma_max"), Some("1000"));
    assert_eq!(luma_key.settings().get("luma_min"), Some("250"));
    let color_key = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "color_key")
        .expect("color key filter");
    assert_eq!(color_key.settings().get("key_green"), Some("255"));
    assert_eq!(color_key.settings().get("similarity"), Some("200"));
    let chroma_key = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "chroma_key")
        .expect("chroma key filter");
    assert_eq!(chroma_key.settings().get("key_green"), Some("255"));
    assert_eq!(chroma_key.settings().get("spill"), Some("140"));
    let sharpen = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "sharpen")
        .expect("sharpen filter");
    assert_eq!(sharpen.settings().get("sharpness"), Some("120"));
    let scroll = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "scroll")
        .expect("scroll filter");
    assert_eq!(scroll.settings().get("speed_x"), Some("120"));
    assert_eq!(scroll.settings().get("speed_y"), Some("-80"));
    assert_eq!(scroll.settings().get("loop"), Some("false"));
    let render_delay = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "render_delay")
        .expect("render delay filter");
    assert_eq!(render_delay.settings().get("milliseconds"), Some("100"));
    let noise_gate = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "noise_gate")
        .expect("noise gate filter");
    assert_eq!(
        noise_gate.settings().get("open_threshold_db_milli"),
        Some("-26000")
    );
    assert_eq!(
        noise_gate.settings().get("close_threshold_db_milli"),
        Some("-32000")
    );
    drop(state_ref);

    window.invoke_select_filter("color_key".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("chroma_key".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("sharpen".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("scroll".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("render_delay".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("noise_gate".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("luma_key".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("color_correction".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("color_multiply_add".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("grayscale".into());
    window.invoke_remove_filter();
    assert_eq!(window.get_effect_rows().row_count(), 1);
    window.invoke_select_filter("brightness".into());
    window.set_selected_filter_name("Uncommitted name".into());
    window.invoke_close_window();
    ui.invoke_open_source_filters_window();
    assert_ne!(window.get_selected_filter_name(), "Uncommitted name");
    window.invoke_close_window();
}

/// Confirms the transform dialog edits scene-item state and does not add
/// transform fields back to source properties.
fn exercise_source_transform_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let controller = crate::install_source_transform_window(ui, state, surface)
        .expect("transform window should instantiate");
    ui.invoke_open_source_transform_window();
    let window = crate::callbacks::source_transform::source_transform_window(&controller);
    assert_eq!(window.get_source_name(), "Background");
    window.show().expect("transform window should show");
    window.set_position_x(42);
    window.set_position_y(-7);
    window.set_item_opacity(200);
    window.set_flip_horizontal(true);
    window.set_rotation_degrees(90);
    window.invoke_accept_transform();

    let state_ref = state.borrow();
    let scene_id = state_ref.preview_scene().expect("preview scene");
    let _source = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("background"))
        .expect("background source after transform edit");
    let item = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.item("background"))
        .expect("background item after transform edit");
    assert_eq!(item.transform().translate_x(), 42);
    assert_eq!(item.transform().translate_y(), -7);
    assert_eq!(item.transform().opacity(), 200);
    assert!(item.transform().flip_x());
    assert_eq!(item.transform().rotation_degrees(), 90);
    drop(state_ref);

    ui.invoke_open_source_transform_window();
    window.invoke_reset_transform();
    assert_eq!(window.get_position_x(), 0);
    assert_eq!(window.get_position_y(), 0);
    window.invoke_close_window();

    let state_ref = state.borrow();
    let scene_id = state_ref.preview_scene().expect("preview scene");
    let item = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.item("background"))
        .expect("background item after transform cancel");
    assert_eq!(item.transform().translate_x(), 42);
}

/// Drives the Add Source window the way a user would: pick a kind, create a
/// source, then copy an existing one into the current scene.
fn exercise_add_source_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let controller = crate::install_add_source_window(ui, state, surface)
        .expect("add source window should instantiate");
    let window = crate::callbacks::add_source_window(&controller);
    window.show().expect("add source window should show");

    // Every registered kind must produce a renderable page.
    for kind in crate::preview::builtin_source_kinds() {
        crate::callbacks::populate_add_source_window(&controller, state, &kind);
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
    crate::callbacks::populate_add_source_window(&controller, state, "color_source");

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

    crate::callbacks::populate_add_source_window(&controller, state, "color_source");
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
    crate::callbacks::populate_add_source_window(&controller, state, "color_source");
    assert!(
        !window
            .get_candidates()
            .iter()
            .any(|row| row.id == candidate.id),
        "a source that is already in the scene must not be offered again"
    );

    // "Recently added" lists existing sources only, so it offers no creation.
    crate::callbacks::populate_add_source_window(&controller, state, "@recent");
    assert!(!window.get_can_create());

    window.hide().expect("add source window should hide");
}

/// Verifies the complete screen/camera source-properties path: selecting a
/// camera source, changing its device in the `ComboBox` callback, and accepting
/// the draft writes the selected stable device ID back into the project.
fn exercise_capture_device_properties_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
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
    refresh_ui(ui, state, surface);

    let controller =
        crate::install_source_properties_window(ui, state, surface).expect("properties controller");
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
    assert!(window.get_source_settings().contains("device_id = "));
    window.invoke_accept_properties();

    let state = state.borrow();
    let source = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("gui-camera"))
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
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-callback-{token}.obsr"));
    ui.set_recording_path(path.to_string_lossy().into_owned().into());
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);

    ui.invoke_toggle_recording();
    assert!(
        state.borrow().recording(),
        "Record button must start the state"
    );
    let frame = PreviewRenderer::new(state.borrow().project_session().project(), 0)
        .expect("preview renderer")
        .render("program")
        .expect("program frame")
        .expect("program scene frame");
    crate::callbacks::push_program_frame(ui, None, None, Some(frame), &output);
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

    exercise_output_reconciliation(ui, state, &output);
}

/// Checks the desktop stops claiming an output the engine is not running.
///
/// The controls set their booleans optimistically, so a start the engine
/// refused would otherwise leave the window showing "recording" forever.
fn exercise_output_reconciliation(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    // The engine is idle here, so both claims are stale by construction.
    state
        .borrow_mut()
        .dispatch(UiCommand::StartRecording)
        .expect("claim recording");
    state
        .borrow_mut()
        .dispatch(UiCommand::StartStreaming)
        .expect("claim streaming");
    ui.set_recording(true);
    ui.set_streaming(true);

    crate::callbacks::reconcile_output_lifecycle(ui, state, output);

    assert!(
        !state.borrow().recording(),
        "a recording the engine never opened must not stay claimed"
    );
    assert!(
        !state.borrow().streaming(),
        "a stream the engine never opened must not stay claimed"
    );
    assert!(!ui.get_recording() && !ui.get_streaming());
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
