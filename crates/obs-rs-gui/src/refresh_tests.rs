use super::*;
use obs_rs_project::{ProjectCommand, SceneItemSpec};

#[test]
fn source_rows_expose_nested_group_targets_and_relative_order() {
    let mut project = crate::initial_project().expect("initial project");
    let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::for_source("background").expect("group child"))
        .expect("group child attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        })
        .expect("add group");
    let mut state = DesktopState::new(project);
    {
        let profile = state
            .project_session()
            .project()
            .active_profile_spec()
            .expect("profile");
        let scene = profile.scene("preview").expect("preview scene");
        let mut rows = Vec::new();
        append_source_rows(&mut rows, profile, scene.items(), &state, &mut Vec::new());

        let group_row = rows
            .iter()
            .find(|row| row.target == "overlay-group")
            .expect("group row");
        assert!(group_row.group);
        assert!(!group_row.nested);
        assert_eq!(group_row.count, 2);

        let child_row = rows
            .iter()
            .find(|row| row.target == "overlay-group/background")
            .expect("group child row");
        assert!(!child_row.group);
        assert!(child_row.nested);
        assert!(!child_row.selected);
        assert_eq!(child_row.count, 1);
        assert_eq!(child_row.order.as_str(), "1");
    }

    state
        .dispatch(UiCommand::SelectSource {
            id: "overlay-group/background".to_owned(),
        })
        .expect("nested source selection");
    let profile = state
        .project_session()
        .project()
        .active_profile_spec()
        .expect("profile after selection");
    let scene = profile
        .scene("preview")
        .expect("preview scene after selection");
    let mut rows = Vec::new();
    append_source_rows(&mut rows, profile, scene.items(), &state, &mut Vec::new());
    assert!(rows
        .iter()
        .find(|row| row.target == "overlay-group/background")
        .is_some_and(|row| row.selected));
}
