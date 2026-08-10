use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_media::{FrameTransition, VideoFrame};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, Weak};

use crate::{
    dispatch_and_refresh, frame_to_image, refresh_output_ui, refresh_ui, MainWindow, OutputRuntime,
    PreviewRenderer,
};

pub(crate) fn install_output_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    install_recording_callback(ui, state, renderer, output);
    install_streaming_callback(ui, state, renderer, output);
    install_transition_callbacks(ui, state, renderer);
}

fn install_recording_callback(
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
        refresh_output_ui(&ui, &recording_output);
    });
}

fn install_streaming_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
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
                // "Automatically record when streaming" only ever starts a
                // recording; stopping the stream leaves it to the user.
                if ui.get_auto_record_when_streaming()
                    && streaming_state.borrow().streaming()
                    && !streaming_state.borrow().recording()
                {
                    ui.invoke_toggle_recording();
                }
            }
            Err(error) => ui.set_status_message(format!("Streaming failed: {error}").into()),
        }
        refresh_output_ui(&ui, &streaming_output);
    });
}

fn install_transition_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let cut_state = Rc::clone(state);
    let cut_renderer = Rc::clone(renderer);
    ui.on_cut_transition(move || {
        take_transition_and_refresh(&weak, &cut_state, &cut_renderer, FrameTransition::Cut);
    });

    let weak = ui.as_weak();
    let fade_state = Rc::clone(state);
    let fade_renderer = Rc::clone(renderer);
    ui.on_fade_transition(move || {
        take_transition_and_refresh(
            &weak,
            &fade_state,
            &fade_renderer,
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
        );
    });
}

pub(crate) fn take_transition_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    transition: FrameTransition,
) {
    let target = state.borrow().preview_scene().map(str::to_owned);
    let Some(target) = target else {
        if let Some(ui) = weak.upgrade() {
            ui.set_status_message("Transition failed: no preview scene is selected".into());
        }
        return;
    };
    let source = state.borrow().program_scene().map(str::to_owned);
    let result: Result<(), Box<dyn Error>> = (|| {
        state
            .borrow_mut()
            .dispatch(UiCommand::TakePreview { transition })?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, renderer);
            if let (Some(source), FrameTransition::CrossFade { .. }) = (source, transition) {
                match renderer
                    .borrow_mut()
                    .render_transition(&source, &target, transition)
                {
                    Ok(Some(frame)) => ui.set_program_image(frame_to_image(&frame)),
                    Ok(None) => {}
                    Err(error) => {
                        ui.set_status_message(format!("Transition preview failed: {error}").into());
                    }
                }
            }
        }
        Err(error) => ui.set_status_message(format!("Transition failed: {error}").into()),
    }
}

pub(crate) fn push_program_frame(
    ui: &MainWindow,
    frame: Option<VideoFrame>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let result = frame
        .ok_or_else(|| std::io::Error::other("program scene is empty").into())
        .and_then(|frame| output.borrow_mut().push_frame(&frame));
    if let Err(error) = result {
        ui.set_status_message(format!("Output failed: {error}").into());
    }
}

pub(crate) fn install_mixer_callbacks(
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
