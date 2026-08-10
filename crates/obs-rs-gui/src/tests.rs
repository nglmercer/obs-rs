use super::i18n::catalog;
use super::refresh::transition_label_for_locale;
use super::{initial_project, refresh_ui, MainWindow, OutputRuntime, PreviewRenderer};
use obs_rs_media::{FrameRate, FrameTransition, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::encode_png;
use obs_rs_project::{ProjectCommand, SceneSpec};
use obs_rs_ui::{DesktopState, UiLocale};
use slint::ComponentHandle;
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
    let mut renderer = PreviewRenderer::new(&project).expect("preview renderer should build");
    let frame = renderer
        .render("preview")
        .expect("preview scene should render")
        .expect("preview scene should produce a frame");
    assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
}

#[test]
fn preview_renderer_composes_scene_transitions() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project).expect("preview renderer should build");
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
    let mut renderer = PreviewRenderer::new(&project).expect("preview renderer should build");
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
fn preview_renderer_rebuilds_after_project_edit() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project).expect("preview renderer should build");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("new-scene", "New scene").expect("scene"),
        })
        .expect("add scene");

    renderer
        .sync_project(&project)
        .expect("renderer should rebuild from the edited project");
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
    let mut renderer = PreviewRenderer::new(&project).expect("preview renderer should build");
    assert!(renderer
        .render("preview")
        .expect("hidden scene should render")
        .is_none());
}

#[test]
fn output_runtime_finalizes_an_atomic_y4m_recording() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-gui-output-{token}.y4m"));
    let mut output = OutputRuntime::new(format);
    output
        .start_recording(final_path.to_str().expect("UTF-8 temp path"))
        .expect("recording should open");
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [20, 30, 40, 255]);
    output.push_frame(&frame).expect("frame should be accepted");
    let bytes = output
        .finish_recording()
        .expect("recording should finalize");
    assert!(bytes > 0);
    let persisted = std::fs::read(&final_path).expect("recording should be persisted");
    assert_eq!(persisted.len(), bytes);
    assert!(persisted.starts_with(b"YUV4MPEG2"));
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
        PreviewRenderer::new(&project).expect("preview renderer"),
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
}
