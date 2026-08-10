//! Slint desktop control room for the Rust-owned OBS-RS state machine.

#![deny(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{cell::RefCell, error::Error, path::PathBuf, rc::Rc, time::Duration};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::Runtime;
use obs_rs_diagnostics::{AtomicDiagnosticFileWriter, DiagnosticBundle};
use obs_rs_media::{
    FrameFilter, FrameRate, FrameTransform, FrameTransition, Timestamp, VideoFormat, VideoFrame,
};
use obs_rs_output::{
    AtomicY4mFileWriter, PacketDropPolicy, ReconnectPolicy, RleVideoEncoder, StreamSession,
    TcpPacketTransport, VideoEncoder,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Profile, Project, ProjectCommand, ProjectFileStore, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel,
    Weak,
};

slint::slint! {
    import { Button, HorizontalBox, LineEdit, ScrollView, Slider, TextEdit, VerticalBox } from "std-widgets.slint";

    export struct SceneRow {
        id: string,
        name: string,
        role: string,
    }

    export struct SourceRow {
        id: string,
        name: string,
        kind: string,
        order: string,
        selected: bool,
    }

    export struct ProfileRow {
        id: string,
        name: string,
    }

    export struct MixerRow {
        id: string,
        name: string,
        gain: float,
        peak: float,
        muted: bool,
    }

    export component MainWindow inherits Window {
        width: 1180px;
        height: 860px;
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
        in property <[SourceRow]> source-rows;
        in property <[ProfileRow]> profile-rows;
        in property <string> source-scene;
        in property <[MixerRow]> mixer-rows;
        in property <string> selected-source;
        in property <string> source-settings-version;
        in property <string> source-properties-version;
        in-out property <string> source-settings;
        in-out property <string> source-transform;
        in-out property <string> source-filters;
        in-out property <string> project-path;
        in-out property <string> diagnostics-path;
        in-out property <string> recording-path;
        in-out property <string> streaming-address;
        in-out property <string> new-scene-id;
        in-out property <string> new-scene-name;
        in-out property <string> new-source-id;
        in-out property <string> new-source-kind;
        in-out property <string> new-source-name;

        callback swap-scenes();
        callback toggle-recording();
        callback toggle-streaming();
        callback cut-transition();
        callback fade-transition();
        callback select-preview(string);
        callback select-program(string);
        callback select-profile(string);
        callback select-source(string);
        callback apply-source-settings();
        callback apply-source-transform();
        callback apply-source-filters();
        callback set-mixer-gain(string, int);
        callback toggle-mixer-mute(string);
        callback save-project();
        callback load-project();
        callback recover-project();
        callback export-diagnostics();
        callback add-scene(string, string);
        callback add-source(string, string, string);

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
                    HorizontalBox {
                        spacing: 6px;
                        Text {
                            text: "Profile:";
                            color: rgb(148, 163, 184);
                            font-size: 12px;
                            vertical-alignment: center;
                        }
                        for profile in profile-rows : Button {
                            text: profile.name;
                            clicked => select-profile(profile.id);
                        }
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
                    horizontal-stretch: 3;
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
                    horizontal-stretch: 3;
                    VerticalBox {
                        padding: 14px;
                        spacing: 8px;
                        Text {
                            text: "Sources  ·  " + source-scene;
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            font-weight: 700;
                        }
                        Text {
                            text: "Scene item order";
                            color: rgb(148, 163, 184);
                            font-size: 12px;
                        }
                        ScrollView {
                            vertical-stretch: 1;
                            for source in source-rows : Rectangle {
                                height: 52px;
                                background: source.selected ? rgb(30, 64, 175) : rgb(39, 52, 73);
                                border-radius: 5px;
                                HorizontalBox {
                                    padding: 8px;
                                    spacing: 8px;
                                    Text {
                                        text: source.order;
                                        color: rgb(96, 165, 250);
                                        font-size: 14px;
                                        width: 22px;
                                        vertical-alignment: center;
                                    }
                                    VerticalBox {
                                        spacing: 2px;
                                        Button {
                                            text: source.name;
                                            clicked => select-source(source.id);
                                        }
                                        Text {
                                            text: source.kind + "  ·  " + source.id;
                                            color: rgb(148, 163, 184);
                                            font-size: 11px;
                                        }
                                    }
                                }
                            }
                        }
                        Text {
                            text: source-rows.length == 0 ? "No source items" : "";
                            color: rgb(148, 163, 184);
                            font-size: 12px;
                        }
                    }
                }

                Rectangle {
                    background: rgb(31, 41, 55);
                    border-radius: 8px;
                    horizontal-stretch: 3;
                    VerticalBox {
                        padding: 14px;
                        spacing: 8px;
                        Text {
                            text: "Audio Mixer";
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            font-weight: 700;
                        }
                        ScrollView {
                            vertical-stretch: 1;
                            for channel in mixer-rows : Rectangle {
                                height: 86px;
                                background: rgb(39, 52, 73);
                                border-radius: 5px;
                                VerticalBox {
                                    padding: 8px;
                                    spacing: 4px;
                                    HorizontalBox {
                                        spacing: 6px;
                                        Text {
                                            text: channel.name;
                                            color: rgb(249, 250, 251);
                                            font-size: 13px;
                                            horizontal-stretch: 1;
                                        }
                                        Text {
                                            text: channel.muted ? "MUTED" : "LIVE";
                                            color: channel.muted ? rgb(252, 165, 165) : rgb(134, 239, 172);
                                            font-size: 10px;
                                        }
                                        Button {
                                            text: channel.muted ? "Unmute" : "Mute";
                                            clicked => toggle-mixer-mute(channel.id);
                                        }
                                    }
                                    Slider {
                                        minimum: 0;
                                        maximum: 2;
                                        step: 0.01;
                                        value: channel.gain;
                                        changed => set-mixer-gain(channel.id, round(self.value * 1000));
                                    }
                                    Rectangle {
                                        width: 100%;
                                        height: 5px;
                                        background: rgb(15, 23, 42);
                                        Rectangle {
                                            width: channel.peak * 100px;
                                            height: 100%;
                                            background: channel.muted ? rgb(127, 29, 29) : rgb(34, 197, 94);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Rectangle {
                    background: rgb(31, 41, 55);
                    border-radius: 8px;
                    horizontal-stretch: 2;
                    VerticalBox {
                        padding: 14px;
                        spacing: 8px;
                        Text {
                            text: "Controls";
                            color: rgb(249, 250, 251);
                            font-size: 18px;
                            font-weight: 700;
                        }
                        Text {
                            text: "Project file";
                            color: rgb(203, 213, 225);
                            font-size: 13px;
                            font-weight: 700;
                        }
                        HorizontalBox {
                            spacing: 6px;
                            LineEdit {
                                text <=> project-path;
                                placeholder-text: "obs-rs-project.txt";
                                horizontal-stretch: 1;
                            }
                            Button {
                                text: "Save";
                                clicked => save-project();
                            }
                            Button {
                                text: "Load";
                                clicked => load-project();
                            }
                            Button {
                                text: "Recover";
                                clicked => recover-project();
                            }
                            Button {
                                text: "Diagnostics";
                                clicked => export-diagnostics();
                            }
                        }
                        LineEdit {
                            text <=> diagnostics-path;
                            placeholder-text: "obs-rs-diagnostics.obsrdg";
                        }
                        Text {
                            text: "Scene editor";
                            color: rgb(203, 213, 225);
                            font-size: 13px;
                            font-weight: 700;
                        }
                        HorizontalBox {
                            spacing: 6px;
                            LineEdit {
                                text <=> new-scene-id;
                                placeholder-text: "scene-id";
                                horizontal-stretch: 1;
                            }
                            LineEdit {
                                text <=> new-scene-name;
                                placeholder-text: "Scene name";
                                horizontal-stretch: 1;
                            }
                            Button {
                                text: "Add";
                                clicked => add-scene(new-scene-id, new-scene-name);
                            }
                        }
                        Text {
                            text: "Source editor";
                            color: rgb(203, 213, 225);
                            font-size: 13px;
                            font-weight: 700;
                        }
                        HorizontalBox {
                            spacing: 6px;
                            LineEdit {
                                text <=> new-source-id;
                                placeholder-text: "source-id";
                                horizontal-stretch: 1;
                            }
                            LineEdit {
                                text <=> new-source-kind;
                                placeholder-text: "test_pattern";
                                horizontal-stretch: 1;
                            }
                            LineEdit {
                                text <=> new-source-name;
                                placeholder-text: "Source name";
                                horizontal-stretch: 1;
                            }
                            Button {
                                text: "Add";
                                clicked => add-source(
                                    new-source-id,
                                    new-source-kind,
                                    new-source-name
                                );
                            }
                        }
                        Text {
                            text: "Adds to preview scene: " + preview-scene;
                            color: rgb(148, 163, 184);
                            font-size: 12px;
                        }
                        Text {
                            text: "Selected source: " + selected-source;
                            color: rgb(203, 213, 225);
                            font-size: 13px;
                            font-weight: 700;
                        }
                        TextEdit {
                            text <=> source-settings;
                            placeholder-text: "width=640\\nheight=360\\ncolor=#202840FF";
                            height: 70px;
                            vertical-stretch: 0;
                        }
                        Button {
                            text: "Apply source settings";
                            clicked => apply-source-settings();
                        }
                        Text {
                            text: "Transform  ·  scale-x,scale-y,x,y,flip-x,flip-y,opacity";
                            color: rgb(203, 213, 225);
                            font-size: 12px;
                        }
                        LineEdit {
                            text <=> source-transform;
                            placeholder-text: "1000,1000,0,0,0,0,255";
                        }
                        Button {
                            text: "Apply transform";
                            clicked => apply-source-transform();
                        }
                        Text {
                            text: "Filters  ·  " + source-filters;
                            color: rgb(203, 213, 225);
                            font-size: 12px;
                            wrap: word-wrap;
                        }
                        LineEdit {
                            text <=> source-filters;
                            placeholder-text: "gray,brightness:750,opacity:200";
                        }
                        Button {
                            text: "Apply filters";
                            clicked => apply-source-filters();
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
                            text: "Recording file (atomic Y4M)";
                            color: rgb(148, 163, 184);
                            font-size: 12px;
                        }
                        LineEdit {
                            text <=> recording-path;
                            placeholder-text: "obs-rs-recording.y4m";
                        }
                        Text {
                            text: "Streaming address (Rust TCP packets)";
                            color: rgb(148, 163, 184);
                            font-size: 12px;
                        }
                        LineEdit {
                            text <=> streaming-address;
                            placeholder-text: "127.0.0.1:9000";
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

struct OutputRuntime {
    format: VideoFormat,
    recording: Option<AtomicY4mFileWriter>,
    streaming: Option<StreamSession<TcpPacketTransport>>,
    encoder: RleVideoEncoder,
}

impl OutputRuntime {
    fn new(format: VideoFormat) -> Self {
        Self {
            format,
            recording: None,
            streaming: None,
            encoder: RleVideoEncoder::new(format),
        }
    }

    fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        if self.recording.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "recording output is already open",
            )
            .into());
        }
        let final_path = PathBuf::from(path.trim());
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| std::io::Error::other("recording path must name a file"))?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        self.recording = Some(AtomicY4mFileWriter::new(
            final_path,
            temp_path,
            self.format,
        )?);
        Ok(())
    }

    fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        let Some(mut recording) = self.recording.take() else {
            return Err(std::io::Error::other("recording output is not open").into());
        };
        match recording.finalize() {
            Ok(bytes) => Ok(bytes),
            Err(error) => {
                self.recording = Some(recording);
                Err(error.into())
            }
        }
    }

    fn abort_recording(&mut self) {
        if let Some(mut recording) = self.recording.take() {
            let _ = recording.abort();
        }
    }

    fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        if self.streaming.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "stream output is already open",
            )
            .into());
        }
        let mut stream = StreamSession::new(
            TcpPacketTransport::new(address.trim()),
            8 * 1024 * 1024,
            PacketDropPolicy::DropNewest,
            ReconnectPolicy::new(3),
        )?;
        stream.connect()?;
        self.streaming = Some(stream);
        Ok(())
    }

    fn finish_streaming(&mut self) {
        if let Some(mut stream) = self.streaming.take() {
            let _ = stream.flush();
            stream.close();
        }
    }

    fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), Box<dyn Error>> {
        if let Some(recording) = self.recording.as_mut() {
            recording.push(frame.clone())?;
        }
        if let Some(stream) = self.streaming.as_mut() {
            let packet = self.encoder.encode(frame)?;
            stream.submit(packet)?;
            stream.flush()?;
        }
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new()?;
    ui.set_project_path("obs-rs-project.txt".into());
    ui.set_diagnostics_path("obs-rs-diagnostics.obsrdg".into());
    ui.set_recording_path("obs-rs-recording.y4m".into());
    ui.set_streaming_address("127.0.0.1:9000".into());
    ui.set_new_source_kind("test_pattern".into());
    let project = initial_project()?;
    let renderer = Rc::new(RefCell::new(PreviewRenderer::new(&project)?));
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    let output = Rc::new(RefCell::new(OutputRuntime::new(renderer.borrow().format)));

    refresh_ui(&ui, &state, &renderer);
    install_callbacks(&ui, &state, &renderer, &output);

    if smoke {
        return Ok(());
    }

    let _preview_timer = start_preview_timer(&ui, &state, &renderer, &output);
    ui.run()?;
    Ok(())
}

fn start_preview_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) -> Timer {
    let timer = Timer::default();
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let output = Rc::clone(output);
    timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        refresh_ui(&ui, &state, &renderer);
        push_program_frame(&ui, &state, &renderer, &output);
    });
    timer
}

