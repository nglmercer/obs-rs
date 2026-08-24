use super::*;

pub(super) fn project_with_nested_group() -> Project {
    let mut project = project();
    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::new("first", "background").expect("first child"))
        .expect("first child attach");
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::new("second", "background").expect("second child"))
        .expect("second child attach");
    let mut inner = SceneItemSpec::for_group("inner-group", "Inner group").expect("inner group");
    inner
        .group_mut()
        .expect("inner group target")
        .add_item(SceneItemSpec::new("nested", "background").expect("nested child"))
        .expect("nested child attach");
    group
        .group_mut()
        .expect("group target")
        .add_item(inner)
        .expect("inner group attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: group,
        })
        .expect("add group");
    project
}

#[test]
fn group_child_commands_are_path_addressed_and_atomic() {
    let mut project = project_with_nested_group();
    let group_path = vec!["overlay-group".to_owned()];
    project
        .apply(ProjectCommand::SetGroupItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: group_path.clone(),
            item: "first".to_owned(),
            visible: false,
        })
        .expect("hide group child");
    project
        .apply(ProjectCommand::SetGroupItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: group_path.clone(),
            item: "first".to_owned(),
            locked: true,
        })
        .expect("lock group child");
    project
        .apply(ProjectCommand::MoveGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: group_path.clone(),
            item: "first".to_owned(),
            target_index: 1,
        })
        .expect("reorder group child");
    project
        .apply(ProjectCommand::SetGroupItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned(), "inner-group".to_owned()],
            item: "nested".to_owned(),
            visible: false,
        })
        .expect("edit nested group child");

    let group = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group after child edits");
    assert_eq!(
        group
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["second", "first", "inner-group"]
    );
    assert!(!group.items()[1].visible());
    assert!(group.items()[1].locked());
    assert!(!group.items()[2].group().expect("inner group").items()[0].visible());

    let before_invalid_move = project.clone();
    let error = project
        .apply(ProjectCommand::MoveGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path,
            item: "first".to_owned(),
            target_index: 99,
        })
        .expect_err("out-of-range child move must fail");
    assert_eq!(error, ProjectError::InvalidSceneItemOrder { index: 99 });
    assert_eq!(project, before_invalid_move);

    let before_invalid_path = project.clone();
    let error = project
        .apply(ProjectCommand::SetGroupItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["missing-group".to_owned()],
            item: "first".to_owned(),
            visible: true,
        })
        .expect_err("unknown group path must fail");
    assert_eq!(error, ProjectError::InvalidGroupPath);
    assert_eq!(project, before_invalid_path);
}

#[test]
fn group_names_are_path_addressed_and_atomic() {
    let mut project = project_with_nested_group();
    project
        .apply(ProjectCommand::SetGroupName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            name: "Overlays".to_owned(),
        })
        .expect("rename root group");
    project
        .apply(ProjectCommand::SetGroupName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned(), "inner-group".to_owned()],
            name: "Inner overlays".to_owned(),
        })
        .expect("rename nested group");

    let outer = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .expect("outer group item");
    assert_eq!(outer.group().expect("outer group").name(), "Overlays");
    assert_eq!(
        outer
            .group()
            .expect("outer group")
            .items()
            .iter()
            .find(|item| item.id().as_str() == "inner-group")
            .and_then(SceneItemSpec::group)
            .expect("inner group")
            .name(),
        "Inner overlays"
    );

    let before_invalid = project.clone();
    let error = project
        .apply(ProjectCommand::SetGroupName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned(), "first".to_owned()],
            name: "Not a group".to_owned(),
        })
        .expect_err("a source child cannot be renamed as a group");
    assert_eq!(error, ProjectError::InvalidGroupPath);
    assert_eq!(project, before_invalid);

    let error = project
        .apply(ProjectCommand::SetGroupName {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            name: "   ".to_owned(),
        })
        .expect_err("empty group names must be rejected");
    assert_eq!(error, ProjectError::InvalidName { kind: "group" });
    assert_eq!(project, before_invalid);
}

