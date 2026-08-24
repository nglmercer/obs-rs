use super::*;

#[test]
fn desktop_state_persists_project_editor_changes() {
    let final_path = std::env::temp_dir().join(format!(
        "obs-rs-ui-persistence-{}.project",
        std::process::id()
    ));
    let temp_path = final_path.with_file_name("obs-rs-ui-persistence.project.tmp");
    let store = ProjectFileStore::new(&final_path, &temp_path).expect("project store");
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::Project(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("studio", "Studio").expect("scene"),
        }))
        .expect("add scene");
    assert!(state.is_dirty());
    let document = state.project_document();

    let bytes = state.save_project(&store).expect("save project");
    assert_eq!(bytes, document.len());
    assert!(!state.is_dirty());

    let mut loaded = DesktopState::new(project());
    loaded.load_project(&store).expect("load project");
    assert_eq!(loaded.project_document(), document);
    assert!(!loaded.is_dirty());
    assert_eq!(loaded.preview_scene(), Some("preview"));
    assert!(!temp_path.exists());

    std::fs::remove_file(final_path).expect("remove project fixture");
}

#[test]
fn keyed_project_switch_restores_each_documents_scene_selection() {
    let first_path = std::env::temp_dir().join(format!(
        "obs-rs-ui-selection-first-{}.project",
        std::process::id()
    ));
    let first_temp = first_path.with_file_name("obs-rs-ui-selection-first.project.tmp");
    let second_path = std::env::temp_dir().join(format!(
        "obs-rs-ui-selection-second-{}.project",
        std::process::id()
    ));
    let second_temp = second_path.with_file_name("obs-rs-ui-selection-second.project.tmp");
    let first_store = ProjectFileStore::new(&first_path, &first_temp).expect("first store");
    let second_store = ProjectFileStore::new(&second_path, &second_temp).expect("second store");

    let mut first_seed = DesktopState::new(project());
    first_seed
        .save_project(&first_store)
        .expect("save first project");
    let mut second_seed = DesktopState::new(project());
    second_seed
        .save_project(&second_store)
        .expect("save second project");

    let first_key = first_path.to_string_lossy().into_owned();
    let second_key = second_path.to_string_lossy().into_owned();
    let mut state = DesktopState::new(project());
    state.set_project_selection_key(&first_key);
    state
        .load_project_for_key(&first_store, &first_key)
        .expect("load first project");
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("first preview selection");
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("first program selection");

    state
        .load_project_for_key(&second_store, &second_key)
        .expect("load second project");
    assert_eq!(state.preview_scene(), Some("preview"));
    assert_eq!(state.program_scene(), Some("preview"));
    state
        .load_project_for_key(&first_store, &first_key)
        .expect("return to first project");
    assert_eq!(state.preview_scene(), Some("source_scene"));
    assert_eq!(state.program_scene(), Some("program"));

    std::fs::remove_file(first_path).expect("remove first project fixture");
    std::fs::remove_file(second_path).expect("remove second project fixture");
}

#[test]
fn project_scene_selection_snapshots_restore_after_a_restart() {
    let key = "/tmp/obs-rs-persisted-selection.obsrproj";
    let mut state = DesktopState::new(project());
    state.set_project_selection_key(key);
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("preview selection");
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("program selection");
    let snapshots = state.project_scene_selections();

    let mut restarted = DesktopState::new(project());
    restarted.set_project_selection_key(key);
    restarted.restore_project_selections(&snapshots);
    restarted.restore_project_selection_for_current_key();

    assert_eq!(restarted.preview_scene(), Some("source_scene"));
    assert_eq!(restarted.program_scene(), Some("program"));
}

