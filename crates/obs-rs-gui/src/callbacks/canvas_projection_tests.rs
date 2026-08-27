use super::*;

use obs_rs_project::ProjectCommand;

#[test]
fn nested_group_canvas_projection_round_trips_crop_and_rotation_under_uniform_parent() {
    let mut project = crate::initial_project().expect("initial project");
    let group_transform =
        FrameTransform::new(1_500, 1_500, 48, 24, false, false, 230).expect("group transform");
    let child_transform = FrameTransform::new(800, 800, 36, -12, false, false, 210)
        .expect("child transform")
        .with_rotation_degrees(15)
        .expect("child rotation")
        .with_crop(40, 20, 60, 30)
        .expect("child crop");
    let mut group =
        SceneItemSpec::for_group("canvas-uniform-group", "Uniform group").expect("group");
    group.set_transform(group_transform);
    let mut child = SceneItemSpec::for_source("background").expect("child");
    child.set_transform(child_transform);
    group
        .group_mut()
        .expect("group target")
        .add_item(child)
        .expect("group child");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        })
        .expect("add uniform canvas group");

    let profile = project.profile("live").expect("profile");
    let scene = profile.scene("preview").expect("scene");
    let canvas = (
        profile.video_format().width(),
        profile.video_format().height(),
    );
    let parent = canvas_parent_transform(
        profile,
        "preview",
        "canvas-uniform-group/background",
        canvas,
    )
    .expect("uniform group parent transform");
    assert_eq!(parent, group_transform);
    let effective = child_transform
        .compose_axis_aligned(parent, canvas.0, canvas.1)
        .expect("effective crop and rotation");
    assert_eq!(
        local_transform_for_canvas_item(
            profile,
            scene,
            "canvas-uniform-group/background",
            effective,
        ),
        Some(child_transform)
    );

    let state = DesktopState::new(project);
    let projected = canvas_item_projections(&state, "preview", canvas)
        .into_iter()
        .find(|item| item.target == "canvas-uniform-group/background")
        .expect("nested transformed leaf projection");
    assert_eq!(projected.transform, effective);
    assert_eq!(projected.parent_transform, parent);
}

#[test]
fn nested_scene_reference_canvas_projection_round_trips_crop_under_mirrored_parent() {
    let mut project = crate::initial_project().expect("initial project");
    let child_transform =
        FrameTransform::new(850, 1_100, 17, -10, false, true, 205).expect("child transform");
    let mut child_scene =
        SceneSpec::new("canvas-crop-child", "Canvas crop child").expect("child scene");
    let mut child_item = SceneItemSpec::for_source("background").expect("child item");
    child_item.set_transform(child_transform);
    child_scene.add_item(child_item).expect("child item attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child_scene,
        })
        .expect("add child scene");
    let parent_transform =
        FrameTransform::new(1_400, 1_200, 26, 18, true, false, 230).expect("parent transform");
    let mut reference = SceneItemSpec::for_scene("canvas-crop-child-ref", "canvas-crop-child")
        .expect("scene reference");
    reference.set_transform(parent_transform);
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: reference,
        })
        .expect("add scene reference");

    let profile = project.profile("live").expect("profile");
    let scene = profile.scene("preview").expect("scene");
    let canvas = (
        profile.video_format().width(),
        profile.video_format().height(),
    );
    let parent = canvas_parent_transform(
        profile,
        "preview",
        "canvas-crop-child-ref/background",
        canvas,
    )
    .expect("scene-reference parent transform");
    let cropped_child = child_transform
        .with_crop(40, 20, 60, 30)
        .expect("cropped child transform");
    let effective = cropped_child
        .compose_axis_aligned(parent, canvas.0, canvas.1)
        .expect("effective scene-reference crop");
    assert_eq!(
        local_transform_for_canvas_item(
            profile,
            scene,
            "canvas-crop-child-ref/background",
            effective,
        ),
        Some(cropped_child)
    );
}
