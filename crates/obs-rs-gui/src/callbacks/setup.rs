//! First-run setup wizard and its local benchmark lifecycle.

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use obs_rs_benchmark::{SetupBenchmarkReport, SetupCandidate};
use obs_rs_project::{ProjectCommand, SceneItemSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};

use crate::settings::{AppSettings, SetupState};
use crate::settings_model::{FpsMode, VideoSettings};
use crate::{
    callbacks::settings::{apply_settings_snapshot, SettingsController},
    fixtures::{capture_devices, kind_runs_in_this_session, source_settings},
    i18n, refresh_ui, MainWindow, OutputRuntime, Palette, PreviewSurface, SetupWindow,
};

type BenchmarkMessage = Result<SetupBenchmarkReport, String>;

/// Owns the setup window and keeps all benchmark work off the Slint thread.
pub(crate) struct SetupController {
    window: SetupWindow,
    main: Weak<MainWindow>,
    state: Rc<RefCell<DesktopState>>,
    surface: Rc<RefCell<PreviewSurface>>,
    output: Rc<RefCell<OutputRuntime>>,
    settings: Rc<SettingsController>,
    receiver: RefCell<Option<Receiver<BenchmarkMessage>>>,
    report: RefCell<Option<SetupBenchmarkReport>>,
    display: Option<DisplayChoice>,
    poll_timer: Timer,
}

#[derive(Clone, Debug)]
struct DisplayChoice {
    kind: String,
    name: String,
}

/// Creates the wizard and registers its open action on the main window.
pub(crate) fn install_setup_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    settings: &Rc<SettingsController>,
) -> Result<Rc<SetupController>, slint::PlatformError> {
    let window = SetupWindow::new()?;
    let display = discover_display();
    let controller = Rc::new(SetupController {
        window,
        main: ui.as_weak(),
        state: Rc::clone(state),
        surface: Rc::clone(surface),
        output: Rc::clone(output),
        settings: Rc::clone(settings),
        receiver: RefCell::new(None),
        report: RefCell::new(None),
        display,
        poll_timer: Timer::default(),
    });

    controller.apply_theme();
    controller.reset_view();
    install_open_callback(ui, &controller);
    install_window_callbacks(&controller);
    install_polling(&controller);
    Ok(controller)
}

impl SetupController {
    /// Opens a fresh setup draft. `setup-active` is the blocking scrim behind
    /// this separate movable window.
    pub(crate) fn open(&self) {
        self.reset_view();
        if let Some(main) = self.main.upgrade() {
            main.set_setup_active(true);
        }
        match self.window.show() {
            Ok(()) => self.window.invoke_focus_keyboard_boundary(),
            Err(error) => {
                if let Some(main) = self.main.upgrade() {
                    main.set_status_message(format!("Setup window: {error}").into());
                }
            }
        }
    }

    fn apply_theme(&self) {
        let locale = self.state.borrow().locale();
        self.window
            .global::<crate::I18n>()
            .set_text(i18n::catalog(locale));
        self.window
            .global::<Palette>()
            .set_tokens(self.settings.committed().tokens());
    }

    fn reset_view(&self) {
        self.receiver.borrow_mut().take();
        self.report.borrow_mut().take();
        self.window.set_step(0);
        self.window.set_running(false);
        self.window
            .set_status(i18n::with_catalog(self.state.borrow().locale(), |text| {
                text.settings_ui.setup_benchmark_hint.clone()
            }));
        self.window.set_recommended_format("".into());
        self.window.set_result_rows(empty_rows());
        self.window.set_display_device(
            self.display
                .as_ref()
                .map_or_else(|| "Not detected".to_owned(), |choice| choice.name.clone())
                .into(),
        );
        let microphone = self
            .output
            .borrow_mut()
            .audio_input_devices()
            .first()
            .map_or_else(|| "Not detected".to_owned(), |(_, name)| name.clone());
        self.window.set_microphone_device(microphone.into());
    }

    fn start_benchmark(self: &Rc<Self>) {
        if self.window.get_running() {
            return;
        }
        self.window.set_running(true);
        let locale = self.state.borrow().locale();
        self.window.set_status(i18n::with_catalog(locale, |text| {
            text.settings_ui.setup_running.clone()
        }));
        let (sender, receiver) = mpsc::channel();
        *self.receiver.borrow_mut() = Some(receiver);
        thread::spawn(move || {
            let _ = sender.send(crate::run_gui_setup_benchmark());
        });
    }

