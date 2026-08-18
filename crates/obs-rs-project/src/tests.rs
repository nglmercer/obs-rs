use super::*;
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::Identifier;
use std::path::PathBuf;

fn format() -> VideoFormat {
    VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format")
}

fn settings() -> Config {
    let mut config = Config::new();
    config.set("color", "#102030FF").expect("color");
    config
}

fn project() -> Project {
    let mut project = Project::new("Studio | Demo").expect("project");
    let mut profile = Profile::new("live", "Live profile", format()).expect("profile");
    let mut scene = SceneSpec::new("main", "Main scene").expect("scene");
    let mut source =
        SourceSpec::new("background", "color_source", "Background", settings()).expect("source");
    source.set_transform(
        FrameTransform::new(1_000, 1_000, 4, -3, true, false, 220).expect("transform"),
    );
    source
        .add_filter(
            SourceFilterSpec::new(
                "brightness",
                "Brightness",
                "brightness",
                Config::parse("milli = 750\n").expect("brightness settings"),
            )
            .expect("brightness filter"),
        )
        .expect("brightness filter attach");
    source
        .add_filter(
            SourceFilterSpec::new(
                "opacity",
                "Opacity",
                "opacity",
                Config::parse("value = 200\n").expect("opacity settings"),
            )
            .expect("opacity filter"),
        )
        .expect("opacity filter attach");
    scene.add_source(source).expect("source attach");
    profile.add_scene(scene).expect("scene attach");
    project.add_profile(profile).expect("profile add");
    project
}

fn unique_paths(label: &str) -> (PathBuf, PathBuf) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir();
    (
        root.join(format!("obs-rs-project-{label}-{token}.json")),
        root.join(format!("obs-rs-project-{label}-{token}.part")),
    )
}

