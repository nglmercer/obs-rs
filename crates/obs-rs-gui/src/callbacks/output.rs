use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_engine::OutputLifecycle;
use obs_rs_media::{FrameTransition, RawVideoFrame, VideoFrame};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, Weak};

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

fn install_transition_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let cut_state = Rc::clone(state);
    let cut_surface = Rc::clone(surface);
    ui.on_cut_transition(move || {
        take_transition_and_refresh(&weak, &cut_state, &cut_surface, FrameTransition::Cut);
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
        );
    });
}

pub(crate) fn take_transition_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    transition: FrameTransition,
) {
    let target = state.borrow().preview_scene().map(str::to_owned);
    if target.is_none() {
        if let Some(ui) = weak.upgrade() {
            ui.set_status_message("Transition failed: no preview scene is selected".into());
        }
        return;
    }
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
            refresh_ui(&ui, state, surface);
        }
        Err(error) => ui.set_status_message(format!("Transition failed: {error}").into()),
    }
}

pub(crate) fn push_program_frame(
    ui: &MainWindow,
    frame: Option<VideoFrame>,
    raw_frame: Option<RawVideoFrame>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    // An accelerated frame is only usable while the encoders run at the canvas
    // geometry: packed and planar layouts are not resampled, so a scaled output
    // takes the RGBA path instead of dropping the frame.
    let accepts_raw = output.borrow().accepts_raw_frames();
    if let Some(frame) = raw_frame.filter(|_| accepts_raw) {
        output.borrow_mut().push_raw_frame(frame);
    } else if let Some(frame) = frame {
        output.borrow_mut().push_frame(&frame);
    } else {
        ui.set_status_message("Output skipped: program scene is empty".into());
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
