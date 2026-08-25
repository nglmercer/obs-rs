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
fn nested_scene_item_visibility_and_lock_route_to_the_owner_scene() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("background").expect("child source"))
        .expect("child source attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::for_scene("child-ref", "child").expect("scene reference"),
        })
        .expect("add scene reference");

    project
        .apply(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "child-ref/background".to_owned(),
            visible: false,
        })
        .expect("hide scene-reference leaf");
    project
        .apply(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "child-ref/background".to_owned(),
            locked: true,
        })
        .expect("lock scene-reference leaf");

    let profile = project.profile("live").expect("profile");
    let child_leaf = profile
        .scene("child")
        .and_then(|scene| scene.item("background"))
        .expect("owner scene leaf");
    assert!(!child_leaf.visible());
    assert!(child_leaf.locked());
    assert!(!profile
        .scene("main")
        .and_then(|scene| scene.item("child-ref"))
        .expect("scene reference")
        .locked());
}

#[test]
fn nested_scene_item_remove_routes_to_the_owner_scene() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("background").expect("child source"))
        .expect("child source attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::for_scene("child-ref", "child").expect("scene reference"),
        })
        .expect("add scene reference");

    project
        .apply(ProjectCommand::RemoveSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "child-ref/background".to_owned(),
        })
        .expect("remove scene-reference leaf");

    let profile = project.profile("live").expect("profile");
    assert!(profile
        .scene("child")
        .and_then(|scene| scene.item("background"))
        .is_none());
    assert!(profile
        .scene("main")
        .and_then(|scene| scene.item("child-ref"))
        .is_some());
}

#[test]
fn nested_scene_item_duplicate_routes_to_the_owner_scene() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("background").expect("child source"))
        .expect("child source attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::for_scene("child-ref", "child").expect("scene reference"),
        })
        .expect("add scene reference");

    project
        .apply(ProjectCommand::DuplicateSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "child-ref/background".to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate scene-reference leaf");

    let profile = project.profile("live").expect("profile");
    let child = profile.scene("child").expect("owner scene");
    assert!(child.item("background").is_some());
    assert!(child.item("background_copy").is_some());
    assert!(profile.source("background_copy").is_some());
    assert!(profile
        .scene("main")
        .and_then(|scene| scene.item("child-ref"))
        .is_some());
}

#[test]
fn nested_scene_item_move_routes_to_the_owner_scene() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("background").expect("first child source"))
        .expect("first child source attach");
    child
        .add_item(SceneItemSpec::new("background-second", "background").expect("second child"))
        .expect("second child source attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::for_scene("child-ref", "child").expect("scene reference"),
        })
        .expect("add scene reference");

    project
        .apply(ProjectCommand::MoveSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "child-ref/background".to_owned(),
            target_index: 1,
        })
        .expect("move scene-reference leaf");

    let profile = project.profile("live").expect("profile");
    let child = profile.scene("child").expect("owner scene");
    assert_eq!(child.items()[0].id().as_str(), "background-second");
    assert_eq!(child.items()[1].id().as_str(), "background");
    assert!(profile
        .scene("main")
        .and_then(|scene| scene.item("child-ref"))
        .is_some());
}

#[test]
fn nested_scene_group_name_routes_to_the_owner_scene() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_group("child-group", "Child group").expect("group"))
        .expect("child group attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::for_scene("child-ref", "child").expect("scene reference"),
        })
        .expect("add scene reference");

    project
        .apply(ProjectCommand::SetGroupName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["child-ref".to_owned(), "child-group".to_owned()],
            name: "Renamed child group".to_owned(),
        })
        .expect("rename scene-reference group");

    let profile = project.profile("live").expect("profile");
    assert_eq!(
        profile
            .scene("child")
            .and_then(|scene| scene.item("child-group"))
            .and_then(SceneItemSpec::group)
            .map(GroupSpec::name),
        Some("Renamed child group")
    );
    assert!(profile
        .scene("main")
        .and_then(|scene| scene.item("child-ref"))
        .is_some());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "this regression exercises projection, owner routing, and atomic failures together"
)]
fn canvas_batch_transform_routes_scene_reference_leaves_to_the_owner_scene() {
    let mut project = project();
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    child
        .add_item(SceneItemSpec::for_source("background").expect("child source"))
        .expect("child source attach");
    let mut child_group = SceneItemSpec::for_group("child-group", "Child group").expect("group");
    child_group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::for_source("background").expect("group source"))
        .expect("group source attach");
    child.add_item(child_group).expect("group attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");

    let parent_transform =
        FrameTransform::new(1_500, 1_250, 18, 22, true, false, 220).expect("parent transform");
    let mut reference = SceneItemSpec::for_scene("child-ref", "child").expect("scene reference");
    reference.set_transform(parent_transform);
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: reference,
        })
        .expect("add scene reference");

    let direct = FrameTransform::new(800, 900, 24, -8, false, false, 255).expect("direct leaf");
    let grouped = FrameTransform::new(700, 1_100, -14, 12, true, false, 210).expect("grouped leaf");
    project
        .apply(ProjectCommand::SetSceneItemTransforms {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![
                ("child-ref/background".to_owned(), direct),
                ("child-ref/child-group/background".to_owned(), grouped),
            ],
        })
        .expect("scene-reference leaves update atomically");
    let profile = project.profile("live").expect("profile");
    let child = profile.scene("child").expect("child scene");
    assert_eq!(
        child.item("background").map(SceneItemSpec::transform),
        Some(direct)
    );
    assert_eq!(
        child
            .item("child-group")
            .and_then(SceneItemSpec::group)
            .and_then(|group| {
                group
                    .items()
                    .iter()
                    .find(|item| item.id().as_str() == "background")
            })
            .map(SceneItemSpec::transform),
        Some(grouped)
    );
    assert_eq!(
        profile
            .scene("main")
            .and_then(|scene| scene.item("child-ref"))
            .map(SceneItemSpec::transform),
        Some(parent_transform)
    );

    let before_invalid = project.clone();
    let error = project
        .apply(ProjectCommand::SetSceneItemTransforms {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![
                ("child-ref/background".to_owned(), direct),
                ("child-ref/missing".to_owned(), grouped),
            ],
        })
        .expect_err("an invalid scene-reference leaf rejects the batch");
    assert_eq!(
        error,
        ProjectError::UnknownSceneItem(Identifier::new("missing").expect("item id"))
    );
    assert_eq!(project, before_invalid);

    let rotated = direct
        .with_rotation_milli_degrees(90_000)
        .expect("rotated transform");
    let before_unsupported = project.clone();
    let error = project
        .apply(ProjectCommand::SetSceneItemTransforms {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![("child-ref/background".to_owned(), rotated)],
        })
        .expect_err("transformed scene boundaries reject rotation");
    assert_eq!(
        error,
        ProjectError::UnsupportedNestedSceneTransform(
            Identifier::new("background").expect("item id")
        )
    );
    assert_eq!(project, before_unsupported);
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