#[test]
fn project_round_trips_deterministically_with_escaped_values() {
    let project = project();
    let encoded = project.serialize();
    let decoded = Project::parse(&encoded).expect("parse project");

    assert_eq!(decoded, project);
    assert_eq!(decoded.serialize(), encoded);
    // JSON strings carry punctuation literally, so the title needs no escaping
    // and stays readable in the saved file.
    assert!(encoded.contains(r#""title": "Studio | Demo""#), "{encoded}");
    assert!(
        encoded.contains(r#""format": "obs-rs-project""#),
        "{encoded}"
    );
}

#[test]
fn parser_rejects_a_document_without_the_format_and_version_tags() {
    let encoded = project().serialize();

    let untagged = encoded.replace(
        r#""format": "obs-rs-project""#,
        r#""format": "something-else""#,
    );
    assert!(matches!(
        Project::parse(&untagged),
        Err(ProjectError::InvalidDocument { .. })
    ));

    let future = encoded.replace(r#""version": 1"#, r#""version": 2"#);
    let error = Project::parse(&future).expect_err("a newer schema is not guessed at");
    assert!(
        format!("{error}").contains("unsupported project schema version 2"),
        "{error}"
    );
}

#[test]
fn parser_reports_the_line_a_syntax_error_is_on() {
    let broken = project()
        .serialize()
        .replace(r#""version": 1"#, r#""version": ?"#);

    let error = Project::parse(&broken).expect_err("malformed JSON is rejected");
    match error {
        ProjectError::InvalidDocument { line, .. } => assert!(line > 1, "expected a real line"),
        other => panic!("expected a document error, got {other}"),
    }
}

#[test]
fn optional_backend_and_output_settings_round_trip_without_changing_legacy_defaults() {
    let defaults = project().serialize();
    assert!(
        defaults.contains(r#""render_backend": "cpu""#),
        "{defaults}"
    );
    let decoded = Project::parse(&defaults).expect("default preferences parse");
    let profile = decoded.profile("live").expect("live profile");
    assert_eq!(profile.render_backend(), RenderBackendPreference::Cpu);
    assert_eq!(profile.output_profile(), OutputProfileKind::ReferencePacket);

    let mut configured = project();
    let profile = configured
        .profile_mut(&Identifier::new("live").expect("profile id"))
        .expect("live profile");
    profile.set_render_backend(RenderBackendPreference::Wgpu);
    profile.set_output_profile(OutputProfileKind::SrtMpegTsH264Aac);
    let encoded = configured.serialize();
    assert!(encoded.contains(r#""render_backend": "wgpu""#), "{encoded}");
    assert!(
        encoded.contains(r#""output_kind": "srt-mpegts-h264-aac""#),
        "{encoded}"
    );
    assert_eq!(Project::parse(&encoded), Ok(configured));
}

#[test]
fn command_session_tracks_dirty_state_and_rejects_bad_references() {
    let mut session = ProjectSession::new(project());
    assert!(!session.is_dirty());
    let source =
        SourceSpec::new("foreground", "test_pattern", "Foreground", Config::new()).expect("source");
    session
        .dispatch(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source,
        })
        .expect("add source command");
    let mut replacement_settings = Config::new();
    replacement_settings
        .set("color", "#203040FF")
        .expect("replacement settings");
    session
        .dispatch(ProjectCommand::SetSourceSettings {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            settings: replacement_settings,
        })
        .expect("set source settings command");
    session
        .dispatch(ProjectCommand::SetSourceFilters {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filters: vec![FrameFilter::Grayscale],
        })
        .expect("set source filters command");
    session
        .dispatch(ProjectCommand::SetSourceVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            visible: false,
        })
        .expect("set source visibility command");
    session
        .dispatch(ProjectCommand::SetSourceLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            locked: true,
        })
        .expect("set source locked command");
    session
        .dispatch(ProjectCommand::MoveSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            target_index: 1,
        })
        .expect("move source command");
    session
        .dispatch(ProjectCommand::RemoveSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "foreground".to_owned(),
        })
        .expect("remove source command");
    assert!(session.is_dirty());
    let saved = session.save();
    assert!(!session.is_dirty());
    assert_eq!(
        Project::parse(&saved).expect("saved project"),
        *session.project()
    );
    let source = session
        .project()
        .profiles()
        .next()
        .and_then(|profile| profile.scenes().next())
        .and_then(|scene| {
            scene
                .sources()
                .iter()
                .find(|source| source.id().as_str() == "background")
        })
        .expect("background source");
    assert!(!source.visible());
    assert!(source.locked());

    assert_eq!(
        session.dispatch(ProjectCommand::SetSourceTransform {
            profile: "missing".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            transform: FrameTransform::IDENTITY,
        }),
        Err(ProjectError::UnknownProfile(
            Identifier::new("missing").expect("identifier")
        ))
    );
    assert!(!session.is_dirty());
}

#[test]
fn source_filter_commands_manage_named_ordered_instances() {
    let mut project = project();
    let mut second_brightness = Config::new();
    second_brightness
        .set("milli", "250")
        .expect("brightness settings");

    project
        .apply(ProjectCommand::AddSourceFilter {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: SourceFilterSpec::new(
                "brightness_2",
                "Brightness 2",
                "brightness",
                second_brightness,
            )
            .expect("second brightness filter"),
        })
        .expect("same-kind filter instances are allowed");

    let source = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.source("background"))
        .expect("background source");
    assert_eq!(
        source
            .filters()
            .iter()
            .map(|filter| filter.id().as_str())
            .collect::<Vec<_>>(),
        ["brightness", "opacity", "brightness_2"]
    );

    project
        .apply(ProjectCommand::SetSourceFilterName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            name: "Warmth".to_owned(),
        })
        .expect("rename filter");
    project
        .apply(ProjectCommand::SetSourceFilterEnabled {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            enabled: false,
        })
        .expect("disable filter");
    project
        .apply(ProjectCommand::SetSourceFilterSettings {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            settings: Config::parse("milli = 500\n").expect("updated settings"),
        })
        .expect("update filter settings");
    project
        .apply(ProjectCommand::MoveSourceFilter {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            target_index: 0,
        })
        .expect("reorder filter");

    let source = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.source("background"))
        .expect("background source after edits");
    let edited = source.filters().first().expect("moved filter");
    assert_eq!(edited.id().as_str(), "brightness_2");
    assert_eq!(edited.name(), "Warmth");
    assert!(!edited.enabled());
    assert_eq!(edited.settings().get("milli"), Some("500"));
    assert_eq!(source.filters()[1].kind().as_str(), "brightness");

    project
        .apply(ProjectCommand::RemoveSourceFilter {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: "opacity".to_owned(),
        })
        .expect("remove filter");
    let source = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.source("background"))
        .expect("background source after removal");
    assert_eq!(source.filters().len(), 2);

    let duplicate = SourceFilterSpec::new("brightness_2", "Duplicate", "brightness", Config::new())
        .expect("duplicate fixture");
    assert_eq!(
        project.apply(ProjectCommand::AddSourceFilter {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: duplicate,
        }),
        Err(ProjectError::DuplicateFilter(
            Identifier::new("brightness_2").expect("filter id")
        ))
    );

    let decoded = Project::parse(&project.serialize()).expect("edited filters persist");
    assert_eq!(decoded, project);
}

