use super::*;
use obs_rs_media::{StingerSpec, TransitionKind, TransitionSpec};

#[test]
fn stinger_resource_override_round_trips_and_clears_through_project_commands() {
    let mut project = project();
    let stinger =
        StingerSpec::new("assets/intro.webm", 625, true, false).expect("stinger resource");
    project
        .apply(ProjectCommand::SetSceneStingerOverride {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            stinger: Some(stinger.clone()),
        })
        .expect("set stinger resource");

    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("scene");
    assert_eq!(scene.stinger_override(), Some(&stinger));

    let encoded = project.serialize();
    assert!(encoded.contains(r#""stinger": {"#), "{encoded}");
    assert!(
        encoded.contains(r#""path": "assets/intro.webm""#),
        "{encoded}"
    );
    assert!(
        encoded.contains(r#""transition_point_milli": 625"#),
        "{encoded}"
    );
    let decoded = Project::parse(&encoded).expect("stinger project parses");
    assert_eq!(decoded, project);

    project
        .apply(ProjectCommand::SetSceneStingerOverride {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            stinger: None,
        })
        .expect("clear stinger resource");
    assert!(project
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .is_some_and(|scene| scene.stinger_override().is_none()));
}

#[test]
fn schema_seven_transition_documents_still_load_without_stinger_state() {
    let mut project = project();
    let transition = TransitionSpec::cross_fade(700).expect("transition");
    project
        .apply(ProjectCommand::SetSceneTransitionOverride {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            transition: Some(transition),
        })
        .expect("set transition");
    let legacy = project
        .serialize()
        .replace(r#""version": 8"#, r#""version": 7"#);
    let decoded = Project::parse(&legacy).expect("schema seven project parses");
    assert_eq!(
        decoded
            .profile("live")
            .and_then(|profile| profile.scene("main"))
            .and_then(SceneSpec::transition_override)
            .map(TransitionSpec::kind),
        Some(TransitionKind::CrossFade)
    );
    assert!(decoded
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .is_some_and(|scene| scene.stinger_override().is_none()));
}

#[test]
fn stinger_document_decode_rejects_invalid_resource_fields() {
    let invalid_path = project().serialize().replace(
        r#""stinger": null"#,
        r#""stinger": {"path": "", "transition_point_milli": 500, "preload": true, "hardware_decode": false}"#,
    );
    assert!(Project::parse(&invalid_path).is_err());

    let invalid_point = project().serialize().replace(
        r#""stinger": null"#,
        r#""stinger": {"path": "assets/intro.webm", "transition_point_milli": 1000, "preload": true, "hardware_decode": false}"#,
    );
    assert!(Project::parse(&invalid_point).is_err());
}