#[test]
fn project_scene_selection_snapshots_restore_each_profile_after_a_restart() {
    let mut project = project();
    let format = project
        .profile("live")
        .expect("live profile")
        .video_format();
    let mut alternate = Profile::new("alternate", "Alternate", format).expect("profile");
    alternate
        .add_scene(SceneSpec::new("alternate_preview", "Alternate preview").expect("scene"))
        .expect("scene");
    alternate
        .add_scene(SceneSpec::new("alternate_program", "Alternate program").expect("scene"))
        .expect("scene");
    project.add_profile(alternate).expect("profile");

    let key = "/tmp/obs-rs-persisted-profile-selections.obsrproj";
    let mut state = DesktopState::new(project);
    state.set_project_selection_key(key);
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("live preview selection");
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("live program selection");
    state
        .dispatch(UiCommand::SelectProfile {
            id: "alternate".to_owned(),
        })
        .expect("alternate profile selection");
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "alternate_program".to_owned(),
        })
        .expect("alternate program selection");
    state
        .dispatch(UiCommand::SelectProfile {
            id: "live".to_owned(),
        })
        .expect("return to live profile");

    let snapshots = state.project_scene_selections();
    assert_eq!(
        snapshots
            .iter()
            .filter(|snapshot| snapshot.key() == key)
            .count(),
        2
    );
    let restart_project = state.project_session().project().clone();
    let mut restarted = DesktopState::new(restart_project);
    restarted.set_project_selection_key(key);
    restarted.restore_project_selections(&snapshots);
    restarted.restore_project_selection_for_current_key();

    assert_eq!(restarted.preview_scene(), Some("source_scene"));
    assert_eq!(restarted.program_scene(), Some("program"));
    restarted
        .dispatch(UiCommand::SelectProfile {
            id: "alternate".to_owned(),
        })
        .expect("restore alternate profile");
    assert_eq!(restarted.preview_scene(), Some("alternate_preview"));
    assert_eq!(restarted.program_scene(), Some("alternate_program"));
}

#[test]
fn console_parser_covers_state_and_output_commands() {
    assert_eq!(
        parse_console_command("preview program"),
        Ok(ConsoleCommand::Apply(UiCommand::SelectPreviewScene {
            id: "program".to_owned(),
        }))
    );
    assert_eq!(
        parse_console_command("record start"),
        Ok(ConsoleCommand::Apply(UiCommand::StartRecording))
    );
    assert_eq!(
        parse_console_command("transition fade 500"),
        Ok(ConsoleCommand::Apply(UiCommand::SetTransition {
            transition: FrameTransition::CrossFade {
                progress_milli: 500,
            },
        }))
    );
    assert_eq!(
        parse_console_command("take fade 500"),
        Ok(ConsoleCommand::Apply(UiCommand::TakePreview {
            transition: FrameTransition::CrossFade {
                progress_milli: 500,
            },
            duration_ms: DEFAULT_TRANSITION_DURATION_MILLIS,
        }))
    );
    assert_eq!(
        parse_console_command("transition color 500 #00FF00"),
        Ok(ConsoleCommand::Apply(UiCommand::SetTransition {
            transition: FrameTransition::FadeToColor {
                progress_milli: 500,
                color: [0, 255, 0, 255],
            },
        }))
    );
    assert_eq!(
        parse_console_command("take color 750 #0000FF80"),
        Ok(ConsoleCommand::Apply(UiCommand::TakePreview {
            transition: FrameTransition::FadeToColor {
                progress_milli: 750,
                color: [0, 0, 255, 128],
            },
            duration_ms: DEFAULT_TRANSITION_DURATION_MILLIS,
        }))
    );
    assert_eq!(
        parse_console_command("mixer desktop gain 1500"),
        Ok(ConsoleCommand::Apply(UiCommand::SetMixerGain {
            id: "desktop".to_owned(),
            gain_milli: 1_500,
        }))
    );
    assert_eq!(
        parse_console_command("mixer mic mute"),
        Ok(ConsoleCommand::Apply(UiCommand::ToggleMixerMute {
            id: "mic".to_owned(),
        }))
    );
    assert_eq!(
        parse_console_command("not-a-command"),
        Err(ConsoleCommandError::UnknownCommand(
            "not-a-command".to_owned()
        ))
    );
    assert_eq!(
        parse_console_command("transition fade 1001"),
        Err(ConsoleCommandError::InvalidTransition(
            MediaError::InvalidTransition {
                progress_milli: 1_001,
            },
        ))
    );
    assert!(matches!(
        parse_console_command("transition color 500 green"),
        Err(ConsoleCommandError::InvalidArgument {
            command: "transition",
            ..
        })
    ));
}

