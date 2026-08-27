use super::*;

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
        .with_rotation_milli_degrees(12_500)
        .expect("rotation")
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
        .item_mut(&source_id)
        .expect("scene item")
        .set_transform(transform);

    let decoded = Project::parse(&cropped.serialize()).expect("parse cropped project");
    let decoded_transform = decoded
        .profile("live")
        .expect("profile")
        .scene("main")
        .expect("scene")
        .item("background")
        .expect("scene item")
        .transform();
    assert_eq!(decoded_transform, transform);

    // Version two documents did not have a rotation member. They remain
    // readable and receive the identity rotation during migration.
    let previous = cropped
        .serialize()
        .replace(r#""version": 3"#, r#""version": 2"#)
        .replace(
            &format!(
                "        \"rotation_milli_degrees\": {},\n",
                transform.rotation_milli_degrees()
            ),
            "",
        );
    let previous_transform = Project::parse(&previous)
        .expect("version two project")
        .profile("live")
        .expect("profile")
        .scene("main")
        .expect("scene")
        .item("background")
        .expect("item")
        .transform();
    assert_eq!(previous_transform.rotation_milli_degrees(), 0);

    let legacy = project().serialize().replace(",0,0,0,0|", "|");
    Project::parse(&legacy).expect("seven-field legacy transforms remain readable");
}