    fn poll_benchmark(&self) {
        let result = {
            let receiver = self.receiver.borrow();
            let Some(receiver) = receiver.as_ref() else {
                return;
            };
            match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("benchmark worker disconnected".to_owned()),
            }
        };
        self.receiver.borrow_mut().take();
        self.window.set_running(false);
        match result {
            Ok(report) => self.show_report(report),
            Err(error) => {
                self.window.set_step(0);
                self.window
                    .set_status(format!("Benchmark failed: {error}").into());
            }
        }
    }

    fn show_report(&self, report: SetupBenchmarkReport) {
        let recommended = report
            .recommended
            .and_then(|index| report.candidates.get(index));
        let Some(recommended) = recommended else {
            self.window
                .set_status("No stable candidate was found".into());
            return;
        };
        self.window
            .set_recommended_format(format_candidate(recommended).into());
        self.window.set_result_rows(report_rows(&report));
        self.window.set_step(1);
        self.window.set_status(
            format!(
                "Benchmark complete in {} ms. Review the recommendation before applying.",
                report.elapsed_millis
            )
            .into(),
        );
        *self.report.borrow_mut() = Some(report);
    }

    fn apply(&self) {
        let report = self.report.borrow().clone();
        let Some(report) = report.as_ref() else {
            return;
        };
        let Some(index) = report.recommended else {
            return;
        };
        let Some(candidate) = report.candidates.get(index) else {
            return;
        };
        let mut settings = self.settings.committed();
        apply_candidate_to_settings(&mut settings, candidate);
        if settings.audio_input_id.is_empty() {
            let devices = self.output.borrow_mut().audio_input_devices();
            if let Some((id, _)) = devices.first() {
                settings.audio_input_id.clone_from(id);
            }
        }
        settings.setup_state = SetupState::Completed;
        settings.setup_benchmark_summary = report.summary();
        if let Err(error) = self.add_starter_display(candidate) {
            self.window
                .set_status(format!("Display source was not added: {error}").into());
        }
        let Some(main) = self.main.upgrade() else {
            return;
        };
        apply_settings_snapshot(
            &main,
            &self.state,
            &self.surface,
            &self.output,
            &self.settings,
            &settings,
        );
        main.set_setup_benchmark_summary(report.summary().into());
        main.set_setup_active(false);
        main.set_status_message("Setup applied; the studio is ready for review".into());
        let _ = self.window.hide();
    }

    fn skip(&self) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let mut settings = self.settings.committed();
        settings.setup_state = SetupState::Skipped;
        if let Err(error) = settings.save(&PathBuf::from(settings_path())) {
            self.window
                .set_status(format!("Could not save setup state: {error}").into());
            return;
        }
        // Keep the in-memory settings controller synchronized with the state
        // written above. No other setting is changed by Skip.
        apply_settings_snapshot(
            &main,
            &self.state,
            &self.surface,
            &self.output,
            &self.settings,
            &settings,
        );
        main.set_setup_active(false);
        let _ = self.window.hide();
    }

    fn add_starter_display(&self, candidate: &SetupCandidate) -> Result<(), String> {
        let Some(display) = self.display.as_ref() else {
            return Ok(());
        };
        let (profile, preview, program, already_exists) = {
            let state = self.state.borrow();
            let project = state.project_session().project();
            let profile = project.active_profile().to_string();
            let preview = state.preview_scene().map(str::to_owned);
            let program = state.program_scene().map(str::to_owned);
            let already_exists = project
                .active_profile_spec()
                .is_some_and(|profile| profile.has_source("setup-display"));
            (profile, preview, program, already_exists)
        };
        if already_exists {
            return Ok(());
        }
        let preview = preview.ok_or_else(|| "no preview scene is selected".to_owned())?;
        let mut config = source_settings(&display.kind).map_err(|error| error.to_string())?;
        config
            .set("width", &candidate.width.to_string())
            .map_err(|error| error.to_string())?;
        config
            .set("height", &candidate.height.to_string())
            .map_err(|error| error.to_string())?;
        let source = SourceSpec::new("setup-display", &display.kind, "Display capture", config)
            .map_err(|error| error.to_string())?;
        self.state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: profile.clone(),
                scene: preview,
                source,
            }))
            .map_err(|error| error.to_string())?;
        if let Some(program) = program {
            let has_program_source = self
                .state
                .borrow()
                .project_session()
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(program.as_str()))
                .is_some_and(|scene| scene.has_source("setup-display"));
            if !has_program_source {
                let item = SceneItemSpec::new("setup-display-program", "setup-display")
                    .map_err(|error| error.to_string())?;
                self.state
                    .borrow_mut()
                    .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
                        profile,
                        scene: program,
                        item,
                    }))
                    .map_err(|error| error.to_string())?;
            }
        }
        let Some(main) = self.main.upgrade() else {
            return Err("studio closed".to_owned());
        };
        refresh_ui(&main, &self.state, &self.surface);
        Ok(())
    }
}

