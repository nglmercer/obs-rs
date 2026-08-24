use super::groups::project_with_nested_group;
use super::*;

#[test]
fn group_child_removal_is_path_addressed_and_retains_the_source_registry() {
    let mut project = project_with_nested_group();
    project
        .apply(ProjectCommand::RemoveGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "first".to_owned(),
        })
        .expect("remove group child");

    let group = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group after child removal");
    assert_eq!(
        group
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["second", "inner-group"]
    );
    assert!(project
        .profile("live")
        .expect("profile")
        .has_source("background"));

    let before_invalid = project.clone();
    let error = project
        .apply(ProjectCommand::RemoveGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned(), "inner-group".to_owned()],
            item: "missing".to_owned(),
        })
        .expect_err("unknown group child must fail");
    assert_eq!(
        error,
        ProjectError::UnknownSceneItem(Identifier::new("missing").expect("id"))
    );
    assert_eq!(project, before_invalid);
}

#[test]
fn group_child_duplicate_supports_reference_and_source_clone_modes() {
    let mut project = project_with_nested_group();
    project
        .apply(ProjectCommand::DuplicateGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "first".to_owned(),
            mode: SceneItemDuplicateMode::Reference,
        })
        .expect("duplicate group child by reference");
    project
        .apply(ProjectCommand::DuplicateGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "second".to_owned(),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate group child with source clone");

    let group = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group after duplicate");
    assert_eq!(
        group
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec![
            "first",
            "second",
            "inner-group",
            "first_copy",
            "second_copy"
        ]
    );
    assert_eq!(group.items()[3].source_id().as_str(), "background");
    assert_eq!(group.items()[4].source_id().as_str(), "background_copy");
    assert!(project
        .profile("live")
        .expect("profile")
        .has_source("background_copy"));

    let before_invalid = project.clone();
    let error = project
        .apply(ProjectCommand::DuplicateGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["missing-group".to_owned()],
            item: "first".to_owned(),
            mode: SceneItemDuplicateMode::Reference,
        })
        .expect_err("unknown group path must fail");
    assert_eq!(error, ProjectError::InvalidGroupPath);
    assert_eq!(project, before_invalid);
}

#[test]
fn group_child_paste_supports_nested_destinations_and_copy_modes() {
    let mut project = project_with_nested_group();
    project
        .apply(ProjectCommand::PasteGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: SceneItemSpec::new("first", "background").expect("copied item"),
            mode: SceneItemDuplicateMode::Reference,
        })
        .expect("reference paste into group");
    project
        .apply(ProjectCommand::PasteGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned(), "inner-group".to_owned()],
            item: SceneItemSpec::new("nested", "background").expect("copied nested item"),
            mode: SceneItemDuplicateMode::DuplicateSource,
        })
        .expect("duplicate paste into nested group");

    let profile = project.profile("live").expect("profile");
    let group = profile
        .scene("main")
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("outer group");
    assert_eq!(
        group
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["first", "second", "inner-group", "first_copy"]
    );
    let inner = group
        .items()
        .iter()
        .find(|item| item.id().as_str() == "inner-group")
        .and_then(SceneItemSpec::group)
        .expect("inner group");
    assert_eq!(
        inner
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["nested", "nested_copy"]
    );
    assert_eq!(inner.items()[1].source_id().as_str(), "background_copy");
    assert!(profile.source("background_copy").is_some());

    let before_invalid = project.clone();
    let error = project
        .apply(ProjectCommand::PasteGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["missing-group".to_owned()],
            item: SceneItemSpec::new("copy", "background").expect("copied item"),
            mode: SceneItemDuplicateMode::Reference,
        })
        .expect_err("invalid destination must fail");
    assert_eq!(error, ProjectError::InvalidGroupPath);
    assert_eq!(project, before_invalid);
}

#[test]
fn group_child_transform_is_path_addressed_and_rejects_unsupported_parent_composition() {
    let mut project = project_with_nested_group();
    let child_transform =
        FrameTransform::new(1_250, 900, 12, -8, false, false, 220).expect("child transform");
    project
        .apply(ProjectCommand::SetGroupItemTransform {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "first".to_owned(),
            transform: child_transform,
        })
        .expect("set group child transform");

    let group = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group after transform");
    assert_eq!(group.items()[0].transform(), child_transform);

    let group_transform =
        FrameTransform::new(1_100, 1_100, 4, 6, false, false, 255).expect("group transform");
    project
        .apply(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group".to_owned(),
            transform: group_transform,
        })
        .expect("set group transform");
    let before_invalid = project.clone();
    let rotated = child_transform
        .with_rotation_degrees(15)
        .expect("rotated child transform");
    let error = project
        .apply(ProjectCommand::SetGroupItemTransform {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "first".to_owned(),
            transform: rotated,
        })
        .expect_err("rotation cannot cross a transformed group boundary");
    assert_eq!(
        error,
        ProjectError::UnsupportedNestedSceneTransform(Identifier::new("first").expect("id"))
    );
    assert_eq!(project, before_invalid);
}