fn install_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    install_scene_callbacks(ui, state, renderer);
    install_output_callbacks(ui, state, renderer, output);
    install_mixer_callbacks(ui, state, renderer);
    install_project_callbacks(ui, state, renderer);
}

fn install_scene_callbacks(
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

    let weak = ui.as_weak();
    let profile_state = Rc::clone(state);
    let profile_renderer = Rc::clone(renderer);
    ui.on_select_profile(move |id| {
        dispatch_and_refresh(
            &weak,
            &profile_state,
            &profile_renderer,
            UiCommand::SelectProfile { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_renderer = Rc::clone(renderer);
    ui.on_select_source(move |id| {
        dispatch_and_refresh(
            &weak,
            &source_state,
            &source_renderer,
            UiCommand::SelectSource { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let settings_state = Rc::clone(state);
    let settings_renderer = Rc::clone(renderer);
    ui.on_apply_source_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_settings().to_string();
        apply_source_settings_and_refresh(&ui, &settings_state, &settings_renderer, &document);
    });

    let weak = ui.as_weak();
    let transform_state = Rc::clone(state);
    let transform_renderer = Rc::clone(renderer);
    ui.on_apply_source_transform(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_transform().to_string();
        apply_source_transform_and_refresh(&ui, &transform_state, &transform_renderer, &document);
    });

    let weak = ui.as_weak();
    let filters_state = Rc::clone(state);
    let filters_renderer = Rc::clone(renderer);
    ui.on_apply_source_filters(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_filters().to_string();
        apply_source_filters_and_refresh(&ui, &filters_state, &filters_renderer, &document);
    });
}

fn install_output_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let weak = ui.as_weak();
    let recording_state = Rc::clone(state);
    let recording_renderer = Rc::clone(renderer);
    let recording_output = Rc::clone(output);
    ui.on_toggle_recording(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result: Result<String, Box<dyn Error>> = (|| {
            if recording_state.borrow().recording() {
                let bytes = recording_output.borrow_mut().finish_recording()?;
                recording_state
                    .borrow_mut()
                    .dispatch(UiCommand::StopRecording)?;
                Ok(format!("Recording finalized: {bytes} bytes"))
            } else {
                let path = ui.get_recording_path().to_string();
                recording_output.borrow_mut().start_recording(&path)?;
                if let Err(error) = recording_state
                    .borrow_mut()
                    .dispatch(UiCommand::StartRecording)
                {
                    recording_output.borrow_mut().abort_recording();
                    return Err(error.into());
                }
                Ok(format!("Recording started: {path}"))
            }
        })();
        match result {
            Ok(message) => {
                refresh_ui(&ui, &recording_state, &recording_renderer);
                ui.set_status_message(message.into());
            }
            Err(error) => ui.set_status_message(format!("Recording failed: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let streaming_state = Rc::clone(state);
    let streaming_renderer = Rc::clone(renderer);
    let streaming_output = Rc::clone(output);
    ui.on_toggle_streaming(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result: Result<String, Box<dyn Error>> = (|| {
            if streaming_state.borrow().streaming() {
                streaming_output.borrow_mut().finish_streaming();
                streaming_state
                    .borrow_mut()
                    .dispatch(UiCommand::StopStreaming)?;
                Ok("Streaming stopped".to_owned())
            } else {
                let address = ui.get_streaming_address().to_string();
                streaming_output.borrow_mut().start_streaming(&address)?;
                if let Err(error) = streaming_state
                    .borrow_mut()
                    .dispatch(UiCommand::StartStreaming)
                {
                    streaming_output.borrow_mut().finish_streaming();
                    return Err(error.into());
                }
                Ok(format!("Streaming connected: {address}"))
            }
        })();
        match result {
            Ok(message) => {
                refresh_ui(&ui, &streaming_state, &streaming_renderer);
                ui.set_status_message(message.into());
            }
            Err(error) => ui.set_status_message(format!("Streaming failed: {error}").into()),
        }
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
}

fn push_program_frame(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let active = {
        let state = state.borrow();
        state.recording() || state.streaming()
    };
    if !active {
        return;
    }
    let scene = state.borrow().program_scene().map(str::to_owned);
    let Some(scene) = scene else {
        return;
    };
    let result = renderer
        .borrow_mut()
        .render(&scene)
        .and_then(|frame| {
            frame.ok_or_else(|| std::io::Error::other("program scene is empty").into())
        })
        .and_then(|frame| output.borrow_mut().push_frame(&frame));
    if let Err(error) = result {
        ui.set_status_message(format!("Output failed: {error}").into());
    }
}

fn install_mixer_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let gain_state = Rc::clone(state);
    let gain_renderer = Rc::clone(renderer);
    ui.on_set_mixer_gain(move |id, gain_milli| {
        let gain_milli = u16::try_from(gain_milli.max(0)).unwrap_or(0);
        dispatch_and_refresh(
            &weak,
            &gain_state,
            &gain_renderer,
            UiCommand::SetMixerGain {
                id: id.to_string(),
                gain_milli,
            },
        );
    });

    let weak = ui.as_weak();
    let mute_state = Rc::clone(state);
    let mute_renderer = Rc::clone(renderer);
    ui.on_toggle_mixer_mute(move |id| {
        dispatch_and_refresh(
            &weak,
            &mute_state,
            &mute_renderer,
            UiCommand::ToggleMixerMute { id: id.to_string() },
        );
    });
}

fn install_project_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let save_state = Rc::clone(state);
    let save_renderer = Rc::clone(renderer);
    ui.on_save_project(move || {
        save_and_refresh(&weak, &save_state, &save_renderer);
    });

    let weak = ui.as_weak();
    let load_state = Rc::clone(state);
    let load_renderer = Rc::clone(renderer);
    ui.on_load_project(move || {
        load_and_refresh(&weak, &load_state, &load_renderer);
    });

    let weak = ui.as_weak();
    let recover_state = Rc::clone(state);
    let recover_renderer = Rc::clone(renderer);
    ui.on_recover_project(move || {
        recover_and_refresh(&weak, &recover_state, &recover_renderer);
    });

    let weak = ui.as_weak();
    let diagnostics_state = Rc::clone(state);
    let diagnostics_renderer = Rc::clone(renderer);
    ui.on_export_diagnostics(move || {
        export_diagnostics(&weak, &diagnostics_state, &diagnostics_renderer);
    });

    let weak = ui.as_weak();
    let add_state = Rc::clone(state);
    let add_renderer = Rc::clone(renderer);
    ui.on_add_scene(move |id, name| {
        add_scene_and_refresh(&weak, &add_state, &add_renderer, id.as_str(), name.as_str());
    });

    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_renderer = Rc::clone(renderer);
    ui.on_add_source(move |id, kind, name| {
        add_source_and_refresh(
            &weak,
            &source_state,
            &source_renderer,
            id.as_str(),
            kind.as_str(),
            name.as_str(),
        );
    });
}

fn project_store(path: &str) -> Result<ProjectFileStore, Box<dyn Error>> {
    let path = path.trim();
    if path.is_empty() {
        return Err(std::io::Error::other("project path is empty").into());
    }
    let final_path = PathBuf::from(path);
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("project path must name a file"))?;
    let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
    Ok(ProjectFileStore::new(final_path, temp_path)?)
}

fn save_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<usize, Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        Ok(state.borrow_mut().save_project(&store)?)
    })();
    match result {
        Ok(bytes) => {
            refresh_ui(&ui, state, renderer);
            ui.set_status_message(format!("Saved {bytes} bytes to {path}").into());
        }
        Err(error) => ui.set_status_message(format!("Save failed: {error}").into()),
    }
}

fn load_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<(), Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        state.borrow_mut().load_project(&store)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, renderer);
            ui.set_status_message(format!("Loaded project from {path}").into());
        }
        Err(error) => ui.set_status_message(format!("Load failed: {error}").into()),
    }
}

