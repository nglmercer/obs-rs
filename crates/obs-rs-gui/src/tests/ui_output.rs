use super::*;
use crate::StingerLoadController;

pub(super) fn exercise_recording_controls(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-callback-{token}.obsr"));
    ui.set_recording_path(path.to_string_lossy().into_owned().into());
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);

    ui.invoke_toggle_recording();
    assert!(
        state.borrow().recording(),
        "Record button must start the state"
    );
    let mut renderer = PreviewRenderer::new(state.borrow().project_session().project(), 0)
        .expect("preview renderer");
    let frame = wait_for_frame(|| renderer.render("program")).expect("program scene frame");
    crate::callbacks::push_program_frame(ui, None, None, Some(frame), &output);
    ui.invoke_toggle_recording();
    for _ in 0..100 {
        crate::callbacks::reconcile_output_lifecycle(ui, state, &output);
        if !state.borrow().recording() && path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !state.borrow().recording(),
        "Record button must stop the state"
    );

    let bytes = std::fs::read(&path).expect("GUI recording file");
    assert!(!bytes.is_empty());
    let packets = MemoryMuxer::decode(&bytes).expect("GUI recording container");
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Video));
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Audio));
    std::fs::remove_file(path).expect("remove GUI recording fixture");

    exercise_replay_controls(ui, state, surface);
    exercise_output_reconciliation(ui, state, &output);

    exercise_transition_callbacks(ui, state);
    exercise_stinger_take_callback(ui, state, surface);

    exercise_scene_properties_dialog(ui, state, surface);
}

/// Exercises the explicit Take path with a worker-injected clip. The first
/// click starts a bounded on-demand load without doing I/O; the refresh-side
/// completion then sends only the already-published clip to the state machine.
fn exercise_stinger_take_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let format = surface.borrow().format;
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectPreviewScene {
            id: "preview".to_owned(),
        })
        .expect("Stinger fixture should select preview");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("Stinger fixture should select program");
    let spec = StingerSpec::new("test://stinger", 500, false, false).expect("stinger spec");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(
            ProjectCommand::SetSceneStingerOverride {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                stinger: Some(spec.clone()),
            },
        ))
        .expect("Stinger fixture should persist the on-demand resource");
    let loader = StingerLoadController::with_loader(
        |request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
            StingerClip::new(
                vec![VideoFrame::solid(
                    request.target_format(),
                    Timestamp::ZERO,
                    [0, 255, 0, 255],
                )],
                vec![100_000_000],
                500,
            )
        },
        format,
    )
    .expect("stinger test loader");
    let loader = Rc::new(RefCell::new(loader));
    crate::callbacks::install_stinger_take_callback(ui, state, surface, &loader);

    ui.invoke_take_stinger("450".into());
    assert!(ui.get_status_message().contains("Stinger is loading"));

    let ready = (0..100).any(|_| {
        let event = {
            let state = state.borrow();
            loader
                .borrow_mut()
                .sync(state.project_session().project(), state.preview_scene())
                .expect("stinger poll")
        };
        if matches!(event, Some(crate::stinger_loader::StingerLoadEvent::Ready)) {
            true
        } else {
            std::thread::sleep(std::time::Duration::from_millis(1));
            false
        }
    });
    assert!(
        ready,
        "test Stinger should become ready without blocking the callback"
    );

    crate::callbacks::dispatch_pending_stinger_take(ui, state, surface, &loader);
    assert_eq!(ui.get_status_message(), "Stinger Take sent to Program");
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("Stinger Take should start a transition");
    assert!(transition.stinger().is_some());
    assert_eq!(transition.transition(), FrameTransition::Cut);
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(
            ProjectCommand::SetSceneStingerOverride {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                stinger: None,
            },
        ))
        .expect("Stinger fixture should clear its transient resource");
}

fn exercise_transition_callbacks(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectPreviewScene {
            id: "preview".to_owned(),
        })
        .expect("transition fixture should select a preview scene");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("transition fixture should select a program scene");
    ui.invoke_fade_to_color("#00FF0080".into(), "450".into());
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("Fade to Color callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::FadeToColor {
            progress_milli,
            color: [0, 255, 0, 128],
        } if progress_milli < 1_000
    ));

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene before slide");
    ui.invoke_slide_transition("450".into());
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("Slide callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::Slide {
            progress_milli,
            direction: obs_rs_media::SlideDirection::Left,
        } if progress_milli < 1_000
    ));
    exercise_directional_transition_callbacks(ui, state);

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene before swipe");
    ui.invoke_swipe_transition("450".into());
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("Swipe callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::Swipe {
            progress_milli,
            direction: obs_rs_media::SlideDirection::Left,
            swipe_in: false,
        } if progress_milli < 1_000
    ));

    exercise_luma_transition_callback(ui, state);

    ui.invoke_set_scene_transition("fade_to_color".into(), "450".into(), "#00FF0080".into());
    let override_spec = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(SceneSpec::transition_override)
        .expect("dock action should persist a scene transition override");
    assert_eq!(override_spec.duration_millis(), 450);
    assert_eq!(
        override_spec.kind(),
        obs_rs_media::TransitionKind::FadeToColor {
            color: [0, 255, 0, 128]
        }
    );

    ui.invoke_clear_scene_transition();
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(SceneSpec::transition_override)
        .is_none());

    ui.invoke_set_scene_transition("cross_fade".into(), "0".into(), "#000000FF".into());
    assert!(ui
        .get_status_message()
        .contains("Transition duration must be 1–60000 ms"));

    ui.invoke_fade_to_color("green".into(), "450".into());
    assert!(ui
        .get_status_message()
        .contains("Transition color must be #RRGGBB or #RRGGBBAA"));
}