#[test]
fn console_commands_drive_desktop_state_without_duplicate_logic() {
    let mut state = DesktopState::new(project());
    for line in ["program program", "swap", "record start", "stream start"] {
        let command = parse_console_command(line).expect("console command");
        if let ConsoleCommand::Apply(command) = command {
            state.dispatch(command).expect("state command");
        }
    }

    assert_eq!(state.preview_scene(), Some("program"));
    assert_eq!(state.program_scene(), Some("preview"));
    assert!(state.recording());
    assert!(state.streaming());
}

#[test]
fn web_request_parser_routes_bounded_browser_commands() {
    assert_eq!(
        parse_web_request(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        Ok(WebRoute::Home)
    );
    assert_eq!(
        parse_web_request(b"GET /snapshot HTTP/1.1\r\n\r\n"),
        Ok(WebRoute::Snapshot)
    );
    let body = "transition fade 500";
    let request = format!(
        "POST /command HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(
        parse_web_request(request.as_bytes()),
        Ok(WebRoute::Command(body.to_owned()))
    );
    assert_eq!(
        parse_web_request(b"POST /command HTTP/1.1\r\nContent-Length: 2\r\n\r\nswap"),
        Err(WebRequestError::ContentLengthMismatch {
            expected: 2,
            actual: 4
        })
    );
    assert_eq!(
        parse_console_command("language es"),
        Ok(ConsoleCommand::Apply(UiCommand::SetLocale {
            locale: UiLocale::Spanish
        }))
    );
    assert_eq!(
        parse_web_request(b"DELETE / HTTP/1.1\r\n\r\n"),
        Err(WebRequestError::UnsupportedMethod("DELETE".to_owned()))
    );
}

#[test]
fn localized_snapshot_uses_the_selected_language() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SetLocale {
            locale: UiLocale::Spanish,
        })
        .expect("locale selection");
    let snapshot = state.accessible_snapshot();
    assert!(snapshot.contains("Proyecto:"));
    assert!(snapshot.contains("Mezclador de audio:"));
    assert!(snapshot.contains("(es)"));
}