#[test]
fn grouping_root_scene_items_preserves_order_and_item_state() {
    let mut project = project();
    let mut middle = SceneItemSpec::new("middle", "background").expect("middle item");
    middle.set_visible(false);
    middle.set_transform(
        FrameTransform::new(1_200, 900, -12, 8, false, true, 180).expect("middle transform"),
    );
    let mut foreground = SceneItemSpec::new("foreground", "background").expect("foreground item");
    foreground.set_transform(
        FrameTransform::new(800, 700, 6, 14, true, false, 90).expect("foreground transform"),
    );
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: middle,
        })
        .expect("add middle item");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: foreground,
        })
        .expect("add foreground item");

    let group = SceneItemSpec::for_group("selection-group", "Selection group").expect("group");
    project
        .apply(ProjectCommand::GroupSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["foreground".to_owned(), "background".to_owned()],
            group,
        })
        .expect("group selected root items");

    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("main scene");
    assert_eq!(
        scene
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["selection-group", "middle"]
    );
    let grouped_items = scene
        .item("selection-group")
        .and_then(SceneItemSpec::group)
        .expect("grouped items");
    assert_eq!(
        grouped_items
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["background", "foreground"]
    );
    let background_child = grouped_items
        .items()
        .iter()
        .find(|item| item.id().as_str() == "background")
        .expect("background child");
    let foreground_child = grouped_items
        .items()
        .iter()
        .find(|item| item.id().as_str() == "foreground")
        .expect("foreground child");
    assert_eq!(
        background_child.transform(),
        FrameTransform::new(1_000, 1_000, 4, -3, true, false, 220).expect("background transform")
    );
    assert_eq!(
        foreground_child.transform(),
        FrameTransform::new(800, 700, 6, 14, true, false, 90).expect("foreground transform")
    );
    assert!(!scene.item("middle").expect("middle item").visible());
    assert_eq!(
        scene.item("middle").expect("middle item").transform(),
        FrameTransform::new(1_200, 900, -12, 8, false, true, 180).expect("middle transform")
    );
    assert!(project
        .profile("live")
        .expect("profile")
        .has_source("background"));
}

#[test]
fn grouping_nested_siblings_preserves_parent_order_and_item_state() {
    let mut project = project_with_nested_group();
    project
        .apply(ProjectCommand::SetGroupItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "first".to_owned(),
            visible: false,
        })
        .expect("hide nested child");
    let child_transform =
        FrameTransform::new(850, 1_150, -7, 12, true, false, 175).expect("child transform");
    project
        .apply(ProjectCommand::SetGroupItemTransform {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "second".to_owned(),
            transform: child_transform,
        })
        .expect("transform nested child");

    project
        .apply(ProjectCommand::GroupSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![
                "overlay-group/second".to_owned(),
                "overlay-group/first".to_owned(),
            ],
            group: SceneItemSpec::for_group("nested-selection", "Nested selection")
                .expect("nested group"),
        })
        .expect("group nested siblings");

    let outer = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("outer group");
    assert_eq!(
        outer
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["nested-selection", "inner-group"]
    );
    let nested = outer
        .items()
        .first()
        .and_then(SceneItemSpec::group)
        .expect("nested selection group");
    assert_eq!(
        nested
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert!(!nested.items()[0].visible());
    assert_eq!(nested.items()[1].transform(), child_transform);
}

#[test]
fn moving_scene_items_between_root_and_groups_preserves_state_and_order() {
    let mut project = project_with_nested_group();
    let original_transform = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("background"))
        .map(SceneItemSpec::transform)
        .expect("root background transform");

    project
        .apply(ProjectCommand::MoveSceneItemToParent {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            destination: vec!["overlay-group".to_owned()],
            target_index: 1,
        })
        .expect("move root item into group");
    let group = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(SceneItemSpec::group)
        .expect("group after root reparent");
    assert_eq!(
        group
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["first", "background", "second", "inner-group"]
    );
    assert_eq!(
        group
            .items()
            .iter()
            .find(|item| item.id().as_str() == "background")
            .expect("moved item")
            .transform(),
        original_transform
    );

    project
        .apply(ProjectCommand::MoveSceneItemToParent {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group/first".to_owned(),
            destination: Vec::new(),
            target_index: 0,
        })
        .expect("move group child to scene root");
    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("scene after reparent");
    assert_eq!(
        scene
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["first", "overlay-group"]
    );
    let group = scene
        .item("overlay-group")
        .and_then(SceneItemSpec::group)
        .expect("group after child reparent");
    assert_eq!(group.items()[0].id().as_str(), "background");

    let before_cycle = project.clone();
    let error = project
        .apply(ProjectCommand::MoveSceneItemToParent {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group".to_owned(),
            destination: vec!["overlay-group".to_owned(), "inner-group".to_owned()],
            target_index: 0,
        })
        .expect_err("a group cannot move inside its own descendant");
    assert_eq!(error, ProjectError::InvalidGroupPath);
    assert_eq!(project, before_cycle);

    let before_order = project.clone();
    let error = project
        .apply(ProjectCommand::MoveSceneItemToParent {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group/background".to_owned(),
            destination: Vec::new(),
            target_index: 99,
        })
        .expect_err("out-of-range reparent destination must fail");
    assert_eq!(error, ProjectError::InvalidSceneItemOrder { index: 99 });
    assert_eq!(project, before_order);
}

