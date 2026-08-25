use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_engine::OutputLifecycle;
use obs_rs_media::{
    parse_rgba8_hex, FrameTransition, LumaWipePattern, RawVideoFrame, SlideDirection,
    TransitionKind, TransitionSpec, VideoFrame, MAX_LUMA_WIPE_SOFTNESS_MILLI,
};
use obs_rs_ui::{
    DesktopState, UiCommand, DEFAULT_TRANSITION_DURATION_MILLIS, MAX_TRANSITION_DURATION_MILLIS,
    MIN_TRANSITION_DURATION_MILLIS,
};
use slint::{ComponentHandle, Weak};

use crate::StingerLoadController;
use crate::{
    dispatch_and_refresh, refresh_output_ui, refresh_ui, MainWindow, OutputRuntime, PreviewSurface,
};

pub(crate) fn install_output_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    install_recording_callback(ui, state, surface, output);
    install_streaming_callback(ui, state, surface, output);
    install_replay_callbacks(ui, output);
    install_remux_recovery_callback(ui, output);
    install_transition_callbacks(ui, state, surface);
}

fn install_recording_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let weak = ui.as_weak();
    let recording_state = Rc::clone(state);
    let recording_surface = Rc::clone(surface);
    let recording_output = Rc::clone(output);
    ui.on_toggle_recording(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result: Result<String, Box<dyn Error>> = (|| {
            if recording_state.borrow().recording() {
                recording_output.borrow_mut().request_finish_recording()?;
                Ok("Recording stop requested".to_owned())
            } else {
                let path = ui.get_recording_path().to_string();
                recording_output
                    .borrow_mut()
                    .request_start_recording(&path)?;
                if let Err(error) = recording_state
                    .borrow_mut()
                    .dispatch(UiCommand::StartRecording)
                {
                    recording_output.borrow_mut().abort_recording();
                    return Err(error.into());
                }
                Ok(format!("Recording start requested: {path}"))
            }
        })();
        match result {
            Ok(message) => {
                refresh_ui(&ui, &recording_state, &recording_surface);
                ui.set_status_message(message.into());
            }
            Err(error) => ui.set_status_message(format!("Recording failed: {error}").into()),
        }
        refresh_output_ui(&ui, &recording_output);
    });
}

fn install_streaming_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let weak = ui.as_weak();
    let streaming_state = Rc::clone(state);
    let streaming_surface = Rc::clone(surface);
    let streaming_output = Rc::clone(output);
    ui.on_toggle_streaming(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result: Result<String, Box<dyn Error>> = (|| {
            let lifecycle = streaming_output.borrow().lifecycles().1;
            if matches!(
                lifecycle,
                OutputLifecycle::Starting | OutputLifecycle::Running | OutputLifecycle::Stopping
            ) {
                streaming_output.borrow_mut().finish_streaming()?;
                Ok("Streaming stop requested".to_owned())
            } else {
                let protocol = streaming_output.borrow_mut().start_configured_stream()?;
                Ok(format!("{protocol} streaming starting"))
            }
        })();
        match result {
            Ok(message) => {
                refresh_ui(&ui, &streaming_state, &streaming_surface);
                ui.set_status_message(message.into());
            }
            Err(error) => ui.set_status_message(format!("Streaming failed: {error}").into()),
        }
        refresh_output_ui(&ui, &streaming_output);
    });
}

fn install_replay_callbacks(ui: &MainWindow, output: &Rc<RefCell<OutputRuntime>>) {
    let weak = ui.as_weak();
    let replay_output = Rc::clone(output);
    ui.on_toggle_replay_buffer(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let buffering = replay_output.borrow().replay_controls().0;
        let result = if buffering {
            replay_output
                .borrow_mut()
                .request_stop_replay_buffer()
                .map(|()| "Replay buffer stop requested".to_owned())
        } else {
            let configuration = replay_output.borrow().replay_configuration_label();
            replay_output
                .borrow_mut()
                .request_start_replay_buffer()
                .map(|()| format!("Replay buffer start requested ({configuration})"))
        };
        match result {
            Ok(message) => ui.set_status_message(message.into()),
            Err(error) => ui.set_status_message(format!("Replay buffer failed: {error}").into()),
        }
        refresh_output_ui(&ui, &replay_output);
    });

    let weak = ui.as_weak();
    let replay_output = Rc::clone(output);
    ui.on_save_replay_buffer(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let (buffering, saving) = replay_output.borrow().replay_controls();
        let result: Result<String, Box<dyn Error>> = if !buffering {
            Err("Replay buffer is not running".into())
        } else if saving {
            Err("Replay save is already in progress".into())
        } else {
            let recording_path = ui.get_recording_path();
            replay_output
                .borrow_mut()
                .request_save_replay_buffer(recording_path.as_ref())
        };
        match result {
            Ok(path) => ui.set_status_message(format!("Replay save requested: {path}").into()),
            Err(error) => ui.set_status_message(format!("Replay save failed: {error}").into()),
        }
        refresh_output_ui(&ui, &replay_output);
    });
}

