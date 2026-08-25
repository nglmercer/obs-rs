use super::*;

#[test]
fn shortcuts_trigger_actions_and_reject_duplicates() {
    let mut state = DesktopState::new(project());
    let shortcut = Shortcut::new(1, "F9").expect("shortcut");
    state
        .dispatch(UiCommand::BindShortcut {
            shortcut: shortcut.clone(),
            action: UiAction::StartStreaming,
        })
        .expect("bind");
    assert_eq!(
        state.shortcut_action(&shortcut),
        Some(UiAction::StartStreaming)
    );
    assert_eq!(
        state.dispatch(UiCommand::BindShortcut {
            shortcut: shortcut.clone(),
            action: UiAction::StopStreaming,
        }),
        Err(UiError::DuplicateShortcut(shortcut.clone()))
    );
    state
        .dispatch(UiCommand::TriggerShortcut { shortcut })
        .expect("trigger");
    assert!(state.streaming());
}

#[test]
fn scene_navigation_shortcuts_follow_persistent_order_and_wrap() {
    let mut state = DesktopState::new(project());
    assert_eq!(state.preview_scene(), Some("preview"));

    state
        .dispatch_action(UiAction::NextPreviewScene)
        .expect("next preview scene");
    assert_eq!(state.preview_scene(), Some("program"));

    state
        .dispatch(UiCommand::SelectAdjacentPreviewScene { direction: 1 })
        .expect("next preview scene should wrap");
    assert_eq!(state.preview_scene(), Some("source_scene"));

    state
        .dispatch_action(UiAction::PreviousPreviewScene)
        .expect("previous preview scene");
    assert_eq!(state.preview_scene(), Some("program"));

    assert_eq!(
        state.dispatch(UiCommand::SelectAdjacentPreviewScene { direction: 0 }),
        Err(UiError::InvalidSceneNavigation(0))
    );
}

#[test]
fn scene_dock_navigation_selects_edges_without_changing_persistent_order() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectAdjacentPreviewScene { direction: 2 })
        .expect("last preview scene");
    assert_eq!(state.preview_scene(), Some("source_scene"));

    state
        .dispatch(UiCommand::SelectAdjacentPreviewScene { direction: -2 })
        .expect("first preview scene");
    assert_eq!(state.preview_scene(), Some("preview"));
    assert_eq!(
        state
            .project_session()
            .project()
            .active_profile_spec()
            .expect("active profile")
            .scene_order()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["preview", "program", "source_scene"]
    );
    assert_eq!(
        state.dispatch(UiCommand::SelectAdjacentPreviewScene { direction: 3 }),
        Err(UiError::InvalidSceneNavigation(3))
    );
}

#[test]
fn shortcut_table_replaces_atomically_and_routes_frontend_actions() {
    let mut state = DesktopState::new(project());
    let old = Shortcut::new(0, "F8").expect("old shortcut");
    state
        .dispatch(UiCommand::BindShortcut {
            shortcut: old.clone(),
            action: UiAction::Undo,
        })
        .expect("initial bind");

    let save = Shortcut::new(1, "S").expect("save shortcut");
    state
        .replace_shortcuts(&[(save.clone(), UiAction::SaveProject)])
        .expect("replace shortcuts");
    assert_eq!(state.shortcut_action(&old), None);
    assert_eq!(state.shortcut_action(&save), Some(UiAction::SaveProject));
    assert_eq!(
        state.dispatch(UiCommand::TriggerShortcut {
            shortcut: save.clone(),
        }),
        Err(UiError::FrontendActionRequired(UiAction::SaveProject))
    );

    let duplicate = Shortcut::new(1, "S").expect("duplicate shortcut");
    let result = state.replace_shortcuts(&[
        (
            Shortcut::new(0, "F9").expect("new shortcut"),
            UiAction::Redo,
        ),
        (duplicate, UiAction::FadeTransition),
        (
            Shortcut::new(1, "S").expect("duplicate shortcut"),
            UiAction::Undo,
        ),
    ]);
    assert!(matches!(result, Err(UiError::DuplicateShortcut(_))));
    assert_eq!(
        state.shortcut_action(&save),
        Some(UiAction::SaveProject),
        "a rejected replacement must not partially update the live map"
    );
}

