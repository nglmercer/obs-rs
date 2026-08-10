//! Slint desktop control room for the Rust-owned OBS-RS state machine.

#![deny(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{cell::RefCell, error::Error, rc::Rc, time::Duration};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::Runtime;
use obs_rs_media::{FrameRate, FrameTransition, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Profile, Project, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel,
    Weak,
};

slint::slint! {
    import { Button, HorizontalBox, ScrollView, VerticalBox } from "std-widgets.slint";

    export struct SceneRow {
        id: string,
        name: string,
        role: string,
    }

    export component MainWindow inherits Window {
        width: 1180px;
        height: 780px;
        title: "OBS-RS Studio";
        background: rgb(17, 24, 39);

        in property <string> project-title;
        in property <string> profile-name;
        in property <string> preview-scene;
        in property <string> program-scene;
        in property <string> transition;
        in property <bool> recording;
        in property <bool> streaming;
        in property <bool> dirty;
        in property <string> status-message;
        in property <string> snapshot;
        in property <image> preview-image;
        in property <image> program-image;
        in property <[SceneRow]> scene-rows;

        callback swap-scenes();
        callback toggle-recording();
        callback toggle-streaming();
        callback cut-transition();
        callback fade-transition();
        callback select-preview(string);
        callback select-program(string);

        VerticalBox {
            padding: 22px;
            spacing: 14px;

            HorizontalBox {
                spacing: 14px;

                VerticalBox {
                    spacing: 3px;
                    Text {
                        text: "OBS-RS Studio";
                        color: rgb(249, 250, 251);
                        font-size: 26px;
                        font-weight: 700;
                    }
                    Text {
                        text: project-title + "  /  " + profile-name;
                        color: rgb(156, 163, 175);
                        font-size: 14px;
                    }
                }
                Rectangle { horizontal-stretch: 1; }
                Text {
                    text: dirty ? "● Unsaved project" : "● Saved project";
                    color: dirty ? rgb(251, 191, 36) : rgb(134, 239, 172);
                    vertical-alignment: center;
                    font-size: 14px;
                }
            }

            HorizontalBox {
                spacing: 14px;

                Rectangle {
                    background: rgb(11, 18, 32);
                    border-width: 1px;
                    border-color: rgb(51, 65, 85);
                    border-radius: 8px;
                    horizontal-stretch: 1;
                    min-height: 280px;
                    VerticalBox {
                        padding: 16px;
                        spacing: 8px;
                        Text {
                            text: "PREVIEW";
                            color: rgb(96, 165, 250);
                            font-size: 13px;
                            font-weight: 700;
                        }
                        Rectangle {
                            background: rgb(3, 7, 18);
                            border-radius: 5px;
                            vertical-stretch: 1;
                            Image {
                                source: preview-image;
                                width: 100%;
                                height: 100%;
                                image-fit: contain;
                            }
                        }
                        Text {
                            text: preview-scene;
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            horizontal-alignment: center;
                        }
                        Text {
                            text: "Queued scene";
                            color: rgb(148, 163, 184);
                            horizontal-alignment: center;
                        }
                    }
                }

                Rectangle {
                    background: rgb(22, 15, 24);
                    border-width: 1px;
                    border-color: rgb(127, 29, 29);
                    border-radius: 8px;
                    horizontal-stretch: 1;
                    min-height: 280px;
                    VerticalBox {
                        padding: 16px;
                        spacing: 8px;
                        Text {
                            text: "PROGRAM";
                            color: rgb(248, 113, 113);
                            font-size: 13px;
                            font-weight: 700;
                        }
                        Rectangle {
                            background: rgb(3, 7, 18);
                            border-radius: 5px;
                            vertical-stretch: 1;
                            Image {
                                source: program-image;
                                width: 100%;
                                height: 100%;
                                image-fit: contain;
                            }
                        }
                        Text {
                            text: program-scene;
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            horizontal-alignment: center;
                        }
                        Text {
                            text: "On air scene";
                            color: rgb(252, 165, 165);
                            horizontal-alignment: center;
                        }
                    }
                }
            }

            HorizontalBox {
                spacing: 14px;
                Rectangle {
                    background: rgb(31, 41, 55);
                    border-radius: 8px;
                    horizontal-stretch: 2;
                    VerticalBox {
                        padding: 14px;
                        spacing: 8px;
                        Text {
                            text: "Scenes";
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            font-weight: 700;
                        }
                        ScrollView {
                            vertical-stretch: 1;
                            for scene in scene-rows : Rectangle {
                                height: 58px;
                                background: rgb(39, 52, 73);
                                border-radius: 5px;
                                HorizontalBox {
                                    padding: 8px;
                                    spacing: 8px;
                                    VerticalBox {
                                        spacing: 2px;
                                        Text {
                                            text: scene.name;
                                            color: rgb(249, 250, 251);
                                            font-size: 15px;
                                        }
                                        Text {
                                            text: scene.id + (scene.role == "" ? "" : "  ·  " + scene.role);
                                            color: rgb(148, 163, 184);
                                            font-size: 12px;
                                        }
                                    }
                                    Rectangle { horizontal-stretch: 1; }
                                    Button {
                                        text: "Preview";
                                        clicked => select-preview(scene.id);
                                    }
                                    Button {
                                        text: "Program";
                                        clicked => select-program(scene.id);
                                    }
                                }
                            }
                        }
                    }
                }

                Rectangle {
                    background: rgb(31, 41, 55);
                    border-radius: 8px;
                    horizontal-stretch: 1;
                    VerticalBox {
                        padding: 14px;
                        spacing: 8px;
                        Text {
                            text: "Controls";
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            font-weight: 700;
                        }
                        Button {
                            text: "Swap preview / program";
                            clicked => swap-scenes();
                        }
                        HorizontalBox {
                            spacing: 8px;
                            Button {
                                text: "Cut";
                                clicked => cut-transition();
                            }
                            Button {
                                text: "Fade";
                                clicked => fade-transition();
                            }
                        }
                        Text {
                            text: "Transition: " + transition;
                            color: rgb(203, 213, 225);
                            font-size: 13px;
                        }
                        HorizontalBox {
                            spacing: 8px;
                            Button {
                                text: recording ? "Stop recording" : "Start recording";
                                clicked => toggle-recording();
                            }
                            Button {
                                text: streaming ? "Stop streaming" : "Start streaming";
                                clicked => toggle-streaming();
                            }
                        }
                        Text {
                            text: recording ? "Recording: active" : "Recording: stopped";
                            color: recording ? rgb(252, 165, 165) : rgb(148, 163, 184);
                            font-size: 13px;
                        }
                        Text {
                            text: streaming ? "Streaming: active" : "Streaming: stopped";
                            color: streaming ? rgb(252, 165, 165) : rgb(148, 163, 184);
                            font-size: 13px;
                        }
                        Rectangle { vertical-stretch: 1; }
                        Text {
                            text: status-message;
                            color: rgb(147, 197, 253);
                            wrap: word-wrap;
                        }
                    }
                }
            }

            Rectangle {
                background: rgb(15, 23, 42);
                border-width: 1px;
                border-color: rgb(51, 65, 85);
                border-radius: 8px;
                min-height: 96px;
                VerticalBox {
                    padding: 12px;
                    spacing: 5px;
                    Text {
                        text: "Accessible state snapshot";
                        color: rgb(203, 213, 225);
                        font-size: 13px;
                        font-weight: 700;
                    }
                    Text {
                        text: snapshot;
                        color: rgb(148, 163, 184);
                        font-size: 11px;
                        wrap: word-wrap;
                        vertical-stretch: 1;
                    }
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new()?;
    let project = initial_project()?;
    let renderer = Rc::new(RefCell::new(PreviewRenderer::new(&project)?));
    let state = Rc::new(RefCell::new(DesktopState::new(project)));

    refresh_ui(&ui, &state, &renderer);
    install_callbacks(&ui, &state, &renderer);

    if smoke {
        return Ok(());
    }

    let _preview_timer = start_preview_timer(&ui, &state, &renderer);
    ui.run()?;
    Ok(())
}

fn start_preview_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) -> Timer {
    let timer = Timer::default();
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        refresh_ui(&ui, &state, &renderer);
    });
    timer
}

fn install_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let swap_state = Rc::clone(state);
    let swap_renderer = Rc::clone(renderer);
    ui.on_swap_scenes(move || {
        dispatch_and_refresh(
            &weak,
            &swap_state,
            &swap_renderer,
            UiCommand::SwapPreviewProgram,
        );
    });

    let weak = ui.as_weak();
    let recording_state = Rc::clone(state);
    let recording_renderer = Rc::clone(renderer);
    ui.on_toggle_recording(move || {
        let command = if recording_state.borrow().recording() {
            UiCommand::StopRecording
        } else {
            UiCommand::StartRecording
        };
        dispatch_and_refresh(&weak, &recording_state, &recording_renderer, command);
    });

    let weak = ui.as_weak();
    let streaming_state = Rc::clone(state);
    let streaming_renderer = Rc::clone(renderer);
    ui.on_toggle_streaming(move || {
        let command = if streaming_state.borrow().streaming() {
            UiCommand::StopStreaming
        } else {
            UiCommand::StartStreaming
        };
        dispatch_and_refresh(&weak, &streaming_state, &streaming_renderer, command);
    });

    let weak = ui.as_weak();
    let cut_state = Rc::clone(state);
    let cut_renderer = Rc::clone(renderer);
    ui.on_cut_transition(move || {
        dispatch_and_refresh(
            &weak,
            &cut_state,
            &cut_renderer,
            UiCommand::SetTransition {
                transition: FrameTransition::Cut,
            },
        );
    });

    let weak = ui.as_weak();
    let fade_state = Rc::clone(state);
    let fade_renderer = Rc::clone(renderer);
    ui.on_fade_transition(move || {
        dispatch_and_refresh(
            &weak,
            &fade_state,
            &fade_renderer,
            UiCommand::SetTransition {
                transition: FrameTransition::CrossFade {
                    progress_milli: 500,
                },
            },
        );
    });

    let weak = ui.as_weak();
    let preview_state = Rc::clone(state);
    let preview_renderer = Rc::clone(renderer);
    ui.on_select_preview(move |id| {
        dispatch_and_refresh(
            &weak,
            &preview_state,
            &preview_renderer,
            UiCommand::SelectPreviewScene { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let program_state = Rc::clone(state);
    let program_renderer = Rc::clone(renderer);
    ui.on_select_program(move |id| {
        dispatch_and_refresh(
            &weak,
            &program_state,
            &program_renderer,
            UiCommand::SelectProgramScene { id: id.to_string() },
        );
    });
}

fn dispatch_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    command: UiCommand,
) {
    let result = state.borrow_mut().dispatch(command);
    let Some(ui) = weak.upgrade() else {
        return;
    };
    if let Err(error) = result {
        ui.set_status_message(format!("Command failed: {error}").into());
    } else {
        refresh_ui(&ui, state, renderer);
    }
}

fn refresh_ui(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let state = state.borrow();
    let project = state.project_session().project();
    let profile_id = project.active_profile();
    let profile = project
        .profiles()
        .find(|profile| profile.id() == profile_id);
    let profile_name =
        profile.map_or_else(|| "No profile".to_owned(), |value| value.name().to_owned());

    ui.set_project_title(project.title().into());
    ui.set_profile_name(profile_name.into());
    ui.set_preview_scene(state.preview_scene().unwrap_or("none").into());
    ui.set_program_scene(state.program_scene().unwrap_or("none").into());
    ui.set_transition(transition_label(state.transition()).into());
    ui.set_recording(state.recording());
    ui.set_streaming(state.streaming());
    ui.set_dirty(state.is_dirty());
    ui.set_snapshot(state.accessible_snapshot().into());

    let (preview_image, preview_error) = scene_image(renderer, state.preview_scene());
    let (program_image, program_error) = scene_image(renderer, state.program_scene());
    ui.set_preview_image(preview_image);
    ui.set_program_image(program_image);
    let render_error = preview_error.or(program_error);
    ui.set_status_message(
        render_error
            .unwrap_or_else(|| latest_notice(&state).to_owned())
            .into(),
    );

    let rows = profile.map_or_else(Vec::new, |profile| {
        profile
            .scenes()
            .map(|scene| {
                let id = scene.id().to_string();
                let role = scene_role(&state, &id);
                SceneRow {
                    id: id.into(),
                    name: scene.name().into(),
                    role: role.into(),
                }
            })
            .collect::<Vec<_>>()
    });
    ui.set_scene_rows(ModelRc::new(VecModel::from(rows)));
}

fn latest_notice(state: &DesktopState) -> &str {
    state
        .notices()
        .last()
        .map_or("Ready", |notice| notice.message())
}

fn scene_role(state: &DesktopState, id: &str) -> &'static str {
    match (
        state.preview_scene() == Some(id),
        state.program_scene() == Some(id),
    ) {
        (true, true) => "Preview / Program",
        (true, false) => "Preview",
        (false, true) => "Program",
        (false, false) => "",
    }
}

fn transition_label(transition: FrameTransition) -> String {
    match transition {
        FrameTransition::Cut => "Cut".to_owned(),
        FrameTransition::CrossFade { progress_milli } => {
            format!("Fade {progress_milli}/1000")
        }
    }
}

struct PreviewRenderer {
    format: VideoFormat,
    runtime: Runtime,
}

impl PreviewRenderer {
    fn new(project: &Project) -> Result<Self, Box<dyn Error>> {
        let active_profile = project.active_profile();
        let profile = project
            .profiles()
            .find(|profile| profile.id() == active_profile)
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        let format = profile.video_format();
        let mut runtime = Runtime::new();
        let plugin = BuiltinPlugin::new()?;
        runtime.register_plugin(&plugin)?;

        for scene in profile.scenes() {
            let scene_id = scene.id().as_str();
            runtime.create_scene(scene_id)?;
            for source in scene.sources() {
                let source_id = runtime.create_source(
                    source.kind().as_str(),
                    source.name(),
                    source.settings(),
                )?;
                runtime.attach_source(scene_id, source_id)?;
                runtime.set_source_transform(scene_id, source_id, source.transform())?;
                for filter in source.filters() {
                    runtime.add_source_filter(scene_id, source_id, *filter)?;
                }
            }
        }

        Ok(Self { format, runtime })
    }

    fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(Timestamp::ZERO, self.format);
        Ok(self.runtime.render_scene(scene, &request)?)
    }
}

fn scene_image(
    renderer: &Rc<RefCell<PreviewRenderer>>,
    scene: Option<&str>,
) -> (Image, Option<String>) {
    let Some(scene) = scene else {
        return (Image::default(), None);
    };
    match renderer.borrow_mut().render(scene) {
        Ok(Some(frame)) => (frame_to_image(&frame), None),
        Ok(None) => (
            Image::default(),
            Some(format!("Scene {scene} has no frame")),
        ),
        Err(error) => (Image::default(), Some(format!("Preview renderer: {error}"))),
    }
}

fn frame_to_image(frame: &VideoFrame) -> Image {
    let format = frame.format();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(format.width(), format.height());
    for (pixel, channels) in buffer
        .make_mut_slice()
        .iter_mut()
        .zip(frame.pixels().chunks_exact(4))
    {
        *pixel = Rgba8Pixel::new(channels[0], channels[1], channels[2], channels[3]);
    }
    Image::from_rgba8(buffer)
}

fn initial_project() -> Result<Project, Box<dyn Error>> {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut project = Project::new("OBS-RS Studio")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    profile.add_scene(scene("preview", "Preview", "#102030FF")?)?;
    profile.add_scene(scene("program", "Program", "#203040FF")?)?;
    profile.add_scene(scene("intermission", "Intermission", "#302040FF")?)?;
    project.add_profile(profile)?;
    Ok(project)
}

fn scene(id: &str, name: &str, color: &str) -> Result<SceneSpec, Box<dyn Error>> {
    let mut settings = Config::new();
    settings.set("width", "640")?;
    settings.set("height", "360")?;
    settings.set("color", color)?;
    let mut scene = SceneSpec::new(id, name)?;
    scene.add_source(SourceSpec::new(
        "background",
        "color_source",
        "Background",
        settings,
    )?)?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::{initial_project, transition_label, PreviewRenderer};
    use obs_rs_media::FrameTransition;

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
        assert_eq!(transition_label(FrameTransition::Cut), "Cut");
        assert_eq!(
            transition_label(FrameTransition::CrossFade {
                progress_milli: 500
            }),
            "Fade 500/1000"
        );
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
}