#[test]
fn moving_scene_items_rejects_locked_source_or_destination_atomically() {
    let mut project = project_with_nested_group();
    project
        .apply(ProjectCommand::SetGroupItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "first".to_owned(),
            locked: true,
        })
        .expect("lock source child");
    let before_source = project.clone();
    let error = project
        .apply(ProjectCommand::MoveSceneItemToParent {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group/first".to_owned(),
            destination: Vec::new(),
            target_index: 0,
        })
        .expect_err("locked source must not move");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("first").expect("id"))
    );
    assert_eq!(project, before_source);

    project
        .apply(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group".to_owned(),
            locked: true,
        })
        .expect("lock destination group");
    let before_destination = project.clone();
    let error = project
        .apply(ProjectCommand::MoveSceneItemToParent {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            destination: vec!["overlay-group".to_owned()],
            target_index: 0,
        })
        .expect_err("locked destination group must reject reparenting");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("overlay-group").expect("id"))
    );
    assert_eq!(project, before_destination);
}

#[test]
fn grouping_rejects_different_parents_or_invalid_selections_atomically() {
    let mut project = project_with_nested_group();
    let invalid_nested = || SceneItemSpec::for_group("new-group", "New group").expect("group");

    for (items, expected) in [
        (
            vec!["background".to_owned(), "overlay-group/first".to_owned()],
            ProjectError::InvalidGroupSelection,
        ),
        (
            vec!["background".to_owned(), "background".to_owned()],
            ProjectError::InvalidGroupSelection,
        ),
        (
            vec!["background".to_owned()],
            ProjectError::InvalidGroupSelection,
        ),
    ] {
        let before = project.clone();
        let error = project
            .apply(ProjectCommand::GroupSceneItems {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                items,
                group: invalid_nested(),
            })
            .expect_err("invalid grouping selection must fail");
        assert_eq!(error, expected);
        assert_eq!(project, before);
    }

    let mut non_empty = invalid_nested();
    non_empty
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::new("child", "background").expect("child"))
        .expect("add child");
    let before = project.clone();
    let error = project
        .apply(ProjectCommand::GroupSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["background".to_owned(), "overlay-group".to_owned()],
            group: non_empty,
        })
        .expect_err("pre-populated group must fail");
    assert_eq!(error, ProjectError::InvalidGroupPath);
    assert_eq!(project, before);

    let mut locked_project = super::project();
    locked_project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::new("middle", "background").expect("middle item"),
        })
        .expect("add item to lock");
    locked_project
        .apply(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "middle".to_owned(),
            locked: true,
        })
        .expect("lock item");
    let before_locked = locked_project.clone();
    let error = locked_project
        .apply(ProjectCommand::GroupSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["background".to_owned(), "middle".to_owned()],
            group: invalid_nested(),
        })
        .expect_err("locked item must not be grouped");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("middle").expect("locked item id"))
    );
    assert_eq!(locked_project, before_locked);
}

#[test]
fn ungrouping_root_group_preserves_child_order_and_state() {
    let mut project = project_with_nested_group();
    let group_path = vec!["overlay-group".to_owned()];
    project
        .apply(ProjectCommand::SetGroupItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: group_path.clone(),
            item: "first".to_owned(),
            visible: false,
        })
        .expect("hide first child");
    project
        .apply(ProjectCommand::SetGroupItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: group_path.clone(),
            item: "second".to_owned(),
            locked: true,
        })
        .expect("lock second child");
    let child_transform =
        FrameTransform::new(1_250, 875, -11, 17, true, false, 190).expect("child transform");
    project
        .apply(ProjectCommand::SetGroupItemTransform {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path,
            item: "first".to_owned(),
            transform: child_transform,
        })
        .expect("transform first child");

    project
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "overlay-group".to_owned(),
        })
        .expect("ungroup root group");

    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("main scene");
    assert_eq!(
        scene
            .items()
            .iter()
            .map(SceneItemSpec::id)
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["background", "first", "second", "inner-group"]
    );
    assert!(scene.item("overlay-group").is_none());
    assert!(!scene.item("first").expect("first child").visible());
    assert_eq!(
        scene.item("first").expect("first child").transform(),
        child_transform
    );
    assert!(scene.item("second").expect("second child").locked());
    assert_eq!(
        scene
            .item("inner-group")
            .and_then(SceneItemSpec::group)
            .map(|group| group.items().len()),
        Some(1)
    );
    assert!(project
        .profile("live")
        .expect("profile")
        .has_source("background"));
}