fn install_remux_recovery_callback(ui: &MainWindow, output: &Rc<RefCell<OutputRuntime>>) {
    let weak = ui.as_weak();
    let recovery_output = Rc::clone(output);
    ui.on_recover_recording(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let path = ui.get_recording_path().to_string();
        match recovery_output
            .borrow_mut()
            .request_discover_interrupted_remux_candidates(&path)
        {
            Ok(()) => ui
                .set_status_message("Scanning recording folder for interrupted recordings…".into()),
            Err(error) => {
                ui.set_status_message(format!("Recording recovery scan failed: {error}").into());
            }
        }
        refresh_output_ui(&ui, &recovery_output);
    });

    let weak = ui.as_weak();
    let recovery_output = Rc::clone(output);
    ui.on_recover_remux_candidate(move |path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match recovery_output
            .borrow_mut()
            .request_recover_interrupted_remux(path.as_str())
        {
            Ok(()) => ui.set_status_message(format!("Recording recovery requested: {path}").into()),
            Err(error) => {
                ui.set_status_message(format!("Recording recovery failed: {error}").into());
            }
        }
        refresh_output_ui(&ui, &recovery_output);
    });
}

fn install_transition_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let cut_state = Rc::clone(state);
    let cut_surface = Rc::clone(surface);
    ui.on_cut_transition(move || {
        take_transition_and_refresh(
            &weak,
            &cut_state,
            &cut_surface,
            FrameTransition::Cut,
            DEFAULT_TRANSITION_DURATION_MILLIS,
        );
    });

    let weak = ui.as_weak();
    let fade_state = Rc::clone(state);
    let fade_surface = Rc::clone(surface);
    ui.on_fade_transition(move || {
        take_transition_and_refresh(
            &weak,
            &fade_state,
            &fade_surface,
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
            DEFAULT_TRANSITION_DURATION_MILLIS,
        );
    });

    let weak = ui.as_weak();
    let duration_state = Rc::clone(state);
    let duration_surface = Rc::clone(surface);
    ui.on_fade_transition_duration(move |duration| {
        let duration = match duration.trim().parse::<u32>() {
            Ok(duration)
                if (MIN_TRANSITION_DURATION_MILLIS..=MAX_TRANSITION_DURATION_MILLIS)
                    .contains(&duration) =>
            {
                duration
            }
            _ => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status_message("Transition duration must be 1–60000 ms".into());
                }
                return;
            }
        };
        take_transition_and_refresh(
            &weak,
            &duration_state,
            &duration_surface,
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
            duration,
        );
    });

    install_slide_transition_callback(ui, state, surface);
    install_swipe_transition_callback(ui, state, surface);
    install_luma_transition_callback(ui, state, surface);

    let weak = ui.as_weak();
    let color_state = Rc::clone(state);
    let color_surface = Rc::clone(surface);
    ui.on_fade_to_color(move |color, duration| {
        let duration = match duration.trim().parse::<u32>() {
            Ok(duration)
                if (MIN_TRANSITION_DURATION_MILLIS..=MAX_TRANSITION_DURATION_MILLIS)
                    .contains(&duration) =>
            {
                duration
            }
            _ => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status_message("Transition duration must be 1–60000 ms".into());
                }
                return;
            }
        };
        let Some(color) = parse_rgba8_hex(color.trim()) else {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message("Transition color must be #RRGGBB or #RRGGBBAA".into());
            }
            return;
        };
        let transition = match FrameTransition::fade_to_color(500, color) {
            Ok(transition) => transition,
            Err(error) => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status_message(format!("Transition failed: {error}").into());
                }
                return;
            }
        };
        take_transition_and_refresh(&weak, &color_state, &color_surface, transition, duration);
    });

    install_scene_transition_override_callbacks(ui, state, surface);
}