fn exercise_luma_transition_callback(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene before luma wipe");
    ui.invoke_luma_transition("450".into(), 1, true, "85".into());
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("Luma Wipe callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::LumaWipe {
            progress_milli,
            pattern: obs_rs_media::LumaWipePattern::LinearVertical,
            invert: true,
            softness_milli: 85,
        } if progress_milli < 1_000
    ));
}

fn exercise_directional_transition_callbacks(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene before directional slide");
    ui.invoke_slide_transition_direction("450".into(), 1);
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("directional Slide callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::Slide {
            progress_milli,
            direction: obs_rs_media::SlideDirection::Right,
        } if progress_milli < 1_000
    ));

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene before directional swipe");
    ui.invoke_swipe_transition_direction("450".into(), 2);
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("directional Swipe callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::Swipe {
            progress_milli,
            direction: obs_rs_media::SlideDirection::Up,
            swipe_in: false,
        } if progress_milli < 1_000
    ));

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene before swipe-in");
    ui.invoke_swipe_transition_direction_mode("450".into(), 3, true);
    let transition = state
        .borrow_mut()
        .transition_snapshot(std::time::Instant::now())
        .expect("swipe-in callback should start a transition");
    assert!(matches!(
        transition.transition(),
        FrameTransition::Swipe {
            progress_milli,
            direction: obs_rs_media::SlideDirection::Down,
            swipe_in: true,
        } if progress_milli < 1_000
    ));
}

fn exercise_scene_properties_dialog(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    ui.invoke_select_preview("preview".into());
    refresh_ui(ui, state, surface);
    assert_eq!(ui.get_scene_transition_index(), 0);

    ui.set_scene_name("Dialog scene".into());
    ui.set_scene_transition_index(2);
    ui.set_scene_transition_duration("900".into());
    ui.set_scene_transition_color("#000000FF".into());
    ui.set_scene_stinger_path("assets/intro.webm".into());
    ui.set_scene_stinger_transition_point("625".into());
    ui.set_scene_stinger_preload(true);
    ui.set_scene_stinger_hardware_decode(false);
    ui.invoke_rename_scene();
    {
        let state = state.borrow();
        let scene = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .expect("scene properties target");
        assert_eq!(scene.name(), "Dialog scene");
        let transition = scene.transition_override().expect("cross-fade override");
        assert_eq!(transition.kind(), obs_rs_media::TransitionKind::CrossFade);
        assert_eq!(transition.duration_millis(), 900);
        assert_eq!(
            scene.stinger_override(),
            Some(&StingerSpec::new("assets/intro.webm", 625, true, false).expect("stinger"))
        );
    }

    // The dialog's name and transition are one project command and therefore
    // one undo step.
    ui.invoke_undo_edit();
    {
        let state = state.borrow();
        let scene = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .expect("scene after undo");
        assert_eq!(scene.name(), "Preview");
        assert!(scene.transition_override().is_none());
        assert!(scene.stinger_override().is_none());
    }

    exercise_scene_transition_variants(ui, state);

    ui.set_scene_name("Inherited scene".into());
    ui.set_scene_transition_index(0);
    ui.invoke_rename_scene();
    {
        let state = state.borrow();
        let scene = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .expect("inherited scene");
        assert_eq!(scene.name(), "Inherited scene");
        assert!(scene.transition_override().is_none());
    }
}

