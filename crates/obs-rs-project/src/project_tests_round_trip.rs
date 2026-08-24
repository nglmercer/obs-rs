use super::*;

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
fn scene_transition_overrides_round_trip_and_clear_through_project_commands() {
    let mut project = project();
    let transition = TransitionSpec::fade_to_color(450, [0, 255, 0, 128]).expect("transition");
    project
        .apply(ProjectCommand::SetSceneTransitionOverride {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            transition: Some(transition),
        })
        .expect("set scene transition");

    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("scene");
    assert_eq!(scene.transition_override(), Some(transition));
    assert_eq!(
        transition.kind(),
        TransitionKind::FadeToColor {
            color: [0, 255, 0, 128]
        }
    );

    let encoded = project.serialize();
    assert!(encoded.contains(r#""kind": "fade_to_color""#), "{encoded}");
    let decoded = Project::parse(&encoded).expect("transition project parses");
    assert_eq!(decoded, project);

    project
        .apply(ProjectCommand::SetSceneTransitionOverride {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            transition: None,
        })
        .expect("clear scene transition");
    assert_eq!(
        project
            .profile("live")
            .and_then(|profile| profile.scene("main"))
            .and_then(SceneSpec::transition_override),
        None
    );
}

#[test]
fn scene_properties_change_name_and_transition_as_one_history_edit() {
    let mut session = ProjectSession::new(project());
    let transition = TransitionSpec::cross_fade(900).expect("transition");
    session
        .dispatch(ProjectCommand::SetSceneProperties {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            name: "Program scene".to_owned(),
            transition: Some(transition),
        })
        .expect("scene properties");
    let scene = session
        .project()
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("updated scene");
    assert_eq!(scene.name(), "Program scene");
    assert_eq!(scene.transition_override(), Some(transition));

    assert!(session.undo());
    let scene = session
        .project()
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("restored scene");
    assert_eq!(scene.name(), "Main scene");
    assert_eq!(scene.transition_override(), None);

    let mut invalid_project = project();
    let before_invalid = invalid_project.clone();
    let error = invalid_project
        .apply(ProjectCommand::SetSceneProperties {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            name: "   ".to_owned(),
            transition: Some(transition),
        })
        .expect_err("empty scene name must reject the complete edit");
    assert_eq!(error, ProjectError::InvalidName { kind: "scene" });
    assert_eq!(invalid_project, before_invalid);
}

#[test]
fn nested_scene_items_round_trip_and_reject_cycles() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("background").expect("child source item"))
        .expect("child source item attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("child scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("parent", "Parent").expect("parent scene"),
        })
        .expect("parent scene");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "parent".to_owned(),
            item: SceneItemSpec::for_scene("child-item", "child").expect("nested item"),
        })
        .expect("nested scene item");

    let encoded = project.serialize();
    assert!(encoded.contains(r#""scene": "child""#), "{encoded}");
    let decoded = Project::parse(&encoded).expect("nested project parses");
    let nested = decoded
        .profile("live")
        .and_then(|profile| profile.scene("parent"))
        .and_then(|scene| scene.item("child-item"))
        .expect("nested item survives round trip");
    assert!(nested.is_scene_reference());
    assert_eq!(nested.scene_id().map(Identifier::as_str), Some("child"));
    let flattened = decoded
        .profile("live")
        .expect("profile")
        .flatten_scene_items("parent")
        .expect("nested scene flattens");
    assert_eq!(flattened.len(), 1);
    assert_eq!(flattened[0].item_id(), "child-item/background");
    assert_eq!(flattened[0].source_id().as_str(), "background");

    let error = project
        .apply(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "child".to_owned(),
        })
        .expect_err("a referenced scene cannot be removed");
    assert_eq!(
        error,
        ProjectError::SceneInUse(Identifier::new("child").expect("scene id"))
    );
    assert!(project
        .profile("live")
        .and_then(|profile| profile.scene("child"))
        .is_some());

    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "child".to_owned(),
            item: SceneItemSpec::for_scene("parent-item", "parent").expect("cycle item"),
        })
        .expect_err("cycle is rejected atomically");

    project
        .apply(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "parent".to_owned(),
            item: "child-item".to_owned(),
        })
        .expect("remove nested reference");
    project
        .apply(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "child".to_owned(),
        })
        .expect("unreferenced scene can be removed");
}

#[test]
fn group_items_round_trip_flatten_and_duplicate_sources() {
    let mut project = project();
    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group.set_transform(
        FrameTransform::new(2_000, 1_500, 20, 30, true, false, 200).expect("group transform"),
    );
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::for_source("background").expect("group child"))
        .expect("group child attach");
    let mut nested_group =
        SceneItemSpec::for_group("inner-group", "Inner group").expect("nested group");
    nested_group.set_transform(
        FrameTransform::new(1_000, 1_000, 0, 0, true, false, 255).expect("nested group transform"),
    );
    nested_group
        .group_mut()
        .expect("nested group target")
        .add_item(SceneItemSpec::for_source("background").expect("nested group child"))
        .expect("nested group child attach");
    group
        .group_mut()
        .expect("group target")
        .add_item(nested_group)
        .expect("nested group attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: group,
        })
        .expect("add group item");

    let encoded = project.serialize();
    assert!(encoded.contains(r#""group""#), "{encoded}");
    let decoded = Project::parse(&encoded).expect("group project parses");
    let group = decoded
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group survives round trip");
    assert_eq!(group.name(), "Overlay group");
    assert_eq!(group.items().len(), 2);
    assert_eq!(
        group.items()[1].group().map(GroupSpec::name),
        Some("Inner group")
    );
    let flattened = decoded
        .profile("live")
        .expect("profile")
        .flatten_scene_items("main")
        .expect("group flattens");
    assert_eq!(flattened.len(), 3);
    assert_eq!(
        flattened
            .iter()
            .map(FlattenedSceneItem::item_id)
            .collect::<Vec<_>>(),
        vec![
            "background",
            "overlay-group/background",
            "overlay-group/inner-group/background"
        ]
    );
    assert!(flattened
        .iter()
        .all(|item| item.source_id().as_str() == "background"));
    assert!(flattened[1].transform().flip_x());
    assert!(!flattened[2].transform().flip_x());

    project
        .apply(ProjectCommand::DuplicateSceneWithMode {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate scene with group sources");
    let profile = project.profile("live").expect("profile");
    assert_eq!(profile.sources().count(), 2);
    let duplicate_group = profile
        .scene("main_copy")
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group copied");
    assert_eq!(duplicate_group.items().len(), 2);
    assert_ne!(
        duplicate_group.items()[0].source_id().as_str(),
        "background"
    );
    assert_ne!(
        duplicate_group.items()[1]
            .group()
            .expect("copied nested group")
            .items()[0]
            .source_id()
            .as_str(),
        "background"
    );
}

#[test]
fn group_nesting_is_bounded_before_runtime_flattening() {
    let mut project = project();
    let mut nested = SceneItemSpec::for_source("background").expect("source item");
    for depth in 0..=64 {
        let mut group =
            SceneItemSpec::for_group(&format!("group-{depth}"), "Nested group").expect("group");
        group
            .group_mut()
            .expect("group target")
            .add_item(nested)
            .expect("group child");
        nested = group;
    }

    let error = project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: nested,
        })
        .expect_err("an excessively deep group must be rejected");
    assert_eq!(
        error,
        ProjectError::GroupNestingTooDeep(64),
        "the rejected command must leave the project unchanged"
    );
    assert_eq!(
        project
            .profile("live")
            .and_then(|profile| profile.scene("main"))
            .expect("main scene")
            .items()
            .len(),
        1
    );
}