/// Installs the explicit Stinger Take action on the shared `MainWindow`.
///
/// The callback only clones a worker-published `Arc<StingerClip>` and dispatches
/// a bounded UI command. Resource loading remains owned by the preview timer
/// and its dedicated worker, so a button press never performs file or decoder
/// I/O on the UI thread.
pub(crate) fn install_stinger_take_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    stinger_loader: &Rc<RefCell<StingerLoadController>>,
) {
    let weak = ui.as_weak();
    let take_state = Rc::clone(state);
    let take_surface = Rc::clone(surface);
    let take_loader = Rc::clone(stinger_loader);
    ui.on_take_stinger(move |duration_value| {
        let Some(duration_ms) = parse_transition_duration(&weak, duration_value.as_str()) else {
            return;
        };
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let ready_result = {
            let loader = take_loader.borrow();
            loader.ready_clip()
        };
        let clip = match ready_result {
            Ok(clip) => clip,
            Err(crate::stinger_loader::StingerTakeError::NotReady) => {
                let request_result = {
                    let state = take_state.borrow();
                    take_loader
                        .borrow_mut()
                        .request_on_demand(state.project_session().project(), state.preview_scene())
                };
                match request_result {
                    Ok(_) => ui.set_status_message(
                        "Stinger is loading; take it again when the resource is ready".into(),
                    ),
                    Err(error) => {
                        ui.set_status_message(format!("Stinger Take failed: {error}").into());
                    }
                }
                return;
            }
            Err(error) => {
                ui.set_status_message(format!("Stinger Take failed: {error}").into());
                return;
            }
        };
        let result = take_state
            .borrow_mut()
            .dispatch(UiCommand::TakeStinger { clip, duration_ms });
        match result {
            Ok(()) => {
                refresh_ui(&ui, &take_state, &take_surface);
                ui.set_status_message("Stinger Take sent to Program".into());
            }
            Err(error) => ui.set_status_message(format!("Stinger Take failed: {error}").into()),
        }
    });
}

fn install_luma_transition_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let luma_state = Rc::clone(state);
    let luma_surface = Rc::clone(surface);
    ui.on_luma_transition(move |duration, pattern_index, invert, softness| {
        apply_luma_transition(
            &weak,
            &luma_state,
            &luma_surface,
            duration.as_str(),
            pattern_index,
            invert,
            softness.as_str(),
        );
    });
}

fn apply_luma_transition(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    duration_value: &str,
    pattern_index: i32,
    invert: bool,
    softness_value: &str,
) {
    let Some(duration) = parse_transition_duration(weak, duration_value) else {
        return;
    };
    let pattern = match luma_pattern_from_index(pattern_index) {
        Ok(pattern) => pattern,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(error.into());
            }
            return;
        }
    };
    let softness_milli = match parse_luma_softness(softness_value) {
        Ok(softness) => softness,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(error.into());
            }
            return;
        }
    };
    if let Some(ui) = weak.upgrade() {
        ui.set_transition_kind("luma_wipe".into());
        ui.set_luma_pattern_index(pattern_index);
        ui.set_luma_invert(invert);
        ui.set_luma_softness(softness_value.trim().into());
    }
    let transition = match FrameTransition::luma_wipe(500, pattern, invert, softness_milli) {
        Ok(transition) => transition,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(format!("Transition failed: {error}").into());
            }
            return;
        }
    };
    take_transition_and_refresh(weak, state, surface, transition, duration);
}

fn install_slide_transition_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let slide_state = Rc::clone(state);
    let slide_surface = Rc::clone(surface);
    ui.on_slide_transition(move |duration| {
        apply_slide_transition(&weak, &slide_state, &slide_surface, duration.as_str(), 0);
    });

    let weak = ui.as_weak();
    let slide_state = Rc::clone(state);
    let slide_surface = Rc::clone(surface);
    ui.on_slide_transition_direction(move |duration, direction_index| {
        apply_slide_transition(
            &weak,
            &slide_state,
            &slide_surface,
            duration.as_str(),
            direction_index,
        );
    });
}