fn exercise_scene_transition_variants(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    ui.set_scene_name("Color scene".into());
    ui.set_scene_transition_index(3);
    ui.set_scene_transition_duration("450".into());
    ui.set_scene_transition_color("#00FF0080".into());
    ui.invoke_rename_scene();
    let transition = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(SceneSpec::transition_override)
        .expect("fade-to-color override");
    assert_eq!(transition.duration_millis(), 450);
    assert_eq!(
        transition.kind(),
        obs_rs_media::TransitionKind::FadeToColor {
            color: [0, 255, 0, 128]
        }
    );

    ui.set_scene_name("Slide scene".into());
    ui.set_scene_transition_index(4);
    ui.set_scene_transition_direction_index(1);
    ui.set_scene_transition_duration("600".into());
    ui.invoke_rename_scene();
    let transition = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(SceneSpec::transition_override)
        .expect("slide override");
    assert_eq!(transition.duration_millis(), 600);
    assert_eq!(
        transition.kind(),
        obs_rs_media::TransitionKind::Slide {
            direction: obs_rs_media::SlideDirection::Right,
        }
    );

    ui.set_scene_name("Swipe scene".into());
    ui.set_scene_transition_index(5);
    ui.set_scene_transition_direction_index(2);
    ui.set_scene_transition_swipe_in(true);
    ui.set_scene_transition_duration("650".into());
    ui.invoke_rename_scene();
    let transition = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(SceneSpec::transition_override)
        .expect("swipe override");
    assert_eq!(transition.duration_millis(), 650);
    assert_eq!(
        transition.kind(),
        obs_rs_media::TransitionKind::Swipe {
            direction: obs_rs_media::SlideDirection::Up,
            swipe_in: true,
        }
    );

    ui.set_scene_name("Luma scene".into());
    ui.set_scene_transition_index(6);
    ui.set_scene_transition_luma_pattern_index(1);
    ui.set_scene_transition_luma_invert(true);
    ui.set_scene_transition_luma_softness("85".into());
    ui.set_scene_transition_duration("700".into());
    ui.invoke_rename_scene();
    let transition = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(SceneSpec::transition_override)
        .expect("luma wipe override");
    assert_eq!(transition.duration_millis(), 700);
    assert_eq!(
        transition.kind(),
        obs_rs_media::TransitionKind::LumaWipe {
            pattern: obs_rs_media::LumaWipePattern::LinearVertical,
            invert: true,
            softness_milli: 85,
        }
    );
}

/// Drives the actual Controls-dock replay actions and verifies that the
/// worker-owned history survives an asynchronous save without stopping until
/// the explicit stop action.
fn exercise_replay_controls(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-gui-replay-{token}"));
    std::fs::create_dir(&directory).expect("replay fixture directory");
    let recording_path = directory.join("recording.obsr");
    ui.set_recording_path(recording_path.to_string_lossy().into_owned().into());
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);

    ui.invoke_toggle_replay_buffer();
    assert!(
        ui.get_replay_buffering(),
        "Replay control must expose the accepted start request"
    );
    let mut renderer = PreviewRenderer::new(state.borrow().project_session().project(), 0)
        .expect("preview renderer");
    let frame = wait_for_frame(|| renderer.render("program")).expect("program scene frame");
    crate::callbacks::push_program_frame(ui, None, None, Some(frame), &output);

    ui.invoke_save_replay_buffer();
    let replay_path = (0..100).find_map(|_| {
        crate::refresh_output_ui(ui, &output);
        let path = std::fs::read_dir(&directory)
            .expect("read replay fixture directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "obsr")
                    && path
                        .file_name()
                        .is_some_and(|name| name != "recording.obsr")
            });
        if path.is_some() {
            path
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
            None
        }
    });
    let replay_path = replay_path.expect("replay save should commit asynchronously");
    let bytes = std::fs::read(&replay_path).expect("replay file");
    assert!(!bytes.is_empty());
    assert!(
        ui.get_replay_buffering(),
        "saving a replay must not stop the capture history"
    );

    ui.invoke_toggle_replay_buffer();
    for _ in 0..100 {
        crate::refresh_output_ui(ui, &output);
        if !ui.get_replay_buffering() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !ui.get_replay_buffering(),
        "Replay control must stop capture"
    );
    std::fs::remove_dir_all(directory).expect("remove replay fixture directory");
}

/// Checks the desktop stops claiming an output the engine is not running.
///
/// The controls set their booleans optimistically, so a start the engine
/// refused would otherwise leave the window showing "recording" forever.
fn exercise_output_reconciliation(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    // The engine is idle here, so both claims are stale by construction.
    state
        .borrow_mut()
        .dispatch(UiCommand::StartRecording)
        .expect("claim recording");
    state
        .borrow_mut()
        .dispatch(UiCommand::StartStreaming)
        .expect("claim streaming");
    ui.set_recording(true);
    ui.set_streaming(true);

    crate::callbacks::reconcile_output_lifecycle(ui, state, output);

    assert!(
        !state.borrow().recording(),
        "a recording the engine never opened must not stay claimed"
    );
    assert!(
        !state.borrow().streaming(),
        "a stream the engine never opened must not stay claimed"
    );
    assert!(!ui.get_recording() && !ui.get_streaming());
}