#[test]
fn save_replay_shortcut_is_a_frontend_action() {
    let mut state = DesktopState::new(project());
    let shortcut = Shortcut::new(0, "F8").expect("replay shortcut");
    state
        .replace_shortcuts(&[(shortcut.clone(), UiAction::SaveReplayBuffer)])
        .expect("replay shortcut table");
    assert_eq!(
        state.shortcut_action(&shortcut),
        Some(UiAction::SaveReplayBuffer)
    );
    assert_eq!(
        state.dispatch(UiCommand::TriggerShortcut { shortcut }),
        Err(UiError::FrontendActionRequired(UiAction::SaveReplayBuffer))
    );
}

#[test]
fn cut_transition_shortcut_is_a_frontend_action() {
    let mut state = DesktopState::new(project());
    let shortcut = Shortcut::new(0, "T").expect("cut shortcut");
    state
        .replace_shortcuts(&[(shortcut.clone(), UiAction::CutTransition)])
        .expect("cut shortcut table");
    assert_eq!(
        state.shortcut_action(&shortcut),
        Some(UiAction::CutTransition)
    );
    assert_eq!(
        state.dispatch(UiCommand::TriggerShortcut { shortcut }),
        Err(UiError::FrontendActionRequired(UiAction::CutTransition))
    );
}

#[test]
fn replay_start_and_stop_shortcuts_are_frontend_actions() {
    let mut state = DesktopState::new(project());
    let start = Shortcut::new(0, "F9").expect("shortcut");
    let stop = Shortcut::new(0, "F10").expect("shortcut");
    state
        .replace_shortcuts(&[
            (start.clone(), UiAction::StartReplayBuffer),
            (stop.clone(), UiAction::StopReplayBuffer),
        ])
        .expect("replay shortcuts");

    assert_eq!(
        state.shortcut_action(&start),
        Some(UiAction::StartReplayBuffer)
    );
    assert_eq!(
        state.shortcut_action(&stop),
        Some(UiAction::StopReplayBuffer)
    );
    assert_eq!(
        state.dispatch_action(UiAction::StartReplayBuffer),
        Err(UiError::FrontendActionRequired(UiAction::StartReplayBuffer))
    );
    assert_eq!(
        state.dispatch_action(UiAction::StopReplayBuffer),
        Err(UiError::FrontendActionRequired(UiAction::StopReplayBuffer))
    );
}

#[test]
fn microphone_and_desktop_mute_shortcuts_toggle_their_mixer_channels() {
    let mut state = DesktopState::new(project());
    let microphone = Shortcut::new(0, "M").expect("microphone shortcut");
    let desktop = Shortcut::new(1, "M").expect("desktop shortcut");
    state
        .replace_shortcuts(&[
            (microphone.clone(), UiAction::ToggleMicrophoneMute),
            (desktop.clone(), UiAction::ToggleDesktopMute),
        ])
        .expect("mute shortcut table");

    state
        .dispatch(UiCommand::TriggerShortcut {
            shortcut: microphone,
        })
        .expect("microphone mute shortcut");
    state
        .dispatch(UiCommand::TriggerShortcut { shortcut: desktop })
        .expect("desktop mute shortcut");

    let channels = state.mixer_channels().collect::<Vec<_>>();
    assert!(channels
        .iter()
        .find(|channel| channel.id() == "mic")
        .expect("microphone mixer channel")
        .muted());
    assert!(channels
        .iter()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop mixer channel")
        .muted());
}