fn install_swipe_transition_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let swipe_state = Rc::clone(state);
    let swipe_surface = Rc::clone(surface);
    ui.on_swipe_transition(move |duration| {
        apply_swipe_transition(
            &weak,
            &swipe_state,
            &swipe_surface,
            duration.as_str(),
            0,
            false,
        );
    });

    let weak = ui.as_weak();
    let swipe_state = Rc::clone(state);
    let swipe_surface = Rc::clone(surface);
    ui.on_swipe_transition_direction(move |duration, direction_index| {
        apply_swipe_transition(
            &weak,
            &swipe_state,
            &swipe_surface,
            duration.as_str(),
            direction_index,
            false,
        );
    });

    let weak = ui.as_weak();
    let swipe_state = Rc::clone(state);
    let swipe_surface = Rc::clone(surface);
    ui.on_swipe_transition_direction_mode(move |duration, direction_index, swipe_in| {
        apply_swipe_transition(
            &weak,
            &swipe_state,
            &swipe_surface,
            duration.as_str(),
            direction_index,
            swipe_in,
        );
    });
}

fn apply_slide_transition(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    duration_value: &str,
    direction_index: i32,
) {
    let Some(duration) = parse_transition_duration(weak, duration_value) else {
        return;
    };
    let direction = match slide_direction_from_index(direction_index) {
        Ok(direction) => direction,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(error.into());
            }
            return;
        }
    };
    if let Some(ui) = weak.upgrade() {
        ui.set_transition_direction_index(direction_index);
    }
    let transition = match FrameTransition::slide(500, direction) {
        Ok(transition) => transition,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(format!("Transition failed: {error}").into());
            }
            return;
        }
    };
    take_transition_and_refresh(weak, state, surface, transition, duration);
}

fn apply_swipe_transition(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    duration_value: &str,
    direction_index: i32,
    swipe_in: bool,
) {
    let Some(duration) = parse_transition_duration(weak, duration_value) else {
        return;
    };
    let direction = match slide_direction_from_index(direction_index) {
        Ok(direction) => direction,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(error.into());
            }
            return;
        }
    };
    if let Some(ui) = weak.upgrade() {
        ui.set_transition_direction_index(direction_index);
        ui.set_swipe_in(swipe_in);
    }
    let transition = match FrameTransition::swipe_with_mode(500, direction, swipe_in) {
        Ok(transition) => transition,
        Err(error) => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(format!("Transition failed: {error}").into());
            }
            return;
        }
    };
    take_transition_and_refresh(weak, state, surface, transition, duration);
}

fn parse_transition_duration(weak: &Weak<MainWindow>, value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(duration)
            if (MIN_TRANSITION_DURATION_MILLIS..=MAX_TRANSITION_DURATION_MILLIS)
                .contains(&duration) =>
        {
            Some(duration)
        }
        _ => {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message("Transition duration must be 1–60000 ms".into());
            }
            None
        }
    }
}

fn install_scene_transition_override_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let override_state = Rc::clone(state);
    let override_surface = Rc::clone(surface);
    ui.on_set_scene_transition(move |kind, duration, color| {
        let result = set_scene_transition_override(
            &override_state,
            kind.as_str(),
            duration.as_str(),
            color.as_str(),
            0,
            false,
        );
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(()) => refresh_ui(&ui, &override_state, &override_surface),
            Err(error) => ui.set_status_message(error.into()),
        }
    });

    let weak = ui.as_weak();
    let override_state = Rc::clone(state);
    let override_surface = Rc::clone(surface);
    ui.on_set_scene_transition_direction(move |kind, duration, color, direction_index| {
        let result = set_scene_transition_override(
            &override_state,
            kind.as_str(),
            duration.as_str(),
            color.as_str(),
            direction_index,
            false,
        );
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(()) => refresh_ui(&ui, &override_state, &override_surface),
            Err(error) => ui.set_status_message(error.into()),
        }
    });

    let weak = ui.as_weak();
    let override_state = Rc::clone(state);
    let override_surface = Rc::clone(surface);
    ui.on_set_scene_transition_direction_mode(
        move |kind, duration, color, direction_index, swipe_in| {
            let result = set_scene_transition_override(
                &override_state,
                kind.as_str(),
                duration.as_str(),
                color.as_str(),
                direction_index,
                swipe_in,
            );
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(()) => refresh_ui(&ui, &override_state, &override_surface),
                Err(error) => ui.set_status_message(error.into()),
            }
        },
    );

    let weak = ui.as_weak();
    let override_state = Rc::clone(state);
    let override_surface = Rc::clone(surface);
    ui.on_set_scene_transition_luma(move |kind, duration, pattern, invert, softness| {
        let result = set_scene_transition_luma_override(
            &override_state,
            kind.as_str(),
            duration.as_str(),
            pattern,
            invert,
            softness.as_str(),
        );
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(()) => refresh_ui(&ui, &override_state, &override_surface),
            Err(error) => ui.set_status_message(error.into()),
        }
    });

    let weak = ui.as_weak();
    let clear_state = Rc::clone(state);
    let clear_surface = Rc::clone(surface);
    ui.on_clear_scene_transition(move || {
        let result = clear_state
            .borrow_mut()
            .dispatch(UiCommand::SetPreviewSceneTransition { transition: None });
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(()) => refresh_ui(&ui, &clear_state, &clear_surface),
            Err(error) => {
                ui.set_status_message(format!("Transition override failed: {error}").into());
            }
        }
    });
}