fn recover_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<bool, Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        Ok(state.borrow_mut().recover_project(&store)?)
    })();
    match result {
        Ok(true) => {
            refresh_ui(&ui, state, renderer);
            ui.set_status_message(
                format!("Recovered interrupted project for {path}; save to publish it").into(),
            );
        }
        Ok(false) => ui.set_status_message("No recoverable project was found".into()),
        Err(error) => ui.set_status_message(format!("Recovery failed: {error}").into()),
    }
}

fn export_diagnostics(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_diagnostics_path().to_string();
    let result: Result<usize, Box<dyn Error>> = (|| {
        let final_path = PathBuf::from(path.trim());
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| std::io::Error::other("diagnostics path must name a file"))?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        let state = state.borrow();
        let metrics = renderer.borrow().runtime.compositor_metrics();
        let mut bundle = DiagnosticBundle::new();
        bundle.insert_text("project", &state.project_document())?;
        bundle.insert_text("ui", &state.accessible_snapshot())?;
        bundle.insert_text(
            "runtime",
            &format!(
                "render_calls={} source_requests={} source_frames={} empty_sources={} transformed={} filtered={} blends={}",
                metrics.render_calls(),
                metrics.source_requests(),
                metrics.source_frames(),
                metrics.empty_sources(),
                metrics.transformed_frames(),
                metrics.filtered_frames(),
                metrics.blended_layers()
            ),
        )?;
        let mut writer = AtomicDiagnosticFileWriter::new(final_path, temp_path)?;
        Ok(writer.finalize(&bundle)?)
    })();
    match result {
        Ok(bytes) => ui.set_status_message(format!("Diagnostics exported: {bytes} bytes").into()),
        Err(error) => ui.set_status_message(format!("Diagnostics failed: {error}").into()),
    }
}