#[test]
fn shortcut_text_is_bounded_and_canonical() {
    let shortcut = Shortcut::parse(" option + shift + f9 ")
        .expect("shortcut syntax")
        .expect("shortcut is bound");
    assert_eq!(shortcut.modifiers(), Shortcut::SHIFT | Shortcut::ALT);
    assert_eq!(shortcut.key(), "F9");
    assert_eq!(shortcut.to_string(), "Shift+Alt+F9");

    assert_eq!(Shortcut::parse(""), Ok(None));
    assert_eq!(
        Shortcut::parse("Ctrl+Ctrl+R"),
        Err(UiError::InvalidShortcut)
    );
    assert_eq!(
        Shortcut::parse("Ctrl+R+Shift"),
        Err(UiError::InvalidShortcut)
    );
    assert_eq!(Shortcut::parse("Ctrl+"), Err(UiError::InvalidShortcut));
    assert_eq!(
        Shortcut::parse(&"A".repeat(MAX_SHORTCUT_TEXT_BYTES + 1)),
        Err(UiError::InvalidShortcut)
    );
}

#[test]
fn project_commands_keep_dirty_state_and_transitions_validate() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::Project(ProjectCommand::SetActiveProfile {
            id: "live".to_owned(),
        }))
        .expect("project command");
    assert!(state.is_dirty());
    state
        .dispatch(UiCommand::SetTransition {
            transition: FrameTransition::cross_fade(500).expect("transition"),
        })
        .expect("transition");
    assert_eq!(
        state.transition(),
        FrameTransition::CrossFade {
            progress_milli: 500
        }
    );
    assert_eq!(
        state.dispatch(UiCommand::SetTransition {
            transition: FrameTransition::CrossFade {
                progress_milli: 1_001
            },
        }),
        Err(UiError::Media(MediaError::InvalidTransition {
            progress_milli: 1_001
        }))
    );
    state
        .dispatch(UiCommand::TakePreview {
            transition: FrameTransition::Cut,
            duration_ms: DEFAULT_TRANSITION_DURATION_MILLIS,
        })
        .expect("take preview");
    assert_eq!(state.program_scene(), state.preview_scene());
}

#[test]
fn preview_scene_transition_override_is_persisted_and_used_by_take() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("program scene");
    let override_spec = TransitionSpec::fade_to_color(450, [0, 255, 0, 128]).expect("transition");
    state
        .dispatch(UiCommand::SetPreviewSceneTransition {
            transition: Some(override_spec),
        })
        .expect("set preview scene override");

    let persisted = Project::parse(&state.project_document()).expect("persisted project");
    assert_eq!(
        persisted
            .profile("live")
            .and_then(|profile| profile.scene("preview"))
            .and_then(SceneSpec::transition_override),
        Some(override_spec)
    );

    state
        .dispatch(UiCommand::TakePreview {
            transition: FrameTransition::Cut,
            duration_ms: 1,
        })
        .expect("take preview");
    assert_eq!(
        state.transition(),
        FrameTransition::FadeToColor {
            progress_milli: 500,
            color: [0, 255, 0, 128],
        }
    );
    assert!(matches!(
        state
            .transition_snapshot(Instant::now())
            .expect("active transition")
            .transition(),
        FrameTransition::FadeToColor {
            progress_milli,
            color: [0, 255, 0, 128],
        } if progress_milli < 1_000
    ));
}

#[test]
fn take_preview_exposes_a_bounded_transient_transition() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("program scene");
    state
        .dispatch(UiCommand::TakePreview {
            transition: FrameTransition::CrossFade {
                progress_milli: 500,
            },
            duration_ms: 100,
        })
        .expect("take preview");

    let snapshot = state
        .transition_snapshot(Instant::now())
        .expect("transition is active");
    assert_eq!(snapshot.source_scene(), "program");
    assert_eq!(snapshot.destination_scene(), "preview");
    assert!(matches!(
        snapshot.transition(),
        FrameTransition::CrossFade { progress_milli } if progress_milli < 1_000
    ));

    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("restore program scene");
    state
        .dispatch(UiCommand::TakePreview {
            transition: FrameTransition::CrossFade {
                progress_milli: 500,
            },
            duration_ms: 1,
        })
        .expect("short take preview");
    std::thread::sleep(Duration::from_millis(5));
    assert!(
        state.transition_snapshot(Instant::now()).is_none(),
        "the transient transition retires at its duration boundary"
    );
    assert_eq!(
        state.dispatch(UiCommand::TakePreview {
            transition: FrameTransition::Cut,
            duration_ms: 0,
        }),
        Err(UiError::InvalidTransitionDuration(0))
    );
}