#[test]
fn ungrouping_nested_group_preserves_parent_order_and_child_state() {
    let mut project = project_with_nested_group();
    let group_path = vec!["overlay-group".to_owned(), "inner-group".to_owned()];
    project
        .apply(ProjectCommand::SetGroupItemVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: group_path.clone(),
            item: "nested".to_owned(),
            visible: false,
        })
        .expect("hide nested group child");
    let child_transform =
        FrameTransform::new(1_100, 925, 13, -6, false, true, 165).expect("child transform");
    project
        .apply(ProjectCommand::SetGroupItemTransform {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path,
            item: "nested".to_owned(),
            transform: child_transform,
        })
        .expect("transform nested group child");

    project
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "overlay-group/inner-group".to_owned(),
        })
        .expect("ungroup nested group");

    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("main scene");
    assert_eq!(
        scene
            .item("overlay-group")
            .and_then(SceneItemSpec::group)
            .map(|group| {
                group
                    .items()
                    .iter()
                    .map(SceneItemSpec::id)
                    .map(Identifier::as_str)
                    .collect::<Vec<_>>()
            }),
        Some(vec!["first", "second", "nested"])
    );
    let child = scene
        .item("overlay-group")
        .and_then(SceneItemSpec::group)
        .and_then(|group| group.items().last())
        .expect("exposed nested child");
    assert_eq!(child.id().as_str(), "nested");
    assert!(!child.visible());
    assert_eq!(child.transform(), child_transform);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one failure matrix covers root and nested ungroup validation"
)]
fn ungrouping_rejects_invalid_or_colliding_groups_atomically() {
    let mut project = project_with_nested_group();
    for (group, expected) in [
        ("background".to_owned(), ProjectError::InvalidGroupPath),
        (
            "overlay-group/first".to_owned(),
            ProjectError::InvalidGroupPath,
        ),
    ] {
        let before = project.clone();
        let error = project
            .apply(ProjectCommand::UngroupSceneItem {
                profile: "live".to_owned(),
                scene: "main".to_owned(),
                group,
            })
            .expect_err("invalid group target must fail");
        assert_eq!(error, expected);
        assert_eq!(project, before);
    }

    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::new("first", "background").expect("colliding item"),
        })
        .expect("add colliding root item");
    let before_collision = project.clone();
    let error = project
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "overlay-group".to_owned(),
        })
        .expect_err("child collision must fail");
    assert_eq!(
        error,
        ProjectError::DuplicateSceneItem(Identifier::new("first").expect("child id"))
    );
    assert_eq!(project, before_collision);

    let mut nested_collision = project_with_nested_group();
    nested_collision
        .apply(ProjectCommand::PasteGroupItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: SceneItemSpec::new("nested", "background").expect("nested collision"),
            mode: SceneItemDuplicateMode::Reference,
        })
        .expect("add nested collision sibling");
    let before_nested_collision = nested_collision.clone();
    let error = nested_collision
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "overlay-group/inner-group".to_owned(),
        })
        .expect_err("nested child collision must fail");
    assert_eq!(
        error,
        ProjectError::DuplicateSceneItem(Identifier::new("nested").expect("nested id"))
    );
    assert_eq!(nested_collision, before_nested_collision);

    let mut locked_nested = project_with_nested_group();
    locked_nested
        .apply(ProjectCommand::SetGroupItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group_path: vec!["overlay-group".to_owned()],
            item: "inner-group".to_owned(),
            locked: true,
        })
        .expect("lock nested group");
    let before_locked_nested = locked_nested.clone();
    let error = locked_nested
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "overlay-group/inner-group".to_owned(),
        })
        .expect_err("locked nested group must fail");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("inner-group").expect("nested group id"))
    );
    assert_eq!(locked_nested, before_locked_nested);

    let mut locked = project_with_nested_group();
    locked
        .apply(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group".to_owned(),
            locked: true,
        })
        .expect("lock root group");
    let before_locked = locked.clone();
    let error = locked
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "overlay-group".to_owned(),
        })
        .expect_err("locked group must fail");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("overlay-group").expect("group id"))
    );
    assert_eq!(locked, before_locked);

    let mut empty = project_with_nested_group();
    empty
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::for_group("empty-group", "Empty group").expect("empty group"),
        })
        .expect("add empty group");
    empty
        .apply(ProjectCommand::UngroupSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            group: "empty-group".to_owned(),
        })
        .expect("ungroup empty group");
    assert!(empty
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .is_some_and(|scene| !scene.has_item("empty-group")));
}