fn add_scene_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    id: &str,
    name: &str,
) {
    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = SceneSpec::new(id, name)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile,
                scene,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, renderer);
            ui.set_new_scene_id("".into());
            ui.set_new_scene_name("".into());
        }
        Err(error) => ui.set_status_message(format!("Add scene failed: {error}").into()),
    }
}

fn add_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    id: &str,
    kind: &str,
    name: &str,
) {
    let (profile, scene) = {
        let state = state.borrow();
        let profile = state
            .project_session()
            .project()
            .active_profile()
            .to_string();
        let scene = state
            .preview_scene()
            .map(str::to_owned)
            .ok_or_else(|| std::io::Error::other("no preview scene is selected"));
        (profile, scene)
    };
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = scene?;
        let source = SourceSpec::new(id, kind, name, source_settings(kind)?)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile,
                scene,
                source,
            }))?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, renderer);
            ui.set_new_source_id("".into());
            ui.set_new_source_name("".into());
        }
        Err(error) => ui.set_status_message(format!("Add source failed: {error}").into()),
    }
}

fn apply_source_settings_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    document: &str,
) {
    let (profile, scene, source) = {
        let state = state.borrow();
        (
            state
                .project_session()
                .project()
                .active_profile()
                .to_string(),
            state.preview_scene().map(str::to_owned),
            state.selected_source().map(str::to_owned),
        )
    };
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = scene.ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
        let source = source.ok_or_else(|| std::io::Error::other("no source is selected"))?;
        let settings = Config::parse(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceSettings {
                profile,
                scene,
                source,
                settings,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source settings failed: {error}").into());
    } else {
        refresh_ui(ui, state, renderer);
    }
}

fn apply_source_transform_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, source) = selected_source_context(&state.borrow())?;
        let transform = parse_source_transform(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceTransform {
                profile,
                scene,
                source,
                transform,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source transform failed: {error}").into());
    } else {
        refresh_ui(ui, state, renderer);
    }
}

