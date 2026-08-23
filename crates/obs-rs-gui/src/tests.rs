use super::dock_tree::DockNode;
use super::i18n::catalog;
use super::output::stream_protocol_label;
use super::preview::PreviewRenderer;
use super::refresh::{peak_db, transition_label_for_locale};
use super::settings::{AppSettings, ProjectorGeometry, ProjectorKind, ProjectorTarget};
use super::settings_model::RecordingQuality;
use super::{
    capture_devices, close_request_response, initial_project, install_canvas_callbacks,
    kind_selects_monitor, refresh_ui, restore_project, source_settings, I18n, MainWindow,
    OutputRuntime, PreviewSurface, SettingsWindow, SourcePropertiesWindow,
};
use i_slint_backend_testing::ElementHandle;
use obs_rs_media::{
    FrameRate, FrameTransform, FrameTransition, ScaleFilter, Timestamp, VideoFormat, VideoFrame,
};
use obs_rs_output::{encode_png, MemoryMuxer, OutputProfileKind, PacketKind, RTMP_SERVICE_PRESETS};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{ProjectCommand, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, ProjectSceneSelection, Shortcut, UiAction, UiCommand, UiLocale};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{CloseRequestResponse, ComponentHandle, LogicalPosition, Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

#[path = "tests/output.rs"]
mod output;
#[path = "tests/runtime.rs"]
mod runtime;
#[path = "tests/scene.rs"]
mod scene;
#[path = "tests/ui.rs"]
mod ui;
#[path = "tests/ui_layout.rs"]
mod ui_layout;
#[path = "tests/ui_navigation.rs"]
mod ui_navigation;
#[path = "tests/ui_output.rs"]
mod ui_output;
#[path = "tests/ui_sources.rs"]
mod ui_sources;

pub(super) fn canvas_fixture() -> (Rc<RefCell<DesktopState>>, Rc<RefCell<OutputRuntime>>) {
    let project = initial_project().expect("initial project");
    let format = project
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    (state, Rc::new(RefCell::new(OutputRuntime::new(format))))
}

pub(super) fn profile_video_format(state: &Rc<RefCell<DesktopState>>) -> VideoFormat {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .expect("profile")
        .video_format()
}

pub(super) fn scene_source_count(state: &Rc<RefCell<DesktopState>>, scene: &str) -> usize {
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
