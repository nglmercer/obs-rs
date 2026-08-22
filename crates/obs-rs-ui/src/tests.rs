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
fn persisted_scene_selection_restores_without_project_history() {
    let mut state = DesktopState::new(project());
    state.restore_scene_selection(Some("source_scene"), Some("program"));

    assert_eq!(state.preview_scene(), Some("source_scene"));
    assert_eq!(state.program_scene(), Some("program"));
    assert_eq!(state.selected_source(), Some("source"));

    // Stale settings do not displace the valid fallback and do not create a
    // user-visible project edit.
    state.restore_scene_selection(Some("missing"), Some("bad id"));
    assert_eq!(state.preview_scene(), Some("source_scene"));
    assert_eq!(state.program_scene(), Some("program"));
    assert!(!state.can_undo());
}

#[test]
fn profile_switch_restores_each_profiles_scene_choices() {
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

    let mut state = DesktopState::new(project);
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
    assert_eq!(state.preview_scene(), Some("alternate_preview"));
    assert_eq!(state.program_scene(), Some("alternate_preview"));

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
    assert_eq!(state.preview_scene(), Some("source_scene"));
    assert_eq!(state.program_scene(), Some("program"));

    state
        .dispatch(UiCommand::SelectProfile {
            id: "alternate".to_owned(),
        })
        .expect("return to alternate profile");
    assert_eq!(state.preview_scene(), Some("alternate_preview"));
    assert_eq!(state.program_scene(), Some("alternate_program"));

    state
        .dispatch(UiCommand::SelectProfile {
            id: "live".to_owned(),
        })
        .expect("switch before history check");
    state
        .dispatch(UiCommand::Undo)
        .expect("undo profile switch");
    assert_eq!(state.preview_scene(), Some("alternate_preview"));
    assert_eq!(state.program_scene(), Some("alternate_program"));
    state
        .dispatch(UiCommand::Redo)
        .expect("redo profile switch");
    assert_eq!(state.preview_scene(), Some("source_scene"));
    assert_eq!(state.program_scene(), Some("program"));
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
