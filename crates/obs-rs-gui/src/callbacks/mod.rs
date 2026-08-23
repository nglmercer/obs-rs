pub(crate) mod add_source;
pub(crate) mod canvas;
pub(crate) mod docks;
mod hotkeys;
pub(crate) mod menu;
pub(crate) mod monitor;
mod output;
mod project;
mod scene;
pub(crate) mod settings;
pub(crate) mod setup;
mod source;
pub(crate) mod source_filters;
pub(crate) mod source_properties;
pub(crate) mod source_transform;

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::{
    preview_worker::{RenderTargets, MAX_MULTIVIEW_SCENES},
    refresh_output_ui, refresh_preview_frames_for_view, MainWindow, OutputRuntime, PreviewRenderer,
    PreviewSurface, PreviewWorker,
};

pub(crate) use add_source::install_add_source_window;
#[cfg(test)]
pub(crate) use add_source::{add_source_window, populate_add_source_window};
pub(crate) use canvas::{install_canvas_callbacks, selection_rect, CanvasController};
#[cfg(test)]
pub(crate) use docks::install_dock_callbacks;
pub(crate) use docks::install_dock_callbacks_with_layout;
pub(crate) use hotkeys::install_shortcut_callbacks;
pub(crate) use menu::{install_menu_callbacks, ProjectorController};
pub(crate) use monitor::install_monitor_window;
pub(crate) use output::{install_mixer_callbacks, install_output_callbacks, push_program_frame};
pub(crate) use project::{
    duplicate_scene_and_refresh, install_project_callbacks, project_store, rename_scene_and_refresh,
};
pub(crate) use scene::install_scene_callbacks;
#[cfg(test)]
pub(crate) use settings::populate_settings_models;
pub(crate) use settings::{install_settings_window, PeerWindows};
pub(crate) use setup::install_setup_window;
#[cfg(test)]
pub(crate) use source::selected_target;
pub(crate) use source::{
    apply_source_name_and_refresh, apply_source_settings_and_refresh, apply_source_settings_to,
    apply_source_transform_to, apply_source_transforms_to, duplicate_source_and_refresh,
    flip_source_and_refresh, item_for_target, move_source_and_refresh, move_source_to_and_refresh,
    remove_scene_and_refresh, remove_source_and_refresh, reset_source_transform_and_refresh,
    scene_item_target, source_target, source_target_is_locked, source_transform_document,
    target_settings_document, toggle_source_locked_and_refresh,
    toggle_source_visibility_and_refresh, transform_source_and_refresh, SceneItemTarget,
    SourceTarget,
};
pub(crate) use source_filters::install_source_filters_window;
pub(crate) use source_properties::install_source_properties_window;
pub(crate) use source_transform::install_source_transform_window;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bounded render consumer is an independent demand bit"
)]
struct RenderDemand {
    request_preview: bool,
    request_program_view: bool,
    request_multiview: bool,
    request_source_projector: bool,
    interval: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowRenderState {
    Hidden,
    Minimized,
    Visible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputRenderState {
    Idle,
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectorRenderDemand {
    None,
    Preview,
    Program,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StudioView {
    Single,
    Studio,
    Multiview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RenderDemandInput {
    window: WindowRenderState,
    output: OutputRenderState,
    projectors: ProjectorRenderDemand,
    multiview_projector: bool,
    source_projector: bool,
    view: StudioView,
}

/// Chooses which consumers need a new GUI render and how often to service
/// them. Output cadence is independent: an active output keeps the worker
/// ticking even when the studio window is hidden, but does not create unused
/// preview/program-view frames.
fn render_demand(input: RenderDemandInput) -> RenderDemand {
    let main_interactive = input.window == WindowRenderState::Visible;
    let projector_open = input.projectors != ProjectorRenderDemand::None
        || input.multiview_projector
        || input.source_projector;
    let request_preview = (main_interactive && input.view != StudioView::Multiview)
        || input.window == WindowRenderState::Minimized
        || matches!(
            input.projectors,
            ProjectorRenderDemand::Preview | ProjectorRenderDemand::Both
        );
    let request_program_view = matches!(
        input.projectors,
        ProjectorRenderDemand::Program | ProjectorRenderDemand::Both
    ) || (main_interactive && input.view == StudioView::Studio);
    let request_multiview =
        input.multiview_projector || (main_interactive && input.view == StudioView::Multiview);
    let request_source_projector = input.source_projector;
    let interval =
        if input.output == OutputRenderState::Active || projector_open || main_interactive {
            Some(Duration::from_millis(16))
        } else if input.window == WindowRenderState::Minimized {
            Some(Duration::from_millis(200))
        } else {
            None
        };
    RenderDemand {
        request_preview,
        request_program_view,
        request_multiview,
        request_source_projector,
        interval,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the timer coordinates bounded rendering and the output lifecycle"
)]
pub(crate) fn start_preview_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    preview_worker: &Rc<PreviewWorker>,
    output: &Rc<RefCell<OutputRuntime>>,
    projectors: &Rc<ProjectorController>,
    docks: &Rc<docks::DockController>,
    canvas: &Rc<CanvasController>,
) -> Timer {
    let timer = Timer::default();
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let preview_worker = Rc::clone(preview_worker);
    let output = Rc::clone(output);
    let projectors = Rc::clone(projectors);
    let docks = Rc::clone(docks);
    let canvas = Rc::clone(canvas);
    let mut last_output_ui_refresh = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut last_preview_request = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let callback_started = Instant::now();
        let (revision, preview_scene, program_scene, program_transition, output_active) = {
            let mut state = state.borrow_mut();
            (
                state.project_session().revision(),
                state.preview_scene().map(str::to_owned),
                state.program_scene().map(str::to_owned),
                state.transition_snapshot(Instant::now()),
                state.recording() || state.streaming(),
            )
        };
        let window = if ui.window().is_minimized() {
            WindowRenderState::Minimized
        } else if ui.window().is_visible() {
            WindowRenderState::Visible
        } else {
            WindowRenderState::Hidden
        };
        let projector_demand = match (projectors.wants_preview(), projectors.wants_program()) {
            (false, false) => ProjectorRenderDemand::None,
            (true, false) => ProjectorRenderDemand::Preview,
            (false, true) => ProjectorRenderDemand::Program,
            (true, true) => ProjectorRenderDemand::Both,
        };
        let multiview_projector = projectors.wants_multiview();
        let source_projector = projectors.source_target();
        let demand = render_demand(RenderDemandInput {
            window,
            output: if output_active {
                OutputRenderState::Active
            } else {
                OutputRenderState::Idle
            },
            projectors: projector_demand,
            multiview_projector,
            source_projector: source_projector.is_some(),
            view: match ui.get_view_mode() {
                0 => StudioView::Studio,
                2 => StudioView::Multiview,
                _ => StudioView::Single,
            },
        });
        // A canvas change held back while recording is applied here, at the
        // first tick after the output stopped, and before the ordinary project
        // sync so both reach the engine in one rebuild.
        if !output_active {
            match settings::apply_staged_audio_format(&output) {
                Some(Ok(format)) => ui.set_status_message(
                    format!(
                        "Audio format changed to {} Hz / {} channels now that the output stopped",
                        format.sample_rate(),
                        format.channels()
                    )
                    .into(),
                ),
                Some(Err(error)) => {
                    ui.set_status_message(
                        format!("Staged audio format change failed: {error}").into(),
                    );
                }
                None => {}
            }
            match settings::apply_staged_video_format(&state, &output) {
                Some(Ok(format)) => ui.set_status_message(
                    format!(
                        "Canvas changed to {}x{} now that the output stopped",
                        format.width(),
                        format.height()
                    )
                    .into(),
                ),
                Some(Err(error)) => {
                    ui.set_status_message(format!("Staged canvas change failed: {error}").into());
                }
                None => {}
            }
            // The encoded output geometry is staged the same way and applied
            // in the same window, so one stopped output rebuilds the engine
            // once rather than twice.
            match settings::apply_staged_output_scaling(&output) {
                Some(Ok((width, height))) => ui.set_status_message(
                    format!(
                        "Output resolution changed to {width}x{height} now that the output stopped"
                    )
                    .into(),
                ),
                Some(Err(error)) => {
                    ui.set_status_message(format!("Staged output scaling failed: {error}").into());
                }
                None => {}
            }
        }
        if !output_active && output.borrow().needs_project_sync(revision) {
            let project = state.borrow().project_session().project().clone();
            if let Err(error) = output.borrow_mut().sync_project(project, revision) {
                ui.set_status_message(format!("Output project sync failed: {error}").into());
            }
        }
        let preview_due = demand
            .interval
            .is_some_and(|interval| last_preview_request.elapsed() >= interval);
        if preview_due {
            let state = state.borrow();
            let draft = canvas.draft();
            if let Some(profile) = state.project_session().project().active_profile_spec() {
                preview_worker.request_render(
                    state.project_session().project(),
                    revision,
                    RenderTargets {
                        preview_scene: demand
                            .request_preview
                            .then_some(preview_scene.as_deref())
                            .flatten(),
                        preview_format: PreviewRenderer::preview_format_for_canvas(
                            profile.video_format(),
                        ),
                        program_scene: (output_active || demand.request_program_view)
                            .then_some(program_scene.as_deref())
                            .flatten(),
                        program_transition,
                        program_preview_format: PreviewRenderer::preview_format_for_canvas(
                            profile.video_format(),
                        ),
                        multiview_scenes: if demand.request_multiview {
                            profile
                                .scenes()
                                .take(MAX_MULTIVIEW_SCENES)
                                .map(|scene| scene.id().as_str().to_owned())
                                .collect()
                        } else {
                            Vec::new()
                        },
                        multiview_format: PreviewRenderer::multiview_format_for_canvas(
                            profile.video_format(),
                            profile.scenes().take(MAX_MULTIVIEW_SCENES).count().max(1),
                        ),
                        source_projector: demand
                            .request_source_projector
                            .then_some(source_projector.as_ref())
                            .flatten(),
                        // A program projector is a third consumer of the program
                        // canvas, so single-canvas editing has to render it again
                        // while one is up.
                        render_program: demand.request_program_view,
                        prepare_output: output_active,
                        prepare_output_rgba: output_active && !output.borrow().accepts_raw_frames(),
                        // A canvas drag reaches the compositor here rather than
                        // through a project revision, so the picture follows the
                        // pointer while the undo history stays at one entry per
                        // gesture.
                        draft: draft.as_ref(),
                    },
                );
                last_preview_request = Instant::now();
            }
        }
        let (preview_frame, program_frame, program_output, program_output_frame, render_error) =
            refresh_preview_frames_for_view(&ui, &preview_worker);
        if let Some(error) = render_error {
            ui.set_status_message(error.into());
        }
        projectors.sync(&ui);
        if output_active {
            push_program_frame(
                &ui,
                program_frame.as_ref(),
                program_output,
                program_output_frame,
                &output,
            );
        } else if let Some(frame) = preview_frame.as_ref().or(program_frame.as_ref()) {
            output.borrow().monitor_audio(frame);
        }
        reconcile_output_lifecycle(&ui, &state, &output);
        // Worker counters remain live in the status bar, but formatting them
        // at 30 fps adds needless allocations and mutex traffic while editing.
        if last_output_ui_refresh.elapsed() >= Duration::from_millis(100) {
            refresh_output_ui(&ui, &output);
            refresh_input_meter(&ui, &state, &output);
            docks.sync(&ui);
            last_output_ui_refresh = Instant::now();
        }
        preview_worker.record_ui_callback(callback_started.elapsed());
    });
    timer
}

/// Brings the desktop's output booleans back in line with the engine's phases.
///
/// The stream and recording callbacks enqueue lifecycle work. This bridge
/// consumes the worker's bounded state/events and clears a desktop claim only
/// after the worker reports a stopped or failed output.
pub(crate) fn reconcile_output_lifecycle(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    for event in output.borrow_mut().take_output_events() {
        match event {
            obs_rs_engine::OutputEvent::Starting => {
                ui.set_status_message("Streaming connection starting".into());
            }
            obs_rs_engine::OutputEvent::Running => {
                if !state.borrow().streaming() {
                    let _ = state
                        .borrow_mut()
                        .dispatch(obs_rs_ui::UiCommand::StartStreaming);
                }
                ui.set_streaming(true);
                ui.set_status_message("Streaming connected".into());
                // Auto-record begins only after the transport is actually
                // running, never merely because a start was enqueued.
                if ui.get_auto_record_when_streaming() && !state.borrow().recording() {
                    ui.invoke_toggle_recording();
                }
            }
            obs_rs_engine::OutputEvent::Disconnected => {
                ui.set_status_message("Streaming disconnected".into());
            }
            obs_rs_engine::OutputEvent::Reconnecting { attempt } => {
                ui.set_status_message(format!("Streaming reconnect attempt {attempt}").into());
            }
            obs_rs_engine::OutputEvent::Failed { reason } => {
                if state.borrow().streaming() {
                    let _ = state
                        .borrow_mut()
                        .dispatch(obs_rs_ui::UiCommand::StopStreaming);
                }
                ui.set_streaming(false);
                ui.set_status_message(format!("Streaming failed: {reason}").into());
                ui.set_active_modal(11);
            }
            obs_rs_engine::OutputEvent::Stopping => {
                ui.set_status_message("Streaming stopping".into());
            }
            obs_rs_engine::OutputEvent::Stopped => {
                if state.borrow().streaming() {
                    let _ = state
                        .borrow_mut()
                        .dispatch(obs_rs_ui::UiCommand::StopStreaming);
                }
                ui.set_streaming(false);
                ui.set_status_message("Streaming stopped".into());
            }
        }
    }
    let (recording, streaming) = output.borrow().lifecycles();
    let (claims_recording, claims_streaming) = {
        let state = state.borrow();
        (state.recording(), state.streaming())
    };

    if claims_recording && recording.is_stopped() {
        let _ = state
            .borrow_mut()
            .dispatch(obs_rs_ui::UiCommand::StopRecording);
        ui.set_recording(false);
        if recording == obs_rs_engine::OutputLifecycle::Failed {
            ui.set_status_message("Recording stopped after an output failure".into());
            ui.set_active_modal(11);
        }
    }
    if claims_streaming && streaming.is_stopped() {
        let _ = state
            .borrow_mut()
            .dispatch(obs_rs_ui::UiCommand::StopStreaming);
        ui.set_streaming(false);
        if streaming == obs_rs_engine::OutputLifecycle::Failed {
            ui.set_status_message("Streaming stopped after transport failure".into());
            ui.set_active_modal(11);
        }
    }
}

/// Pushes the engine's live input peak onto the mixer's microphone channel.
///
/// The meter is refreshed from the capture backend rather than from a mix the
/// desktop performs itself, because the engine worker is what actually reads
/// the microphone. Only the mixer rows are rebuilt, so this stays off the
/// scene-graph refresh path.
fn refresh_input_meter(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    if ui.get_meters_paused() {
        return;
    }
    // A fallback generator is not the user's microphone, so its level must not
    // be shown as if the input were live.
    let (input_meter, desktop_meter) = {
        let output = output.borrow();
        let input_meter = if output.audio_is_fallback() {
            (0, 0, false)
        } else {
            output.input_meter()
        };
        (input_meter, output.desktop_meter())
    };
    let mut state_guard = state.borrow_mut();
    let changed = state_guard
        .set_channel_meter(
            crate::MIC_CHANNEL_ID,
            input_meter.0,
            input_meter.1,
            input_meter.2,
        )
        .is_ok()
        | state_guard
            .set_channel_meter(
                crate::DESKTOP_CHANNEL_ID,
                desktop_meter.0,
                desktop_meter.1,
                desktop_meter.2,
            )
            .is_ok();
    drop(state_guard);
    if changed {
        crate::refresh::refresh_mixer_rows(ui, &state.borrow());
    }
}

pub(crate) fn install_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    install_scene_callbacks(ui, state, surface);
    install_output_callbacks(ui, state, surface, output);
    install_mixer_callbacks(ui, state, surface, output);
    install_project_callbacks(ui, state, surface, output);
    install_shortcut_callbacks(ui, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_idle_window_has_no_render_demand() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Hidden,
                output: OutputRenderState::Idle,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: false,
                source_projector: false,
                view: StudioView::Single,
            }),
            RenderDemand {
                request_preview: false,
                request_program_view: false,
                request_multiview: false,
                request_source_projector: false,
                interval: None,
            }
        );
    }

    #[test]
    fn minimized_window_keeps_a_bounded_five_fps_preview() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Minimized,
                output: OutputRenderState::Idle,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: false,
                source_projector: false,
                view: StudioView::Studio,
            }),
            RenderDemand {
                request_preview: true,
                request_program_view: false,
                request_multiview: false,
                request_source_projector: false,
                interval: Some(Duration::from_millis(200)),
            }
        );
    }

    #[test]
    fn hidden_output_renders_only_the_output_consumer() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Hidden,
                output: OutputRenderState::Active,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: false,
                source_projector: false,
                view: StudioView::Studio,
            }),
            RenderDemand {
                request_preview: false,
                request_program_view: false,
                request_multiview: false,
                request_source_projector: false,
                interval: Some(Duration::from_millis(16)),
            }
        );
    }

    #[test]
    fn visible_studio_mode_requests_both_gui_feeds_at_sixty_fps() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Visible,
                output: OutputRenderState::Idle,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: false,
                source_projector: false,
                view: StudioView::Studio,
            }),
            RenderDemand {
                request_preview: true,
                request_program_view: true,
                request_multiview: false,
                request_source_projector: false,
                interval: Some(Duration::from_millis(16)),
            }
        );
    }

    #[test]
    fn visible_multiview_requests_only_the_bounded_scene_grid() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Visible,
                output: OutputRenderState::Idle,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: false,
                source_projector: false,
                view: StudioView::Multiview,
            }),
            RenderDemand {
                request_preview: false,
                request_program_view: false,
                request_multiview: true,
                request_source_projector: false,
                interval: Some(Duration::from_millis(16)),
            }
        );
    }

    #[test]
    fn hidden_multiview_projector_requests_only_the_bounded_scene_grid() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Hidden,
                output: OutputRenderState::Idle,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: true,
                source_projector: false,
                view: StudioView::Single,
            }),
            RenderDemand {
                request_preview: false,
                request_program_view: false,
                request_multiview: true,
                request_source_projector: false,
                interval: Some(Duration::from_millis(16)),
            }
        );
    }

    #[test]
    fn hidden_source_projector_requests_only_the_selected_source() {
        assert_eq!(
            render_demand(RenderDemandInput {
                window: WindowRenderState::Hidden,
                output: OutputRenderState::Idle,
                projectors: ProjectorRenderDemand::None,
                multiview_projector: false,
                source_projector: true,
                view: StudioView::Single,
            }),
            RenderDemand {
                request_preview: false,
                request_program_view: false,
                request_multiview: false,
                request_source_projector: true,
                interval: Some(Duration::from_millis(16)),
            }
        );
    }
}
