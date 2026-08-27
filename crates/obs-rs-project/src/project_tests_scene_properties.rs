use super::*;
use obs_rs_media::StingerSpec;

#[test]
fn scene_properties_change_name_transition_and_stinger_as_one_history_edit() {
    let mut session = ProjectSession::new(project());
    let transition = TransitionSpec::cross_fade(900).expect("transition");
    let stinger = StingerSpec::new("assets/intro.webm", 625, true, false).expect("stinger");
    session
        .dispatch(ProjectCommand::SetSceneProperties {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            name: "Program scene".to_owned(),
            transition: Some(transition),
            stinger: Some(stinger.clone()),
        })
        .expect("scene properties");
    let scene = session
        .project()
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("updated scene");
    assert_eq!(scene.name(), "Program scene");
    assert_eq!(scene.transition_override(), Some(transition));
    assert_eq!(scene.stinger_override(), Some(&stinger));

    assert!(session.undo());
    let scene = session
        .project()
        .profile("live")
        .and_then(|profile| profile.scene("main"))
        .expect("restored scene");
    assert_eq!(scene.name(), "Main scene");
    assert_eq!(scene.transition_override(), None);
    assert_eq!(scene.stinger_override(), None);

    let mut invalid_project = project();
    let before_invalid = invalid_project.clone();
    let error = invalid_project
        .apply(ProjectCommand::SetSceneProperties {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            name: "   ".to_owned(),
            transition: Some(transition),
            stinger: Some(stinger),
        })
        .expect_err("empty scene name must reject the complete edit");
    assert_eq!(error, ProjectError::InvalidName { kind: "scene" });
    assert_eq!(invalid_project, before_invalid);
}