#[test]
fn source_filter_commands_participate_in_undo_and_redo() {
    let mut session = ProjectSession::new(project());
    session
        .dispatch(ProjectCommand::SetSourceFilterName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: "brightness".to_owned(),
            name: "Renamed brightness".to_owned(),
        })
        .expect("rename filter");
    assert_eq!(
        session
            .project()
            .profile("live")
            .and_then(|profile| profile.scene("main"))
            .and_then(|scene| scene.source("background"))
            .and_then(|source| source.filter(&Identifier::new("brightness").expect("id")))
            .expect("brightness filter")
            .name(),
        "Renamed brightness"
    );
    assert!(session.undo());
    assert_eq!(
        session
            .project()
            .profile("live")
            .and_then(|profile| profile.scene("main"))
            .and_then(|scene| scene.source("background"))
            .and_then(|source| source.filter(&Identifier::new("brightness").expect("id")))
            .expect("brightness filter")
            .name(),
        "Brightness"
    );
    assert!(session.redo());
}

#[test]
fn audio_video_filter_categories_are_persistent_data() {
    let mut project = project();
    project
        .apply(ProjectCommand::AddSourceFilter {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filter: SourceFilterSpec::with_category(
                "compressor",
                "Compressor",
                "compressor",
                SourceFilterCategory::AudioVideo,
                Config::new(),
            )
            .expect("audio/video filter"),
        })
        .expect("add audio/video filter");

    let decoded = Project::parse(&project.serialize()).expect("parse categorized filter");
    let filter = decoded
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.source("background"))
        .and_then(|source| source.filter(&Identifier::new("compressor").expect("filter id")))
        .expect("categorized filter");
    assert_eq!(filter.category(), SourceFilterCategory::AudioVideo);
    assert_eq!(filter.kind().as_str(), "compressor");
}

#[test]
fn duplicate_commands_copy_definitions_and_choose_unique_ids() {
    let mut project = project();
    project
        .apply(ProjectCommand::SetSourceName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            name: "Renamed background".to_owned(),
        })
        .expect("rename source command");
    assert_eq!(
        project.apply(ProjectCommand::SetSourceName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            name: "  ".to_owned(),
        }),
        Err(ProjectError::InvalidName { kind: "source" })
    );

    project
        .apply(ProjectCommand::DuplicateSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
        })
        .expect("duplicate source command");
    project
        .apply(ProjectCommand::DuplicateSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
        })
        .expect("duplicate source command chooses a suffix");

    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("main scene");
    assert_eq!(scene.sources().len(), 3);
    let original = scene.source("background").expect("original source");
    let copy = scene.source("background_copy").expect("first source copy");
    let second_copy = scene
        .source("background_copy_2")
        .expect("second source copy");
    assert_eq!(copy.name(), "Renamed background Copy");
    assert_eq!(second_copy.name(), "Renamed background Copy 2");
    assert_eq!(copy.transform(), original.transform());
    assert_eq!(copy.filters(), original.filters());
    assert_eq!(copy.settings(), original.settings());

    project
        .apply(ProjectCommand::DuplicateScene {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
        })
        .expect("duplicate scene command");
    project
        .apply(ProjectCommand::DuplicateScene {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
        })
        .expect("duplicate scene command chooses a suffix");
    let profile = project.profile("live").expect("live profile");
    assert_eq!(
        profile.scene("main_copy").expect("scene copy").name(),
        "Main scene Copy"
    );
    assert_eq!(
        profile
            .scene("main_copy")
            .expect("scene copy")
            .sources()
            .len(),
        3
    );
    assert_eq!(
        profile
            .scene("main_copy_2")
            .expect("second scene copy")
            .name(),
        "Main scene Copy 2"
    );
}