fn install_open_callback(ui: &MainWindow, controller: &Rc<SetupController>) {
    let callback_controller = Rc::clone(controller);
    ui.on_open_setup_window(move || callback_controller.open());
}

fn install_window_callbacks(controller: &Rc<SetupController>) {
    let benchmark_controller = Rc::clone(controller);
    controller
        .window
        .on_run_benchmark(move || benchmark_controller.start_benchmark());

    let apply_controller = Rc::clone(controller);
    controller
        .window
        .on_apply_setup(move || apply_controller.apply());

    let skip_controller = Rc::clone(controller);
    controller
        .window
        .on_skip_setup(move || skip_controller.skip());

    let close_controller = Rc::clone(controller);
    controller
        .window
        .on_close_requested(move || close_controller.skip());
    install_native_close(&controller.window);
}

/// Bridges the desktop window-manager close event to the wizard's existing
/// close callback. Keeping the bridge separate means the native event cannot
/// bypass the `skip()` policy owned by the controller.
pub(crate) fn install_native_close(window: &SetupWindow) {
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        if let Some(window) = weak.upgrade() {
            window.invoke_close_requested();
        }
        slint::CloseRequestResponse::HideWindow
    });
}

fn install_polling(controller: &Rc<SetupController>) {
    let weak = Rc::downgrade(controller);
    controller
        .poll_timer
        .start(TimerMode::Repeated, Duration::from_millis(80), move || {
            if let Some(controller) = weak.upgrade() {
                controller.poll_benchmark();
            }
        });
}

fn discover_display() -> Option<DisplayChoice> {
    let preferred = if kind_runs_in_this_session("x11_screen_capture") {
        Some("x11_screen_capture")
    } else if kind_runs_in_this_session("wayland_screen_capture") {
        Some("wayland_screen_capture")
    } else {
        None
    };
    preferred
        .into_iter()
        .chain(["screen_capture"])
        .find_map(|kind| {
            capture_devices(kind)
                .first()
                .map(|(_, name)| DisplayChoice {
                    kind: kind.to_owned(),
                    // The source factory reuses the current first display when
                    // it serializes its settings; the label is what the user
                    // needs to review here.
                    name: name.clone(),
                })
        })
}

fn apply_candidate_to_settings(settings: &mut AppSettings, candidate: &SetupCandidate) {
    settings.video = VideoSettings {
        base_width: candidate.width,
        base_height: candidate.height,
        output_width: candidate.width,
        output_height: candidate.height,
        fps_mode: FpsMode::Integer,
        fps_numerator: candidate.fps.max(1),
        fps_denominator: 1,
        ..settings.video
    };
    let mut output = settings.video;
    output.output_width = candidate.width;
    output.output_height = candidate.height;
    settings.video = output;
}

fn format_candidate(candidate: &SetupCandidate) -> String {
    format!(
        "{} ({}x{} @ {} fps)",
        candidate.label, candidate.width, candidate.height, candidate.fps
    )
}

fn report_rows(report: &SetupBenchmarkReport) -> ModelRc<SharedString> {
    let rows = report
        .candidates
        .iter()
        .map(|candidate| {
            if let Some(metrics) = candidate.result {
                let marker = if report.recommended
                    == Some(
                        report
                            .candidates
                            .iter()
                            .position(|entry| entry.label == candidate.label)
                            .unwrap_or(usize::MAX),
                    ) {
                    " · recommended"
                } else {
                    ""
                };
                format!(
                    "{}{} — tier {} · p95 {} ms · p99 {} ms · missed {} · dropped {}",
                    format_candidate(candidate),
                    marker,
                    candidate.tier,
                    metrics.render_p95_nanos / 1_000_000,
                    metrics.render_p99_nanos / 1_000_000,
                    metrics.missed_deadlines,
                    metrics.dropped_frames
                )
            } else {
                format!(
                    "{} — unavailable: {}",
                    candidate.label,
                    candidate.error.as_deref().unwrap_or("unknown error")
                )
            }
        })
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn empty_rows() -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(Vec::<SharedString>::new()))
}

fn settings_path() -> String {
    crate::settings::settings_path()
        .to_string_lossy()
        .into_owned()
}