fn apply_source_filters_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    document: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, source) = selected_source_context(&state.borrow())?;
        let filters = parse_source_filters(document)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceFilters {
                profile,
                scene,
                source,
                filters,
            }))?;
        Ok(())
    })();
    if let Err(error) = result {
        ui.set_status_message(format!("Source filters failed: {error}").into());
    } else {
        refresh_ui(ui, state, renderer);
    }
}

fn selected_source_context(
    state: &DesktopState,
) -> Result<(String, String, String), Box<dyn Error>> {
    let profile = state
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let scene = state
        .preview_scene()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
    let source = state
        .selected_source()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("no source is selected"))?;
    Ok((profile, scene, source))
}

fn parse_source_transform(document: &str) -> Result<FrameTransform, Box<dyn Error>> {
    let values = document.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 7 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "transform needs 7 comma-separated values",
        )
        .into());
    }
    let flip_x = parse_transform_flag(values[4], "flip-x")?;
    let flip_y = parse_transform_flag(values[5], "flip-y")?;
    Ok(FrameTransform::new(
        values[0].parse()?,
        values[1].parse()?,
        values[2].parse()?,
        values[3].parse()?,
        flip_x,
        flip_y,
        values[6].parse()?,
    )?)
}

fn parse_transform_flag(value: &str, field: &str) -> Result<bool, Box<dyn Error>> {
    match value {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{field} must be 0, 1, false, or true"),
        )
        .into()),
    }
}