#[test]
fn remove_scene_command_updates_project_without_partial_mutation() {
    let mut project = project();
    let extra = SceneSpec::new("extra", "Extra").expect("scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: extra,
        })
        .expect("add scene");
    project
        .apply(ProjectCommand::SetSceneName {
            profile: "live".to_owned(),
            scene: "extra".to_owned(),
            name: "Renamed".to_owned(),
        })
        .expect("rename scene");
    assert_eq!(
        project
            .profiles()
            .next()
            .expect("profile")
            .scenes()
            .find(|scene| scene.id().as_str() == "extra")
            .expect("renamed scene")
            .name(),
        "Renamed"
    );
    project
        .apply(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "extra".to_owned(),
        })
        .expect("remove scene");
    assert!(project
        .profiles()
        .next()
        .expect("profile")
        .scenes()
        .all(|scene| scene.id().as_str() != "extra"));
    assert_eq!(
        project.apply(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "missing".to_owned(),
        }),
        Err(ProjectError::UnknownScene(
            Identifier::new("missing").expect("id")
        ))
    );
}

#[test]
fn parser_rejects_duplicate_records_and_preserves_plugin_filter_kinds() {
    // Two profile entries with the same id: the array preserves both, so the
    // duplicate is caught when the second one is attached.
    let mut duplicated = project();
    let encoded = duplicated.serialize();
    let one_profile = encoded
        .find(r"    {")
        .and_then(|start| encoded.rfind("    }").map(|end| &encoded[start..=end + 4]))
        .expect("the document has one profile object");
    let duplicate = encoded.replace(one_profile, &format!("{one_profile},\n{one_profile}"));
    assert!(
        matches!(
            Project::parse(&duplicate),
            Err(ProjectError::DuplicateProfile(_))
        ),
        "{duplicate}"
    );

    duplicated = project();
    let missing_member = duplicated
        .serialize()
        .replace(r#""active_profile""#, r#""inactive_profile""#);
    assert!(matches!(
        Project::parse(&missing_member),
        Err(ProjectError::InvalidDocument { .. })
    ));

    let unknown_filter = project()
        .serialize()
        .replace(r#""kind": "brightness""#, r#""kind": "sepia""#);
    let parsed = Project::parse(&unknown_filter).expect("plugin filter kind is data");
    assert_eq!(parsed.serialize(), unknown_filter);
}

#[test]
fn project_file_store_commits_atomically_and_loads_the_same_state() {
    let (final_path, temp_path) = unique_paths("save");
    let store = ProjectFileStore::new(&final_path, &temp_path).expect("store");
    let mut session = ProjectSession::new(project());
    session
        .dispatch(ProjectCommand::SetActiveProfile {
            id: "live".to_owned(),
        })
        .expect("select profile");
    assert!(session.is_dirty());

    let bytes = store.save(&mut session).expect("save project");
    assert!(bytes > 0);
    assert!(!session.is_dirty());
    assert!(!temp_path.exists());
    assert_eq!(store.load().expect("load project"), *session.project());
    std::fs::remove_file(final_path).expect("remove project fixture");
}

#[test]
fn project_file_store_recovers_a_valid_unswitched_temporary_file() {
    let (final_path, temp_path) = unique_paths("recovery");
    let store = ProjectFileStore::new(&final_path, &temp_path).expect("store");
    let project = project();
    std::fs::write(&temp_path, project.serialize()).expect("write recovery fixture");

    assert_eq!(store.recover().expect("recover project"), Some(project));
    assert!(!final_path.exists());
    std::fs::remove_file(temp_path).expect("remove recovery fixture");
}

#[test]
fn set_profile_video_format_command_applies_and_round_trips() {
    let mut project = project();
    let updated =
        VideoFormat::new(1920, 1080, FrameRate::new(60, 1).expect("rate")).expect("format");

    project
        .apply(ProjectCommand::SetProfileVideoFormat {
            profile: "live".to_owned(),
            format: updated,
        })
        .expect("video format applies");

    let profile = project.profiles().next().expect("profile");
    assert_eq!(profile.video_format(), updated);

    let decoded = Project::parse(&project.serialize()).expect("parse project");
    assert_eq!(
        decoded.profiles().next().expect("profile").video_format(),
        updated
    );
}

#[test]
fn set_profile_video_format_command_rejects_an_unknown_profile() {
    let mut project = project();

    let error = project
        .apply(ProjectCommand::SetProfileVideoFormat {
            profile: "missing".to_owned(),
            format: format(),
        })
        .expect_err("unknown profile is rejected");

    assert!(matches!(error, ProjectError::UnknownProfile(_)));
}

fn rename_scene(name: &str) -> ProjectCommand {
    ProjectCommand::SetSceneName {
        profile: "live".to_owned(),
        scene: "main".to_owned(),
        name: name.to_owned(),
    }
}

fn scene_name(session: &ProjectSession) -> String {
    session
        .project()
        .profile("live")
        .expect("profile")
        .scene("main")
        .expect("scene")
        .name()
        .to_owned()
}

#[test]
fn undo_restores_the_state_before_the_last_accepted_command() {
    let mut session = ProjectSession::new(project());
    assert!(!session.can_undo(), "a fresh session has nothing to undo");

    session.dispatch(rename_scene("Renamed")).expect("rename");
    assert_eq!(scene_name(&session), "Renamed");
    assert!(session.can_undo());

    let before_undo = session.revision();
    assert!(session.undo());
    assert_eq!(scene_name(&session), "Main scene");
    assert!(session.can_redo());
    assert_ne!(
        session.revision(),
        before_undo,
        "an undo is a change observers must see"
    );

    assert!(session.redo());
    assert_eq!(scene_name(&session), "Renamed");
    assert!(!session.can_redo());
}

#[test]
fn undo_and_redo_are_no_ops_at_the_ends_of_the_history() {
    let mut session = ProjectSession::new(project());

    assert!(!session.undo(), "nothing precedes a fresh session");
    assert!(!session.redo(), "nothing has been undone yet");

    session.dispatch(rename_scene("Renamed")).expect("rename");
    assert!(session.undo());
    assert!(
        !session.undo(),
        "the history bottom is reached exactly once"
    );
    assert_eq!(scene_name(&session), "Main scene");
}

#[test]
fn a_rejected_command_records_no_undo_step() {
    let mut session = ProjectSession::new(project());

    session
        .dispatch(ProjectCommand::SetSceneName {
            profile: "live".to_owned(),
            scene: "missing".to_owned(),
            name: "Renamed".to_owned(),
        })
        .expect_err("an unknown scene is rejected");

    assert!(
        !session.can_undo(),
        "a rejected command must not become an undoable step"
    );
    assert!(!session.is_dirty());
}

#[test]
fn a_new_edit_discards_the_redo_branch() {
    let mut session = ProjectSession::new(project());
    session.dispatch(rename_scene("First")).expect("first");
    assert!(session.undo());
    assert!(session.can_redo());

    session.dispatch(rename_scene("Second")).expect("second");

    assert!(
        !session.can_redo(),
        "redoing onto a diverged state would reapply a replaced edit"
    );
    assert_eq!(scene_name(&session), "Second");
}

#[test]
fn history_is_bounded_and_drops_the_oldest_states_first() {
    let mut session = ProjectSession::new(project());
    // One more edit than the bound, so the very first state must have aged out.
    for index in 0..=MAX_HISTORY_DEPTH {
        session
            .dispatch(rename_scene(&format!("Take {index}")))
            .expect("rename");
    }

    let mut undone = 0;
    while session.undo() {
        undone += 1;
    }

    assert_eq!(undone, MAX_HISTORY_DEPTH);
    assert_eq!(
        scene_name(&session),
        "Take 0",
        "the oldest retained state is the one after the dropped original"
    );
}

#[test]
fn loading_a_project_clears_the_history() {
    let mut session = ProjectSession::new(project());
    session.dispatch(rename_scene("Renamed")).expect("rename");
    assert!(session.can_undo());

    session.replace(project());

    assert!(
        !session.can_undo() && !session.can_redo(),
        "undoing across a load would resurrect an unrelated project"
    );
}

#[test]
fn project_codec_round_trips_crop_and_accepts_legacy_transforms() {
    let mut cropped = project();
    let transform = FrameTransform::new(1_250, 900, 12, -8, true, false, 180)
        .expect("transform")
        .with_crop(4, 5, 6, 7)
        .expect("crop");
    let profile_id = Identifier::new("live").expect("profile id");
    let scene_id = Identifier::new("main").expect("scene id");
    let source_id = Identifier::new("background").expect("source id");
    cropped
        .profile_mut(&profile_id)
        .expect("profile")
        .scene_mut(&scene_id)
        .expect("scene")
        .source_mut(&source_id)
        .expect("source")
        .set_transform(transform);

    let decoded = Project::parse(&cropped.serialize()).expect("parse cropped project");
    let decoded_transform = decoded
        .profile("live")
        .expect("profile")
        .scene("main")
        .expect("scene")
        .source("background")
        .expect("source")
        .transform();
    assert_eq!(decoded_transform, transform);

    let legacy = project().serialize().replace(",0,0,0,0|", "|");
    Project::parse(&legacy).expect("seven-field legacy transforms remain readable");
}