fn set_scene_transition_override(
    state: &Rc<RefCell<DesktopState>>,
    kind: &str,
    duration: &str,
    color: &str,
    direction_index: i32,
    swipe_in: bool,
) -> Result<(), String> {
    let transition = scene_transition_spec(&SceneTransitionInput {
        kind,
        duration,
        color,
        direction_index,
        swipe_in,
        luma_pattern_index: 0,
        luma_invert: false,
        luma_softness: "30",
    })?;
    state
        .borrow_mut()
        .dispatch(UiCommand::SetPreviewSceneTransition {
            transition: Some(transition),
        })
        .map_err(|error| format!("Transition override failed: {error}"))
}

fn set_scene_transition_luma_override(
    state: &Rc<RefCell<DesktopState>>,
    kind: &str,
    duration: &str,
    pattern_index: i32,
    invert: bool,
    softness: &str,
) -> Result<(), String> {
    let transition = scene_transition_spec(&SceneTransitionInput {
        kind,
        duration,
        color: "#000000FF",
        direction_index: 0,
        swipe_in: false,
        luma_pattern_index: pattern_index,
        luma_invert: invert,
        luma_softness: softness,
    })?;
    state
        .borrow_mut()
        .dispatch(UiCommand::SetPreviewSceneTransition {
            transition: Some(transition),
        })
        .map_err(|error| format!("Transition override failed: {error}"))
}

pub(crate) struct SceneTransitionInput<'a> {
    pub(crate) kind: &'a str,
    pub(crate) duration: &'a str,
    pub(crate) color: &'a str,
    pub(crate) direction_index: i32,
    pub(crate) swipe_in: bool,
    pub(crate) luma_pattern_index: i32,
    pub(crate) luma_invert: bool,
    pub(crate) luma_softness: &'a str,
}

pub(crate) fn scene_transition_spec(
    input: &SceneTransitionInput<'_>,
) -> Result<TransitionSpec, String> {
    let duration = input
        .duration
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|duration| {
            (MIN_TRANSITION_DURATION_MILLIS..=MAX_TRANSITION_DURATION_MILLIS).contains(duration)
        })
        .ok_or_else(|| "Transition duration must be 1–60000 ms".to_owned())?;
    match input.kind {
        "cut" => TransitionSpec::new(TransitionKind::Cut, duration),
        "cross_fade" => TransitionSpec::new(TransitionKind::CrossFade, duration),
        "fade_to_color" => {
            let color = parse_rgba8_hex(input.color.trim())
                .ok_or_else(|| "Transition color must be #RRGGBB or #RRGGBBAA".to_owned())?;
            TransitionSpec::new(TransitionKind::FadeToColor { color }, duration)
        }
        "slide" => TransitionSpec::new(
            TransitionKind::Slide {
                direction: slide_direction_from_index(input.direction_index)?,
            },
            duration,
        ),
        "swipe" => TransitionSpec::new(
            TransitionKind::Swipe {
                direction: slide_direction_from_index(input.direction_index)?,
                swipe_in: input.swipe_in,
            },
            duration,
        ),
        "luma_wipe" => {
            let pattern = luma_pattern_from_index(input.luma_pattern_index)?;
            let softness_milli = parse_luma_softness(input.luma_softness)?;
            TransitionSpec::new(
                TransitionKind::LumaWipe {
                    pattern,
                    invert: input.luma_invert,
                    softness_milli,
                },
                duration,
            )
        }
        _ => return Err("Transition override failed: unknown transition kind".to_owned()),
    }
    .map_err(|error| format!("Transition override failed: {error}"))
}

pub(crate) fn luma_pattern_from_index(index: i32) -> Result<LumaWipePattern, String> {
    match index {
        0 => Ok(LumaWipePattern::LinearHorizontal),
        1 => Ok(LumaWipePattern::LinearVertical),
        _ => Err("Luma Wipe pattern selection is invalid".to_owned()),
    }
}