fn parse_source_filters(document: &str) -> Result<Vec<FrameFilter>, Box<dyn Error>> {
    let document = document.trim();
    if document.is_empty() {
        return Ok(Vec::new());
    }
    document
        .split(',')
        .map(str::trim)
        .map(|filter| {
            if filter == "gray" || filter == "grayscale" {
                return Ok(FrameFilter::Grayscale);
            }
            if let Some(value) = filter.strip_prefix("brightness:") {
                return Ok(FrameFilter::Brightness {
                    milli: value.trim().parse()?,
                });
            }
            if let Some(value) = filter.strip_prefix("opacity:") {
                return Ok(FrameFilter::Opacity(value.trim().parse()?));
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown filter: {filter}"),
            )
            .into())
        })
        .collect()
}

fn source_transform_document(transform: FrameTransform) -> String {
    format!(
        "{},{},{},{},{},{},{}",
        transform.scale_x_milli(),
        transform.scale_y_milli(),
        transform.translate_x(),
        transform.translate_y(),
        u8::from(transform.flip_x()),
        u8::from(transform.flip_y()),
        transform.opacity()
    )
}

fn source_filters_document(filters: &[FrameFilter]) -> String {
    filters
        .iter()
        .map(|filter| match filter {
            FrameFilter::Grayscale => "gray".to_owned(),
            FrameFilter::Brightness { milli } => format!("brightness:{milli}"),
            FrameFilter::Opacity(opacity) => format!("opacity:{opacity}"),
        })
        .collect::<Vec<_>>()
        .join(",")
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

    let profile_rows = project
        .profiles()
        .map(|profile| ProfileRow {
            id: profile.id().as_str().into(),
            name: profile.name().into(),
        })
        .collect::<Vec<_>>();
    ui.set_profile_rows(ModelRc::new(VecModel::from(profile_rows)));

    let sync_error = renderer.borrow_mut().sync_project(project).err();
    let (preview_image, preview_error) = match sync_error.as_ref() {
        Some(error) => (Image::default(), Some(format!("Preview renderer: {error}"))),
        None => scene_image(renderer, state.preview_scene()),
    };
    let (program_image, program_error) = match sync_error.as_ref() {
        Some(error) => (Image::default(), Some(format!("Preview renderer: {error}"))),
        None => scene_image(renderer, state.program_scene()),
    };
    ui.set_preview_image(preview_image);
    ui.set_program_image(program_image);
    let render_error = preview_error.or(program_error);
    ui.set_status_message(
        render_error
            .unwrap_or_else(|| latest_notice(&state).to_owned())
            .into(),
    );

    refresh_docks(ui, &state, profile);
}

