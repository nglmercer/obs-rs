use std::{cell::RefCell, rc::Rc, sync::Arc};

use obs_rs_media::StingerClip;
use obs_rs_ui::{DesktopState, UiCommand};
use slint::ComponentHandle;

use super::output::parse_transition_duration;
use crate::{refresh_ui, MainWindow, PreviewSurface, StingerLoadController};

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
                    take_loader.borrow_mut().request_on_demand_take(
                        state.project_session().project(),
                        state.preview_scene(),
                        duration_ms,
                    )
                };
                match request_result {
                    Ok(_) => ui.set_status_message(
                        "Stinger is loading; Take will complete when the resource is ready".into(),
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
        dispatch_stinger_clip(&ui, &take_state, &take_surface, clip, duration_ms);
    });
}

/// Completes one pending Take after the bounded worker publishes its clip.
///
/// This is called from the ordinary refresh cadence, never from the decoder
/// thread. The pending intent is consumed before the state command is sent.
pub(crate) fn dispatch_pending_stinger_take(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    stinger_loader: &Rc<RefCell<StingerLoadController>>,
) {
    let Some((clip, duration_ms)) = stinger_loader.borrow_mut().take_ready_pending() else {
        return;
    };
    dispatch_stinger_clip(ui, state, surface, clip, duration_ms);
}

fn dispatch_stinger_clip(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    clip: Arc<StingerClip>,
    duration_ms: u32,
) {
    let result = state
        .borrow_mut()
        .dispatch(UiCommand::TakeStinger { clip, duration_ms });
    match result {
        Ok(()) => {
            refresh_ui(ui, state, surface);
            ui.set_status_message("Stinger Take sent to Program".into());
        }
        Err(error) => ui.set_status_message(format!("Stinger Take failed: {error}").into()),
    }
}