#[test]
fn web_page_is_accessible_and_escapes_snapshot_text() {
    let state = DesktopState::new(project());
    let page = state.web_page();
    assert!(page.contains("<main id=\"main\""));
    assert!(page.contains("aria-live=\"polite\""));
    assert!(page.contains("data-command=\"swap\""));
    assert!(page.contains("data-command=\"take fade 500\""));
    assert!(page.contains("OBS-RS desktop state"));
    let mut spanish = DesktopState::new(project());
    spanish
        .dispatch(UiCommand::SetLocale {
            locale: UiLocale::Spanish,
        })
        .expect("locale selection");
    let spanish_page = spanish.web_page();
    assert!(spanish_page.contains("<html lang=\"es\">"));
    assert!(spanish_page.contains("Estado actual"));
    assert_eq!(escape_html("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
    assert_eq!(
        parse_web_request(&vec![b'x'; MAX_WEB_REQUEST_BYTES + 1]),
        Err(WebRequestError::TooLarge)
    );
}

#[test]
fn accessible_snapshot_contains_labeled_state_and_scene_markers() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("program selection");
    state
        .dispatch(UiCommand::StartRecording)
        .expect("recording start");
    let snapshot = state.accessible_snapshot();

    assert!(snapshot.contains("OBS-RS desktop state"));
    assert!(snapshot.contains("Preview scene: preview"));
    assert!(snapshot.contains("Program scene: program"));
    assert!(snapshot.contains("Recording: active"));
    assert!(snapshot.contains("- preview: Preview [preview]"));
    assert!(snapshot.contains("- program: Program [program]"));
    assert!(snapshot.contains("Recent notices:"));
}

#[test]
fn locale_parser_accepts_case_and_region_variants() {
    assert_eq!(UiLocale::supported().len(), 2);
    assert_eq!(UiLocale::from_code("EN_us"), Some(UiLocale::English));
    assert_eq!(UiLocale::from_code("es-ES"), Some(UiLocale::Spanish));
    assert_eq!(UiLocale::from_code("  english  "), Some(UiLocale::English));
    assert_eq!(UiLocale::from_code("fr-FR"), None);
}

#[test]
fn set_audio_format_rebuilds_the_mixer_and_keeps_channel_state() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SetMixerGain {
            id: "mic".to_owned(),
            gain_milli: 250,
        })
        .expect("gain applies");
    state
        .dispatch(UiCommand::SetMixerPan {
            id: "mic".to_owned(),
            pan_milli: 500,
        })
        .expect("pan applies");
    state
        .dispatch(UiCommand::ToggleMixerMute {
            id: "desktop".to_owned(),
        })
        .expect("mute applies");

    state
        .dispatch(UiCommand::SetAudioFormat {
            sample_rate: 44_100,
            channels: 1,
        })
        .expect("audio format applies");

    assert_eq!(state.audio_format().sample_rate(), 44_100);
    assert_eq!(state.audio_format().channels(), 1);
    let channels = state.mixer_channels().collect::<Vec<_>>();
    let desktop = channels
        .iter()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop channel survives the rebuild");
    let mic = channels
        .iter()
        .find(|channel| channel.id() == "mic")
        .expect("mic channel survives the rebuild");
    assert!(desktop.muted());
    assert_eq!(mic.gain_milli(), 250);
    assert_eq!(mic.pan_milli(), 500);

    // The rebuilt mixer still resolves the same UI channel labels.
    state
        .dispatch(UiCommand::SetMixerGain {
            id: "mic".to_owned(),
            gain_milli: 900,
        })
        .expect("gain still applies after the rebuild");
}

#[test]
fn set_audio_format_rejects_an_unsupported_format() {
    let mut state = DesktopState::new(project());

    let error = state
        .dispatch(UiCommand::SetAudioFormat {
            sample_rate: 0,
            channels: 2,
        })
        .expect_err("a zero sample rate is rejected");

    assert!(matches!(error, UiError::Audio(_)));
    assert_eq!(state.audio_format().sample_rate(), 48_000);
}

#[test]
fn undo_and_redo_reverse_project_edits_and_resync_selections() {
    let mut state = DesktopState::new(project());
    assert!(!state.can_undo());

    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("select the scene holding the source");
    state
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "source_scene".to_owned(),
            item: "source".to_owned(),
        }))
        .expect("remove source");
    assert_eq!(state.selected_source(), None);
    assert!(state.can_undo());

    state.dispatch(UiCommand::Undo).expect("undo");

    assert!(state
        .project_session()
        .project()
        .profile("live")
        .expect("profile")
        .scene("source_scene")
        .expect("scene")
        .has_item("source"));
    // The selection is reconciled against the restored project rather than
    // being left pointing at whatever the removal fell back to.
    assert_eq!(state.selected_source(), Some("source"));
    assert!(state.can_redo());

    state.dispatch(UiCommand::Redo).expect("redo");
    assert_eq!(state.selected_source(), None);
}

#[test]
fn undo_at_the_history_bottom_is_a_reported_no_op() {
    let mut state = DesktopState::new(project());

    state
        .dispatch(UiCommand::Undo)
        .expect("an empty history is not an error");

    assert_eq!(
        state.notices().last().map(UiNotice::message),
        Some("nothing to undo")
    );
    assert!(!state.is_dirty());
}
