use super::*;

fn add_item(project: &mut Project, id: &str) {
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: SceneItemSpec::new(id, "background").expect("scene item"),
        })
        .expect("scene item insertion");
}

fn scene_item_ids(project: &Project) -> Vec<String> {
    project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("main scene")
        .items()
        .iter()
        .map(|item| item.id().to_string())
        .collect()
}

#[test]
fn multiple_scene_items_are_removed_as_one_undoable_edit() {
    let mut project = project();
    add_item(&mut project, "first");
    add_item(&mut project, "second");
    add_item(&mut project, "third");
    let before = project.clone();

    let mut session = ProjectSession::new(project);
    session
        .dispatch(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["second".to_owned(), "first".to_owned()],
        })
        .expect("selected scene items should be removed");
    assert_eq!(
        scene_item_ids(session.project()),
        vec!["background", "third"]
    );
    assert!(
        session.can_undo(),
        "the batch should create one history entry"
    );

    assert!(session.undo(), "the batch should undo in one step");
    assert_eq!(session.project(), &before);
    assert!(session.redo(), "the batch should redo in one step");
    assert_eq!(
        scene_item_ids(session.project()),
        vec!["background", "third"]
    );
}

#[test]
fn nested_selection_and_group_ancestor_are_removed_atomically() {
    let mut project = project();
    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group body")
        .add_item(SceneItemSpec::new("nested", "background").expect("nested item"))
        .expect("nested insertion");
    group
        .group_mut()
        .expect("group body")
        .add_item(SceneItemSpec::new("remaining", "background").expect("remaining item"))
        .expect("remaining insertion");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: group,
        })
        .expect("group insertion");

    project
        .apply(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["overlay-group/nested".to_owned(), "background".to_owned()],
        })
        .expect("root and nested selection should be removed");
    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("main scene after removal");
    assert!(scene.item("background").is_none());
    assert!(scene.item("overlay-group").is_some());
    assert!(scene
        .item("overlay-group")
        .and_then(SceneItemSpec::group)
        .is_some_and(|group| {
            group
                .items()
                .iter()
                .all(|item| item.id().as_str() != "nested")
                && group
                    .items()
                    .iter()
                    .any(|item| item.id().as_str() == "remaining")
        }));

    // Selecting both a group and its descendant is one user gesture; the
    // ancestor owns the descendant and is removed only once.
    project
        .apply(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec![
                "overlay-group".to_owned(),
                "overlay-group/remaining".to_owned(),
            ],
        })
        .expect("the group ancestor should subsume its selected descendant");
    assert!(project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .and_then(|scene| scene.item("overlay-group"))
        .is_none());
}

#[test]
fn locked_batch_selection_fails_without_partial_removal() {
    let mut project = project();
    add_item(&mut project, "safe");
    project
        .apply(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "background".to_owned(),
            locked: true,
        })
        .expect("lock item");
    let before = project.clone();

    let error = project
        .apply(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["safe".to_owned(), "background".to_owned()],
        })
        .expect_err("a locked target must reject the complete batch");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("background").expect("identifier"))
    );
    assert_eq!(project, before);
}

#[test]
fn locked_group_parent_rejects_nested_batch_without_partial_removal() {
    let mut project = project();
    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group body")
        .add_item(SceneItemSpec::new("nested", "background").expect("nested item"))
        .expect("nested insertion");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: group,
        })
        .expect("group insertion");
    project
        .apply(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            item: "overlay-group".to_owned(),
            locked: true,
        })
        .expect("lock group");
    let before = project.clone();

    let error = project
        .apply(ProjectCommand::RemoveSceneItems {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            items: vec!["overlay-group/nested".to_owned(), "background".to_owned()],
        })
        .expect_err("a locked ancestor must reject the complete batch");
    assert_eq!(
        error,
        ProjectError::LockedSceneItem(Identifier::new("overlay-group").expect("identifier"))
    );
    assert_eq!(project, before);
}