#[test]
fn mixer_commands_update_real_audio_controls() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SetMixerGain {
            id: "desktop".to_owned(),
            gain_milli: 1_500,
        })
        .expect("mixer gain");
    state
        .dispatch(UiCommand::ToggleMixerMute {
            id: "desktop".to_owned(),
        })
        .expect("mixer mute");
    state
        .dispatch(UiCommand::SetMixerPan {
            id: "desktop".to_owned(),
            pan_milli: -750,
        })
        .expect("mixer pan");

    let desktop = state
        .mixer_channels()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop mixer channel");
    assert_eq!(desktop.gain_milli(), 1_500);
    assert_eq!(desktop.pan_milli(), -750);
    assert!(desktop.muted());
    assert_eq!(desktop.peak_milli(), 0);
    assert_eq!(
        state.dispatch(UiCommand::SetMixerGain {
            id: "desktop".to_owned(),
            gain_milli: 2_001,
        }),
        Err(UiError::InvalidMixerGain(2_001))
    );
    assert_eq!(
        state.dispatch(UiCommand::SetMixerPan {
            id: "desktop".to_owned(),
            pan_milli: 1_001,
        }),
        Err(UiError::InvalidMixerPan(1_001))
    );
}

#[test]
fn mixer_updates_visible_peak_meters_from_real_audio() {
    let mut state = DesktopState::new(project());
    let format = AudioFormat::new(48_000, 2).expect("audio format");
    let input = AudioBuffer::new(format, Timestamp::ZERO, vec![0.75; 8]).expect("audio input");
    let output = state
        .mix_audio(Timestamp::ZERO, 4, &[("desktop", &input)])
        .expect("audio mix");
    assert_eq!(output.samples(), &[0.75; 8]);
    assert_eq!(
        state
            .mixer_channels()
            .find(|channel| channel.id() == "desktop")
            .expect("desktop channel")
            .peak_milli(),
        750
    );
    assert_eq!(
        state
            .mixer_channels()
            .find(|channel| channel.id() == "desktop")
            .expect("desktop channel")
            .peak_hold_milli(),
        750
    );
    assert!(!state
        .mixer_channels()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop channel")
        .clipped());
}

#[test]
fn mixer_meter_state_exposes_clip_flash_and_peak_hold() {
    let mut state = DesktopState::new(project());
    let loud = AudioBuffer::new(
        AudioFormat::new(48_000, 2).expect("audio format"),
        Timestamp::ZERO,
        vec![1.5; 8],
    )
    .expect("loud input");
    state
        .mix_audio(Timestamp::ZERO, 4, &[("desktop", &loud)])
        .expect("loud mix");
    let desktop = state
        .mixer_channels()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop channel");
    assert_eq!(desktop.peak_milli(), 1_000);
    assert_eq!(desktop.peak_hold_milli(), 1_000);
    assert!(desktop.clipped());

    let quiet = AudioBuffer::new(
        AudioFormat::new(48_000, 2).expect("audio format"),
        Timestamp::from_millis(500),
        vec![0.1; 8],
    )
    .expect("quiet input");
    state
        .mix_audio(Timestamp::from_millis(500), 4, &[("desktop", &quiet)])
        .expect("quiet mix");
    let desktop = state
        .mixer_channels()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop channel");
    assert_eq!(desktop.peak_hold_milli(), 1_000);
    assert!(desktop.clipped());
}
