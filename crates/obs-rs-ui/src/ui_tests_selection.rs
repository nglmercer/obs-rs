use super::*;

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
fn desktop_state_selects_nested_group_targets_without_duplicate_state() {
    let mut state = DesktopState::new(project());
    state
        .dispatch(UiCommand::SelectPreviewScene {
            id: "source_scene".to_owned(),
        })
        .expect("source scene selection");

    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::for_source("source").expect("nested item"))
        .expect("nested item attach");
    state
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "source_scene".to_owned(),
            item: group,
        }))
        .expect("group insertion");

    state
        .dispatch(UiCommand::SelectSource {
            id: "overlay-group/source".to_owned(),
        })
        .expect("nested selection");
    assert_eq!(state.selected_source(), Some("overlay-group/source"));
    assert_eq!(
        state.selected_sources().collect::<Vec<_>>(),
        vec!["overlay-group/source"]
    );

    state
        .dispatch(UiCommand::ToggleSourceSelection {
            id: "overlay-group/source".to_owned(),
        })
        .expect("nested toggle");
    assert_eq!(state.selected_source(), None);

    let error = state
        .dispatch(UiCommand::SelectSource {
            id: "overlay-group/missing".to_owned(),
        })
        .expect_err("missing nested target must be rejected");
    assert!(error.to_string().contains("overlay-group/missing"));
}

#[test]
fn desktop_state_selects_scene_reference_targets() {
    let mut state = DesktopState::new(project());
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("source").expect("child source"))
        .expect("child source attach");
    state
        .dispatch(UiCommand::Project(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        }))
        .expect("add child scene");
    state
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: SceneItemSpec::for_scene("child-ref", "child").expect("scene reference"),
        }))
        .expect("add scene reference");

    state
        .dispatch(UiCommand::SelectSource {
            id: "child-ref/source".to_owned(),
        })
        .expect("scene-reference selection");
    assert_eq!(state.selected_source(), Some("child-ref/source"));
    state
        .dispatch(UiCommand::ToggleSourceSelection {
            id: "child-ref/source".to_owned(),
        })
        .expect("scene-reference selection toggle");
    assert_eq!(state.selected_source(), None);
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
