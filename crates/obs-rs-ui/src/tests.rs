use super::*;
use crate::helpers::escape_html;
use obs_rs_audio::AudioBuffer;
use obs_rs_audio::AudioFormat;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, FrameTransition, MediaError, Timestamp, VideoFormat};
use obs_rs_project::{
    Profile, Project, ProjectCommand, ProjectFileStore, SceneItemDuplicateMode, SceneItemSpec,
    SceneSpec, SourceSpec,
};

fn project() -> Project {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut project = Project::new("UI fixture").expect("project");
    let mut profile = Profile::new("live", "Live", format).expect("profile");
    profile
        .add_scene(SceneSpec::new("preview", "Preview").expect("scene"))
        .expect("scene");
    profile
        .add_scene(SceneSpec::new("program", "Program").expect("scene"))
        .expect("scene");
    let mut source_scene = SceneSpec::new("source_scene", "Source").expect("scene");
    source_scene
        .add_item(SceneItemSpec::for_source("source").expect("scene item"))
        .expect("item");
    profile
        .add_source(
            SourceSpec::new("source", "color_source", "Color", Config::new()).expect("source"),
        )
        .expect("source registry");
    profile.add_scene(source_scene).expect("scene");
    project.add_profile(profile).expect("profile");
    project
}

#[test]
fn desktop_state_selects_scenes_and_tracks_outputs() {
    let mut state = DesktopState::new(project());
    assert_eq!(state.preview_scene(), Some("preview"));
    assert_eq!(state.program_scene(), Some("preview"));
    state
        .dispatch(UiCommand::SelectProgramScene {
            id: "program".to_owned(),
        })
        .expect("program selection");
    state
        .dispatch(UiCommand::StartRecording)
        .expect("recording start");
    assert!(state.recording());
    assert!(!state.is_dirty());
    assert_eq!(state.notices().count(), 2);
}

#[test]
fn desktop_state_selects_source_items_in_preview_scene() {
    let mut state = DesktopState::new(project());
    assert_eq!(state.selected_source(), None);
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("source scene selection");
    assert_eq!(state.selected_source(), Some("source"));
    state
        .dispatch(UiCommand::SelectSource {
            id: "source".to_owned(),
        })
        .expect("source selection");
    assert_eq!(state.selected_source(), Some("source"));
}

#[test]
fn desktop_state_supports_bounded_multi_selection_and_active_item() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("source scene selection");
    state
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "source_scene".to_owned(),
            source: SourceSpec::new("second", "color_source", "Second", Config::new())
                .expect("second source"),
        }))
        .expect("second item");
    state
        .dispatch(UiCommand::SelectSources {
            ids: vec!["source".to_owned(), "second".to_owned()],
            additive: false,
        })
        .expect("multi-selection");
    assert_eq!(
        state.selected_sources().collect::<Vec<_>>(),
        vec!["source", "second"]
    );
    assert_eq!(state.selected_source(), Some("second"));
    state
        .dispatch(UiCommand::ToggleSourceSelection {
            id: "source".to_owned(),
        })
        .expect("toggle selection");
    assert_eq!(state.selected_sources().collect::<Vec<_>>(), vec!["second"]);
    state
        .dispatch(UiCommand::SelectSources {
            ids: Vec::new(),
            additive: false,
        })
        .expect("clear selection");
    assert_eq!(state.selected_source(), None);
}

#[test]
fn desktop_state_copy_and_paste_support_reference_and_duplicate_modes() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("source scene selection");
    state
        .dispatch(UiCommand::CopySource {
            id: "source".to_owned(),
        })
        .expect("copy source item");
    assert!(state.can_paste_source());

    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "preview".to_owned(),
        })
        .expect("target scene selection");
    state
        .dispatch(UiCommand::PasteSource {
            mode: SceneItemDuplicateMode::Reference,
            target: String::new(),
        })
        .expect("reference paste");
    assert_eq!(state.selected_source(), Some("source"));
    let profile = state
        .project_session()
        .project()
        .profile("live")
        .expect("profile");
    assert_eq!(
        profile
            .scene("preview")
            .expect("preview")
            .item("source")
            .expect("reference item")
            .source_id()
            .as_str(),
        "source"
    );

    state
        .dispatch(UiCommand::PasteSource {
            mode: SceneItemDuplicateMode::DuplicateSource,
            target: String::new(),
        })
        .expect("duplicate paste");
    let profile = state
        .project_session()
        .project()
        .profile("live")
        .expect("profile");
    let duplicate = profile
        .scene("preview")
        .expect("preview")
        .item("source_copy")
        .expect("duplicate item");
    assert_ne!(duplicate.source_id().as_str(), "source");
    assert!(profile.source(duplicate.source_id()).is_some());
}

#[test]
fn desktop_state_copies_and_pastes_nested_group_items_by_target() {
    let mut project = project();
    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::new("nested-source", "source").expect("group child"))
        .expect("group child attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        })
        .expect("group add");

    let mut state = DesktopState::new(project);
    state
        .dispatch(UiCommand::CopySource {
            id: "overlay-group/nested-source".to_owned(),
        })
        .expect("nested copy");
    state
        .dispatch(UiCommand::PasteSource {
            mode: SceneItemDuplicateMode::Reference,
            target: "overlay-group".to_owned(),
        })
        .expect("nested reference paste");
    state
        .dispatch(UiCommand::PasteSource {
            mode: SceneItemDuplicateMode::DuplicateSource,
            target: "overlay-group/nested-source".to_owned(),
        })
        .expect("nested duplicate paste");

    let profile = state
        .project_session()
        .project()
        .profile("live")
        .expect("profile");
    let group = profile
        .scene("preview")
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group");
    assert_eq!(
        group
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(obs_rs_util::Identifier::as_str)
            .collect::<Vec<_>>(),
        vec![
            "nested-source",
            "nested-source_copy",
            "nested-source_copy_2"
        ]
    );
    assert_eq!(
        group.items()[2].source_id().as_str(),
        "source_copy",
        "duplicate paste clones the profile source"
    );
}

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
        })
        .expect("take preview");
    assert_eq!(state.program_scene(), state.preview_scene());
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

    let desktop = state
        .mixer_channels()
        .find(|channel| channel.id() == "desktop")
        .expect("desktop mixer channel");
    assert_eq!(desktop.gain_milli(), 1_500);
    assert!(desktop.muted());
    assert_eq!(desktop.peak_milli(), 0);
    assert_eq!(
        state.dispatch(UiCommand::SetMixerGain {
            id: "desktop".to_owned(),
            gain_milli: 2_001,
        }),
        Err(UiError::InvalidMixerGain(2_001))
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
}

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