fn refresh_docks(ui: &MainWindow, state: &DesktopState, profile: Option<&Profile>) {
    let scene_rows = profile.map_or_else(Vec::new, |profile| {
        profile
            .scenes()
            .map(|scene| {
                let id = scene.id().to_string();
                let role = scene_role(state, &id);
                SceneRow {
                    id: id.into(),
                    name: scene.name().into(),
                    role: role.into(),
                }
            })
            .collect::<Vec<_>>()
    });
    ui.set_scene_rows(ModelRc::new(VecModel::from(scene_rows)));

    let source_scene = state.preview_scene().unwrap_or("none");
    let selected_source = state.selected_source().unwrap_or("none");
    let source_rows = profile
        .and_then(|profile| {
            profile
                .scenes()
                .find(|scene| scene.id().as_str() == source_scene)
        })
        .map_or_else(Vec::new, |scene| {
            scene
                .sources()
                .iter()
                .enumerate()
                .map(|(index, source)| SourceRow {
                    id: source.id().as_str().into(),
                    name: source.name().into(),
                    kind: source.kind().as_str().into(),
                    order: (index + 1).to_string().into(),
                    selected: source.id().as_str() == selected_source,
                })
                .collect::<Vec<_>>()
        });
    ui.set_source_scene(source_scene.into());
    ui.set_source_rows(ModelRc::new(VecModel::from(source_rows)));
    let selected_source_spec = profile
        .and_then(|profile| {
            profile
                .scenes()
                .find(|scene| scene.id().as_str() == source_scene)
        })
        .and_then(|scene| {
            scene
                .sources()
                .iter()
                .find(|source| source.id().as_str() == selected_source)
        });
    let selected_settings =
        selected_source_spec.map_or_else(String::new, |source| source.settings().serialize());
    if ui.get_source_settings_version().as_str() != selected_settings {
        ui.set_source_settings(selected_settings.into());
        ui.set_source_settings_version(ui.get_source_settings().clone());
    }
    let selected_transform =
        selected_source_spec.map_or(FrameTransform::IDENTITY, SourceSpec::transform);
    let transform_document = source_transform_document(selected_transform);
    let filters_document = selected_source_spec.map_or_else(String::new, |source| {
        source_filters_document(source.filters())
    });
    let properties_version = format!("{transform_document}\u{1f}{filters_document}");
    if ui.get_source_properties_version().as_str() != properties_version {
        ui.set_source_transform(transform_document.into());
        ui.set_source_filters(filters_document.into());
        ui.set_source_properties_version(properties_version.into());
    }
    ui.set_selected_source(selected_source.into());

    let mixer_rows = state
        .mixer_channels()
        .map(|channel| MixerRow {
            id: channel.id().into(),
            name: channel.name().into(),
            gain: f32::from(channel.gain_milli()) / 1_000.0,
            peak: f32::from(channel.peak_milli()) / 1_000.0,
            muted: channel.muted(),
        })
        .collect::<Vec<_>>();
    ui.set_mixer_rows(ModelRc::new(VecModel::from(mixer_rows)));
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
    timestamp: Timestamp,
    project_document: String,
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

        Ok(Self {
            format,
            runtime,
            timestamp: Timestamp::ZERO,
            project_document: project.serialize(),
        })
    }

    fn sync_project(&mut self, project: &Project) -> Result<(), Box<dyn Error>> {
        let document = project.serialize();
        if document != self.project_document {
            *self = Self::new(project)?;
        }
        Ok(())
    }

    fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let frame = self.runtime.render_scene(scene, &request)?;
        let period = self
            .format
            .frame_rate()
            .period_nanos()
            .unwrap_or(33_333_333);
        self.timestamp = self
            .timestamp
            .checked_add(period)
            .unwrap_or(Timestamp::ZERO);
        Ok(frame)
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
    let mut intermission = scene("intermission", "Intermission", "#302040FF")?;
    intermission.add_source(SourceSpec::new(
        "pattern",
        "test_pattern",
        "Animated pattern",
        video_settings(),
    )?)?;
    profile.add_scene(intermission)?;
    project.add_profile(profile)?;
    Ok(project)
}

fn video_settings() -> Config {
    let mut settings = Config::new();
    settings
        .set("width", "640")
        .expect("static width setting is valid");
    settings
        .set("height", "360")
        .expect("static height setting is valid");
    settings
}

fn source_settings(kind: &str) -> Result<Config, Box<dyn Error>> {
    let mut settings = video_settings();
    if kind.trim() == "color_source" {
        settings.set("color", "#405070FF")?;
    }
    Ok(settings)
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
    use super::{initial_project, transition_label, OutputRuntime, PreviewRenderer};
    use obs_rs_media::{FrameRate, FrameTransition, Timestamp, VideoFormat, VideoFrame};
    use obs_rs_project::{ProjectCommand, SceneSpec};

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
}