pub(crate) fn parse_luma_softness(value: &str) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|softness| *softness <= MAX_LUMA_WIPE_SOFTNESS_MILLI)
        .ok_or_else(|| "Luma Wipe softness must be 0–1000‰".to_owned())
}

pub(crate) fn slide_direction_from_index(index: i32) -> Result<SlideDirection, String> {
    match index {
        0 => Ok(SlideDirection::Left),
        1 => Ok(SlideDirection::Right),
        2 => Ok(SlideDirection::Up),
        3 => Ok(SlideDirection::Down),
        _ => Err("Transition direction selection is invalid".to_owned()),
    }
}

pub(crate) fn take_transition_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    transition: FrameTransition,
    duration_ms: u32,
) {
    let target = state.borrow().preview_scene().map(str::to_owned);
    if target.is_none() {
        if let Some(ui) = weak.upgrade() {
            ui.set_status_message("Transition failed: no preview scene is selected".into());
        }
        return;
    }
    let result: Result<(), Box<dyn Error>> = (|| {
        state.borrow_mut().dispatch(UiCommand::TakePreview {
            transition,
            duration_ms,
        })?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, surface);
        }
        Err(error) => ui.set_status_message(format!("Transition failed: {error}").into()),
    }
}

pub(crate) fn push_program_frame(
    ui: &MainWindow,
    preview_frame: Option<&VideoFrame>,
    raw_frame: Option<RawVideoFrame>,
    canvas_frame: Option<VideoFrame>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    // An accelerated frame is only usable while the encoders run at the canvas
    // geometry: packed and planar layouts are not resampled, so a scaled output
    // takes the full-canvas RGBA path instead of dropping the bounded GUI view.
    let accepts_raw = output.borrow().accepts_raw_frames();
    if let Some(frame) = raw_frame.filter(|_| accepts_raw) {
        output.borrow_mut().push_raw_frame(frame);
    } else if let Some(frame) = canvas_frame {
        output.borrow_mut().push_frame(&frame);
    } else {
        let message = if preview_frame.is_some() {
            "Output skipped: full-canvas program frame unavailable"
        } else {
            "Output skipped: program scene is empty"
        };
        ui.set_status_message(message.into());
    }
}

pub(crate) fn install_mixer_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let weak = ui.as_weak();
    let gain_state = Rc::clone(state);
    let gain_surface = Rc::clone(surface);
    let gain_output = Rc::clone(output);
    ui.on_set_mixer_gain(move |id, gain_milli| {
        let gain_milli = u16::try_from(gain_milli.max(0)).unwrap_or(0);
        dispatch_and_refresh(
            &weak,
            &gain_state,
            &gain_surface,
            UiCommand::SetMixerGain {
                id: id.to_string(),
                gain_milli,
            },
        );
        if let Err(error) = gain_output
            .borrow_mut()
            .set_channel_gain_milli(id.as_str(), gain_milli)
        {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(format!("Audio channel failed: {error}").into());
            }
        }
    });

    let weak = ui.as_weak();
    let pan_state = Rc::clone(state);
    let pan_surface = Rc::clone(surface);
    let pan_output = Rc::clone(output);
    ui.on_set_mixer_pan(move |id, pan_milli| {
        dispatch_and_refresh(
            &weak,
            &pan_state,
            &pan_surface,
            UiCommand::SetMixerPan {
                id: id.to_string(),
                pan_milli,
            },
        );
        if let Err(error) = pan_output
            .borrow_mut()
            .set_channel_pan_milli(id.as_str(), pan_milli)
        {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(format!("Audio channel failed: {error}").into());
            }
        }
    });

    let weak = ui.as_weak();
    let mute_state = Rc::clone(state);
    let mute_surface = Rc::clone(surface);
    let mute_output = Rc::clone(output);
    ui.on_toggle_mixer_mute(move |id| {
        dispatch_and_refresh(
            &weak,
            &mute_state,
            &mute_surface,
            UiCommand::ToggleMixerMute { id: id.to_string() },
        );
        let muted = mute_state
            .borrow()
            .mixer_channels()
            .find(|channel| channel.id() == id.as_str())
            .is_some_and(obs_rs_ui::MixerChannel::muted);
        if let Err(error) = mute_output
            .borrow_mut()
            .set_channel_muted(id.as_str(), muted)
        {
            if let Some(ui) = weak.upgrade() {
                ui.set_status_message(format!("Audio channel failed: {error}").into());
            }
        }
    });
}
