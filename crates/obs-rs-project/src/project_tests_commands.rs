use super::*;

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

    let future = encoded.replace(r#""version": 8"#, r#""version": 9"#);
    let error = Project::parse(&future).expect_err("a newer schema is not guessed at");
    assert!(
        format!("{error}").contains("unsupported project schema version 9"),
        "{error}"
    );
}

#[test]
fn parser_reports_the_line_a_syntax_error_is_on() {
    let broken = project()
        .serialize()
        .replace(r#""version": 8"#, r#""version": ?"#);

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
#[allow(
    clippy::too_many_lines,
    reason = "the session test exercises one atomic history workflow across all filter variants"
)]
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
            source: "background".to_owned(),
            settings: replacement_settings,
        })
        .expect("set source settings command");
    session
        .dispatch(ProjectCommand::SetSourceFilters {
            profile: "live".to_owned(),
            source: "background".to_owned(),
            filters: vec![
                FrameFilter::Grayscale,
                FrameFilter::ColorCorrection(
                    ColorCorrection::new(250, -500, 125, 750, 30, 900).expect("color correction"),
                ),
                FrameFilter::LumaKey(LumaKey::new(900, 100, 40, 60).expect("luma key")),
                FrameFilter::ColorKey(ColorKey::new(0, 255, 0, 120, 80).expect("color key")),
                FrameFilter::ChromaKey(
                    ChromaKey::new(0, 255, 0, 400, 80, 100).expect("chroma key"),
                ),
                FrameFilter::Sharpen { milli: 80 },
                FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new([220, 240, 255], [4, 8, 12])),
                FrameFilter::Scroll {
                    speed_x: 120,
                    speed_y: -80,
                    looped: false,
                },
                FrameFilter::RenderDelay(obs_rs_media::RenderDelay { milliseconds: 100 }),
            ],
        })
        .expect("set source filters command");
    session
        .dispatch(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            visible: false,
        })
        .expect("set source visibility command");
    session
        .dispatch(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            locked: true,
        })
        .expect("set source locked command");
    session
        .dispatch(ProjectCommand::MoveSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            target_index: 1,
        })
        .expect("move source command");
    session
        .dispatch(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "foreground".to_owned(),
        })
        .expect("remove source command");
    assert!(session.is_dirty());
    let saved = session.save();
    assert!(!session.is_dirty());
    assert_eq!(
        Project::parse(&saved).expect("saved project"),
        *session.project()
    );
    let _source = session
        .project()
        .profile("live")
        .and_then(|profile| profile.source("background"))
        .expect("background source");
    let item = session
        .project()
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("background"))
        .expect("background item");
    assert!(!item.visible());
    assert!(item.locked());

    assert_eq!(
        session.dispatch(ProjectCommand::SetSceneItemTransform {
            profile: "missing".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
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
        .and_then(|profile| profile.source("background"))
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
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            name: "Warmth".to_owned(),
        })
        .expect("rename filter");
    project
        .apply(ProjectCommand::SetSourceFilterEnabled {
            profile: "live".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            enabled: false,
        })
        .expect("disable filter");
    project
        .apply(ProjectCommand::SetSourceFilterSettings {
            profile: "live".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            settings: Config::parse("milli = 500\n").expect("updated settings"),
        })
        .expect("update filter settings");
    project
        .apply(ProjectCommand::MoveSourceFilter {
            profile: "live".to_owned(),
            source: "background".to_owned(),
            filter: "brightness_2".to_owned(),
            target_index: 0,
        })
        .expect("reorder filter");

    let source = project
        .profile("live")
        .and_then(|profile| profile.source("background"))
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
            source: "background".to_owned(),
            filter: "opacity".to_owned(),
        })
        .expect("remove filter");
    let source = project
        .profile("live")
        .and_then(|profile| profile.source("background"))
        .expect("background source after removal");
    assert_eq!(source.filters().len(), 2);

    let duplicate = SourceFilterSpec::new("brightness_2", "Duplicate", "brightness", Config::new())
        .expect("duplicate fixture");
    assert_eq!(
        project.apply(ProjectCommand::AddSourceFilter {
            profile: "live".to_owned(),
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
fn batched_scene_item_transforms_are_atomic_and_one_undo_step() {
    let mut project = project();
    project
        .apply(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: SourceSpec::new("foreground", "color_source", "Foreground", settings())
                .expect("foreground source"),
        })
        .expect("second item");
    let mut session = ProjectSession::new(project);
    let first =
        FrameTransform::new(1_000, 1_000, 40, 50, false, false, 255).expect("first transform");
    let second =
        FrameTransform::new(1_200, 800, -20, 30, true, false, 220).expect("second transform");
    session
        .dispatch(ProjectCommand::SetSceneItemTransforms {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![
                ("background".to_owned(), first),
                ("foreground".to_owned(), second),
            ],
        })
        .expect("batch transform");
    let scene = session
        .project()
        .profile("live")
        .expect("profile")
        .scene("main")
        .expect("scene");
    assert_eq!(
        scene.item("background").expect("background").transform(),
        first
    );
    assert_eq!(
        scene.item("foreground").expect("foreground").transform(),
        second
    );
    assert!(session.can_undo());
    session.undo();
    assert_eq!(
        session
            .project()
            .profile("live")
            .expect("profile")
            .scene("main")
            .expect("scene")
            .item("background")
            .expect("background")
            .transform()
            .translate_x(),
        4
    );

    let mut invalid = session.project().clone();
    let before = invalid.clone();
    assert!(invalid
        .apply(ProjectCommand::SetSceneItemTransforms {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![
                ("background".to_owned(), first),
                ("missing".to_owned(), second),
            ],
        })
        .is_err());
    assert_eq!(invalid, before);
}

#[test]
fn source_filter_commands_participate_in_undo_and_redo() {
    let mut session = ProjectSession::new(project());
    session
        .dispatch(ProjectCommand::SetSourceFilterName {
            profile: "live".to_owned(),
            source: "background".to_owned(),
            filter: "brightness".to_owned(),
            name: "Renamed brightness".to_owned(),
        })
        .expect("rename filter");
    assert_eq!(
        session
            .project()
            .profile("live")
            .and_then(|profile| profile.source("background"))
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
            .and_then(|profile| profile.source("background"))
            .and_then(|source| source.filter(&Identifier::new("brightness").expect("id")))
            .expect("brightness filter")
            .name(),
        "Brightness"
    );
    assert!(session.redo());
}

#[test]
fn source_registry_shares_configuration_but_not_scene_item_state() {
    let mut project = Project::new("Shared source fixture").expect("project");
    let mut profile = Profile::new("live", "Live", format()).expect("profile");
    profile
        .add_source(
            SourceSpec::new("camera", "camera_capture", "Camera", Config::new()).expect("source"),
        )
        .expect("source registry");

    let mut preview = SceneSpec::new("preview", "Preview").expect("scene");
    preview
        .add_item(SceneItemSpec::for_source("camera").expect("preview item"))
        .expect("preview item");
    let mut program = SceneSpec::new("program", "Program").expect("scene");
    program
        .add_item(SceneItemSpec::new("program_camera", "camera").expect("program item"))
        .expect("program item");
    profile.add_scene(preview).expect("preview scene");
    profile.add_scene(program).expect("program scene");
    project.add_profile(profile).expect("profile");

    let transform = FrameTransform::new(1_250, 900, 24, -8, true, false, 210).expect("transform");
    project
        .apply(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "camera".to_owned(),
            transform,
        })
        .expect("preview transform");
    project
        .apply(ProjectCommand::SetSourceName {
            profile: "live".to_owned(),
            source: "camera".to_owned(),
            name: "Shared camera".to_owned(),
        })
        .expect("shared source rename");

    let profile = project.profile("live").expect("profile");
    assert_eq!(
        profile.source("camera").expect("source").name(),
        "Shared camera"
    );
    assert_eq!(
        profile
            .scene("preview")
            .expect("preview")
            .item("camera")
            .expect("preview item")
            .transform(),
        transform
    );
    assert_eq!(
        profile
            .scene("program")
            .expect("program")
            .item("program_camera")
            .expect("program item")
            .transform(),
        FrameTransform::IDENTITY,
        "a second scene reference keeps independent item state"
    );
    assert_eq!(
        profile
            .scene("program")
            .expect("program")
            .item("program_camera")
            .expect("program item")
            .source_id()
            .as_str(),
        "camera"
    );
}

#[test]
fn scene_item_copy_modes_preserve_references_or_clone_sources() {
    let mut project = project();
    let empty_scene = SceneSpec::new("program", "Program").expect("scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: empty_scene,
        })
        .expect("program scene");

    let item = SceneItemSpec::for_source("background").expect("item");
    project
        .apply(ProjectCommand::PasteSceneItem {
            profile: "live".to_owned(),
            scene: "program".to_owned(),
            item,
            mode: SceneItemDuplicateMode::Reference,
        })
        .expect("reference paste");
    let profile = project.profile("live").expect("profile");
    let reference = profile
        .scene("program")
        .expect("program")
        .item("background")
        .expect("reference item");
    assert_eq!(reference.source_id().as_str(), "background");

    project
        .apply(ProjectCommand::DuplicateSceneWithMode {
            profile: "live".to_owned(),
            scene: "program".to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate scene with cloned source");
    let profile = project.profile("live").expect("profile");
    let duplicate = profile
        .scene("program_copy")
        .expect("duplicated program")
        .item("background")
        .expect("duplicated item");
    assert_ne!(duplicate.source_id().as_str(), "background");
    assert!(profile.source(duplicate.source_id()).is_some());
    assert_eq!(
        profile.sources().count(),
        2,
        "the duplicate scene clones once"
    );
}

#[test]
fn audio_video_filter_categories_are_persistent_data() {
    let mut project = project();
    project
        .apply(ProjectCommand::AddSourceFilter {
            profile: "live".to_owned(),
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
        .and_then(|profile| profile.source("background"))
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
            source: "background".to_owned(),
            name: "Renamed background".to_owned(),
        })
        .expect("rename source command");
    assert_eq!(
        project.apply(ProjectCommand::SetSourceName {
            profile: "live".to_owned(),
            source: "background".to_owned(),
            name: "  ".to_owned(),
        }),
        Err(ProjectError::InvalidName { kind: "source" })
    );

    project
        .apply(ProjectCommand::DuplicateSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate source command");
    project
        .apply(ProjectCommand::DuplicateSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate source command chooses a suffix");

    let profile = project.profile("live").expect("live profile");
    let scene = profile.scene("main").expect("main scene");
    assert_eq!(scene.items().len(), 3);
    let original_item = scene.item("background").expect("original item");
    let copy_item = scene.item("background_copy").expect("first item copy");
    let second_copy_item = scene.item("background_copy_2").expect("second source copy");
    let original = profile
        .source(original_item.source_id())
        .expect("original source");
    let copy = profile
        .source(copy_item.source_id())
        .expect("first source copy");
    let second_copy = profile
        .source(second_copy_item.source_id())
        .expect("second source copy");
    assert_eq!(copy.name(), "Renamed background Copy");
    assert_eq!(second_copy.name(), "Renamed background Copy 2");
    assert_eq!(copy_item.transform(), original_item.transform());
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
fn scene_order_command_is_persisted_and_rejects_invalid_destinations() {
    let mut project = project();
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("zulu", "Zulu").expect("scene"),
        })
        .expect("add zulu scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("alpha", "Alpha").expect("scene"),
        })
        .expect("add alpha scene");

    let scene_ids = |project: &Project| {
        project
            .profile("live")
            .expect("profile")
            .scenes()
            .map(|scene| scene.id().as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(scene_ids(&project), vec!["main", "zulu", "alpha"]);

    project
        .apply(ProjectCommand::MoveScene {
            profile: "live".to_owned(),
            scene: "alpha".to_owned(),
            target_index: 0,
        })
        .expect("move alpha scene");
    assert_eq!(scene_ids(&project), vec!["alpha", "main", "zulu"]);

    let before_invalid = project.clone();
    assert_eq!(
        project.apply(ProjectCommand::MoveScene {
            profile: "live".to_owned(),
            scene: "alpha".to_owned(),
            target_index: 99,
        }),
        Err(ProjectError::InvalidSceneOrder { index: 99 })
    );
    assert_eq!(project, before_invalid);

    let encoded = project.serialize();
    assert!(encoded.contains(r#""scene_order": ["#), "{encoded}");
    assert_eq!(Project::parse(&encoded).expect("ordered project"), project);
}

#[test]
fn previous_schema_preserves_serialized_scene_array_order_without_scene_order_member() {
    let mut project = project();
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("zulu", "Zulu").expect("scene"),
        })
        .expect("add zulu scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("alpha", "Alpha").expect("scene"),
        })
        .expect("add alpha scene");
    let encoded = project.serialize();
    let order_start = encoded.find("      \"scene_order\":").expect("scene order");
    let order_end = encoded[order_start..]
        .find("      ],\n")
        .map(|offset| order_start + offset + "      ],\n".len())
        .expect("scene order end");
    let mut previous = encoded;
    previous.replace_range(order_start..order_end, "");
    previous = previous.replace(r#""version": 8"#, r#""version": 5"#);

    let decoded = Project::parse(&previous).expect("version five project");
    let scene_ids = decoded
        .profile("live")
        .expect("profile")
        .scenes()
        .map(|scene| scene.id().as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(scene_ids, vec!["main", "zulu", "alpha"]);
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
