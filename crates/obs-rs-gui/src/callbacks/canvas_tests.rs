use super::*;

use obs_rs_project::{ProjectCommand, SceneItemSpec};

const CANVAS: (u32, u32) = (1_920, 1_080);

fn rect() -> ItemRect {
    ItemRect {
        x: 100,
        y: 50,
        width: 400,
        height: 300,
    }
}

#[test]
fn canvas_zoom_accepts_only_the_bounded_reference_presets() {
    assert_eq!(CanvasZoom::from_ui_value(0), Some(CanvasZoom::FitToWindow));
    assert_eq!(CanvasZoom::from_ui_value(25), Some(CanvasZoom::Percent(25)));
    assert_eq!(CanvasZoom::from_ui_value(50), Some(CanvasZoom::Percent(50)));
    assert_eq!(
        CanvasZoom::from_ui_value(100),
        Some(CanvasZoom::Percent(100))
    );
    assert_eq!(
        CanvasZoom::from_ui_value(200),
        Some(CanvasZoom::Percent(200))
    );
    assert_eq!(CanvasZoom::from_ui_value(75), None);
}

#[test]
fn canvas_zoom_steps_stay_inside_the_preset_bounds() {
    assert_eq!(CanvasZoom::FitToWindow.stepped(-1), CanvasZoom::FitToWindow);
    assert_eq!(CanvasZoom::FitToWindow.stepped(1), CanvasZoom::Percent(25));
    assert_eq!(CanvasZoom::Percent(25).stepped(-1), CanvasZoom::FitToWindow);
    assert_eq!(
        CanvasZoom::Percent(100).stepped(1),
        CanvasZoom::Percent(200)
    );
    assert_eq!(
        CanvasZoom::Percent(200).stepped(1),
        CanvasZoom::Percent(200)
    );
    assert_eq!(CanvasZoom::Percent(63).stepped(-1), CanvasZoom::Percent(50));
    assert_eq!(CanvasZoom::Percent(63).stepped(1), CanvasZoom::Percent(100));
}

#[test]
fn wheel_zoom_is_continuous_and_bounded() {
    assert_eq!(
        CanvasZoom::FitToWindow.wheel(1, 500_000),
        CanvasZoom::Percent(63)
    );
    assert_eq!(
        CanvasZoom::Percent(63).wheel(-1, 1_000_000),
        CanvasZoom::Percent(50)
    );
    assert_eq!(
        CanvasZoom::Percent(MIN_ZOOM_PERCENT).wheel(-1, 1_000_000),
        CanvasZoom::Percent(MIN_ZOOM_PERCENT)
    );
    assert_eq!(
        CanvasZoom::Percent(MAX_ZOOM_PERCENT).wheel(1, 1_000_000),
        CanvasZoom::Percent(MAX_ZOOM_PERCENT)
    );
}

#[test]
fn cursor_anchored_zoom_preserves_the_canvas_point_under_the_pointer() {
    let state = CanvasState::default()
        .with_zoom(CanvasZoom::Percent(100))
        .zoomed_at(1, (400, 250), (500, 300), (100, 50), 1_000_000);

    assert_eq!(state.zoom(), CanvasZoom::Percent(125));
    assert_eq!(state.pan(), (-80, -50));

    let new_scale = i64::from(state.zoom().ui_value()) * SCALE_MICROS_PER_PERCENT;
    let new_origin_x = 100_i64 * SCALE_MICROS_PER_UNIT + i64::from(state.pan().0) * new_scale;
    let new_origin_y = 50_i64 * SCALE_MICROS_PER_UNIT + i64::from(state.pan().1) * new_scale;
    assert_eq!(
        new_origin_x + 400_i64 * new_scale,
        500_i64 * SCALE_MICROS_PER_UNIT
    );
    assert_eq!(
        new_origin_y + 250_i64 * new_scale,
        300_i64 * SCALE_MICROS_PER_UNIT
    );
}

#[test]
fn canvas_state_keeps_viewport_state_outside_the_project_transform() {
    let state = CanvasState::default()
        .with_zoom(CanvasZoom::Percent(100))
        .with_snapping(SnapSettings {
            enabled: false,
            distance: 4,
        })
        .panned(24, -12);

    assert_eq!(state.zoom().ui_value(), 100);
    assert_eq!(state.pan(), (24, -12));
    assert_eq!(state.snapping().distance, 4);

    let bounded = state.panned(i32::MAX, i32::MIN);
    assert_eq!(bounded.pan(), (MAX_PAN_PIXELS, -MAX_PAN_PIXELS));
}

#[test]
fn snap_distance_is_bounded_at_the_canvas_boundary() {
    assert_eq!(
        CanvasState::default()
            .with_snap_distance(0)
            .snapping
            .distance,
        i64::from(*CANVAS_SNAP_DISTANCE_RANGE.start())
    );
    assert_eq!(
        CanvasState::default()
            .with_snap_distance(u16::MAX)
            .snapping
            .distance,
        i64::from(*CANVAS_SNAP_DISTANCE_RANGE.end())
    );

    let controller = CanvasController {
        draft: RefCell::new(None),
        rotation: RefCell::new(None),
        state: RefCell::new(CanvasState::default()),
    };
    assert_eq!(
        controller.set_snap_distance(24).snapping.distance,
        24,
        "the controller applies the settings snapshot to its one live policy"
    );
}

#[test]
fn safe_area_guides_match_ebu_r95_reference_margins() {
    let guides = SnapGuides::with_canvas(CANVAS);
    let x = &guides.x[..guides.x_len];
    let y = &guides.y[..guides.y_len];

    assert!(x.contains(&67), "3.5% action-safe left edge");
    assert!(x.contains(&1_853), "3.5% action-safe right edge");
    assert!(x.contains(&96), "5% graphics-safe left edge");
    assert!(x.contains(&1_824), "5% graphics-safe right edge");
    assert!(x.contains(&312), "16.25% 4:3-safe left edge");
    assert!(x.contains(&1_608), "16.25% 4:3-safe right edge");
    assert!(y.contains(&38), "3.5% action-safe top edge");
    assert!(y.contains(&1_042), "3.5% action-safe bottom edge");
    assert!(y.contains(&54), "5% graphics-safe top edge");
    assert!(y.contains(&1_026), "5% graphics-safe bottom edge");
}

#[test]
fn snapping_aligns_moves_and_resizes_to_bounded_guides() {
    let mut guides = SnapGuides::with_canvas(CANVAS);
    guides.push_rect(ItemRect {
        x: 700,
        y: 200,
        width: 100,
        height: 100,
    });
    let settings = SnapSettings::default();

    let near_edge = snap_rect(
        ItemRect {
            x: 6,
            y: 50,
            width: 400,
            height: 300,
        },
        0,
        &guides,
        settings,
    );
    assert_eq!(near_edge.x, 0, "the left edge should snap to the canvas");

    let near_safe_area = snap_rect(
        ItemRect {
            x: 90,
            y: 100,
            width: 200,
            height: 100,
        },
        0,
        &SnapGuides::with_canvas(CANVAS),
        settings,
    );
    assert_eq!(
        near_safe_area.x, 96,
        "the left edge should snap to the graphics-safe area"
    );

    let near_other = snap_rect(
        ItemRect {
            x: 296,
            y: 50,
            width: 400,
            height: 300,
        },
        0,
        &guides,
        settings,
    );
    assert_eq!(
        near_other.x, 300,
        "the right edge should snap to another source"
    );

    let resized = snap_rect(drag_rect(rect(), 8, -96, 0), 8, &guides, settings);
    assert_eq!((resized.x, resized.width), (0, 500));

    let unchanged = snap_rect(
        ItemRect {
            x: 6,
            y: 50,
            width: 400,
            height: 300,
        },
        0,
        &guides,
        SnapSettings {
            enabled: false,
            distance: 10,
        },
    );
    assert_eq!(unchanged.x, 6);
}

#[test]
fn an_identity_transform_covers_the_whole_canvas() {
    let rect = item_rect(FrameTransform::IDENTITY, CANVAS);

    assert_eq!(
        rect,
        ItemRect {
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080
        }
    );
}

#[test]
fn nested_group_canvas_projection_uses_stable_leaf_paths_and_round_trips_local_geometry() {
    let mut project = crate::initial_project().expect("initial project");
    let mut group = SceneItemSpec::for_group("canvas-group", "Canvas group").expect("group");
    let group_transform =
        FrameTransform::new(1_500, 1_250, 48, 24, true, false, 230).expect("group transform");
    group.set_transform(group_transform);
    let child_transform =
        FrameTransform::new(800, 700, 36, -12, false, false, 210).expect("child transform");
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
        .expect("add canvas group");

    let profile = project.profile("live").expect("profile");
    let scene = profile.scene("preview").expect("scene");
    let canvas = (
        profile.video_format().width(),
        profile.video_format().height(),
    );
    let parent = group_parent_transform(scene, "canvas-group/background", canvas)
        .expect("group parent transform");
    assert_eq!(parent, group_transform);
    let effective = child_transform
        .compose_axis_aligned(parent, canvas.0, canvas.1)
        .expect("effective transform");
    assert_eq!(
        local_transform_for_canvas_item(profile, scene, "canvas-group/background", effective),
        Some(child_transform)
    );

    let state = DesktopState::new(project);
    let projections = canvas_item_projections(&state, "preview", canvas);
    assert_eq!(
        projections
            .iter()
            .map(|item| item.target.as_str())
            .collect::<Vec<_>>(),
        vec!["background", "canvas-group", "canvas-group/background"]
    );
    let projected = projections
        .iter()
        .find(|item| item.target == "canvas-group/background")
        .expect("nested leaf is projected");
    assert_eq!(projected.transform, effective);
    assert_eq!(projected.parent_transform, parent);
}

#[test]
fn nested_canvas_target_inherits_lock_from_each_group_ancestor() {
    let mut project = crate::initial_project().expect("initial project");
    let mut group = SceneItemSpec::for_group("locked-group", "Locked group").expect("group");
    group.set_locked(true);
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::for_source("background").expect("group child"))
        .expect("group child");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        })
        .expect("add locked group");
    let scene = project
        .profile("live")
        .and_then(|profile| profile.scene("preview"))
        .expect("scene");

    assert!(canvas_target_is_locked(scene, "locked-group/background"));
    assert!(canvas_target_is_locked(scene, "locked-group/missing"));
    assert!(!canvas_target_is_locked(scene, "background"));
}

#[test]
fn dragging_the_body_moves_without_resizing() {
    let moved = drag_rect(rect(), 0, 25, -10);

    assert_eq!(moved.x, 125);
    assert_eq!(moved.y, 40);
    assert_eq!((moved.width, moved.height), (400, 300));
}

#[test]
fn opposite_edges_stay_put_while_a_handle_is_dragged() {
    // Dragging the left edge right must not move the right edge.
    let edited = drag_rect(rect(), 8, 40, 0);
    assert_eq!(edited.x, 140);
    assert_eq!(edited.x + edited.width, 500);

    // Dragging the top edge down must not move the bottom edge.
    let edited = drag_rect(rect(), 2, 0, 30);
    assert_eq!(edited.y, 80);
    assert_eq!(edited.y + edited.height, 350);

    // The bottom-right corner moves only that corner.
    let edited = drag_rect(rect(), 5, 20, 15);
    assert_eq!((edited.x, edited.y), (100, 50));
    assert_eq!((edited.width, edited.height), (420, 315));
}

#[test]
fn free_resize_covers_all_handles_and_keeps_the_opposite_edges_fixed() {
    let base = rect();
    let cases = [
        (
            1,
            20,
            15,
            ItemRect {
                x: 120,
                y: 65,
                width: 380,
                height: 285,
            },
        ),
        (
            2,
            20,
            15,
            ItemRect {
                x: 100,
                y: 65,
                width: 400,
                height: 285,
            },
        ),
        (
            3,
            20,
            15,
            ItemRect {
                x: 100,
                y: 65,
                width: 420,
                height: 285,
            },
        ),
        (
            4,
            20,
            15,
            ItemRect {
                x: 100,
                y: 50,
                width: 420,
                height: 300,
            },
        ),
        (
            5,
            20,
            15,
            ItemRect {
                x: 100,
                y: 50,
                width: 420,
                height: 315,
            },
        ),
        (
            6,
            20,
            15,
            ItemRect {
                x: 100,
                y: 50,
                width: 400,
                height: 315,
            },
        ),
        (
            7,
            20,
            15,
            ItemRect {
                x: 120,
                y: 50,
                width: 380,
                height: 315,
            },
        ),
        (
            8,
            20,
            15,
            ItemRect {
                x: 120,
                y: 50,
                width: 380,
                height: 300,
            },
        ),
    ];

    for (handle, dx, dy, expected) in cases {
        assert_eq!(
            drag_rect(base, handle, dx, dy),
            expected,
            "free resize handle {handle}"
        );
    }
}

#[test]
fn aspect_resize_covers_all_handles_and_keeps_the_opposite_anchor() {
    let base = rect();
    let cases = [
        (1, -80, -60, 20, -10),
        (2, 0, -60, 60, -10),
        (3, 80, -60, 100, -10),
        (4, 80, 0, 100, 20),
        (5, 80, 60, 100, 50),
        (6, 0, 60, 60, 50),
        (7, -80, 60, 20, 50),
        (8, -80, 0, 20, 20),
    ];

    for (handle, dx, dy, expected_x, expected_y) in cases {
        let raw = drag_rect(base, handle, dx, dy);
        let edited = preserve_resize_aspect(base, raw, handle, 4, 3);
        assert_eq!(
            (edited.x, edited.y, edited.width, edited.height),
            (expected_x, expected_y, 480, 360),
            "aspect resize handle {handle}"
        );
        assert_eq!(edited.width * 3, edited.height * 4);
    }
}

#[test]
fn minimum_size_is_enforced_for_every_resize_handle() {
    let base = rect();
    let cases = [
        (
            1,
            1_000,
            1_000,
            484,
            334,
            MINIMUM_ITEM_PIXELS,
            MINIMUM_ITEM_PIXELS,
        ),
        (2, 0, 1_000, 100, 334, 400, MINIMUM_ITEM_PIXELS),
        (
            3,
            -1_000,
            1_000,
            100,
            334,
            MINIMUM_ITEM_PIXELS,
            MINIMUM_ITEM_PIXELS,
        ),
        (4, -1_000, 0, 100, 50, MINIMUM_ITEM_PIXELS, 300),
        (
            5,
            -1_000,
            -1_000,
            100,
            50,
            MINIMUM_ITEM_PIXELS,
            MINIMUM_ITEM_PIXELS,
        ),
        (6, 0, -1_000, 100, 50, 400, MINIMUM_ITEM_PIXELS),
        (
            7,
            1_000,
            -1_000,
            484,
            50,
            MINIMUM_ITEM_PIXELS,
            MINIMUM_ITEM_PIXELS,
        ),
        (8, 1_000, 0, 484, 50, MINIMUM_ITEM_PIXELS, 300),
    ];

    for (handle, dx, dy, expected_x, expected_y, expected_width, expected_height) in cases {
        let edited = drag_rect(base, handle, dx, dy);
        assert_eq!(
            (edited.x, edited.y, edited.width, edited.height),
            (expected_x, expected_y, expected_width, expected_height),
            "minimum size handle {handle}"
        );
    }
}

#[test]
fn ordinary_resize_preserves_aspect_and_shift_allows_free_resize() {
    let base = rect();
    let raw = drag_rect(base, 5, 100, 10);
    let preserved = preserve_resize_aspect(base, raw, 5, 4, 3);
    assert_eq!(
        preserved,
        ItemRect {
            x: 100,
            y: 50,
            width: 500,
            height: 375,
        }
    );

    let raw_side = drag_rect(base, 4, 100, 0);
    let preserved_side = preserve_resize_aspect(base, raw_side, 4, 4, 3);
    assert_eq!(
        preserved_side,
        ItemRect {
            x: 100,
            y: 13,
            width: 500,
            height: 375,
        }
    );

    let free = raw;
    assert_eq!((free.width, free.height), (500, 310));
    assert_eq!(
        CanvasResizeModifiers::from_mask(0),
        CanvasResizeModifiers {
            preserve_aspect: true,
            snapping: true,
            crop: false,
        }
    );
    assert_eq!(
        CanvasResizeModifiers::from_mask(RESIZE_MODIFIER_SHIFT),
        CanvasResizeModifiers {
            preserve_aspect: false,
            snapping: true,
            crop: false,
        }
    );
    assert_eq!(
        CanvasResizeModifiers::from_mask(RESIZE_MODIFIER_CONTROL),
        CanvasResizeModifiers {
            preserve_aspect: true,
            snapping: false,
            crop: false,
        }
    );
    assert_eq!(
        CanvasResizeModifiers::from_mask(
            RESIZE_MODIFIER_SHIFT | RESIZE_MODIFIER_CONTROL | RESIZE_MODIFIER_ALT
        ),
        CanvasResizeModifiers {
            preserve_aspect: false,
            snapping: false,
            crop: true,
        }
    );
}

#[test]
fn a_source_cannot_be_shrunk_past_its_handles() {
    let collapsed = drag_rect(rect(), 5, -1_000, -1_000);
    assert_eq!(collapsed.width, MINIMUM_ITEM_PIXELS);
    assert_eq!(collapsed.height, MINIMUM_ITEM_PIXELS);

    // Pulling a left edge past the limit must not drag the origin beyond
    // the opposite edge either.
    let collapsed = drag_rect(rect(), 8, 1_000, 0);
    assert_eq!(collapsed.width, MINIMUM_ITEM_PIXELS);
    assert_eq!(collapsed.x + collapsed.width, 500);
}

#[test]
fn an_edited_rectangle_round_trips_through_the_transform() {
    let edited = ItemRect {
        x: 240,
        y: 135,
        width: 960,
        height: 540,
    };

    let transform = transform_for_rect(FrameTransform::IDENTITY, edited, CANVAS);

    assert_eq!(transform.scale_x_milli(), 500);
    assert_eq!(transform.scale_y_milli(), 500);
    assert_eq!(item_rect(transform, CANVAS), edited);
}

#[test]
fn rebuilding_a_transform_keeps_flips_and_opacity() {
    let base = FrameTransform::new(1_000, 1_000, 0, 0, true, true, 128).expect("transform");

    let rebuilt = transform_for_rect(base, rect(), CANVAS);

    assert!(rebuilt.flip_x() && rebuilt.flip_y());
    assert_eq!(rebuilt.opacity(), 128);
}

#[test]
fn alt_handle_crop_changes_source_edges_without_losing_the_opposite_edge() {
    let base = FrameTransform::IDENTITY;
    let left = crop_transform(base, 8, 100, 0, CANVAS);
    assert_eq!(left.crop_left(), 100);
    assert_eq!(left.crop_right(), 0);
    assert_eq!(left.translate_x(), 100);
    assert_eq!(
        item_rect(left, CANVAS).x + item_rect(left, CANVAS).width,
        1_920
    );

    let right = crop_transform(base, 4, -100, 0, CANVAS);
    assert_eq!(right.crop_left(), 0);
    assert_eq!(right.crop_right(), 100);
    assert_eq!(right.translate_x(), 0);
    assert_eq!(item_rect(right, CANVAS).width, 1_820);
}

#[test]
fn alt_crop_covers_all_handles_without_encoding_crop_as_a_handle_id() {
    let base = FrameTransform::IDENTITY;
    let cases = [
        (1, 100, 50, (100, 50, 0, 0, 100, 50)),
        (2, 0, 50, (0, 50, 0, 0, 0, 50)),
        (3, -100, 50, (0, 50, 100, 0, 0, 50)),
        (4, -100, 0, (0, 0, 100, 0, 0, 0)),
        (5, -100, -50, (0, 0, 100, 50, 0, 0)),
        (6, 0, -50, (0, 0, 0, 50, 0, 0)),
        (7, 100, -50, (100, 0, 0, 50, 100, 0)),
        (8, 100, 0, (100, 0, 0, 0, 100, 0)),
    ];

    for (handle, dx, dy, expected) in cases {
        let cropped = crop_transform(base, handle, dx, dy, CANVAS);
        assert_eq!(
            (
                cropped.crop_left(),
                cropped.crop_top(),
                cropped.crop_right(),
                cropped.crop_bottom(),
                cropped.translate_x(),
                cropped.translate_y(),
            ),
            expected,
            "crop handle {handle}"
        );
    }
}

#[test]
fn rotated_alt_crop_maps_pointer_deltas_into_source_axes() {
    let base = FrameTransform::new(1_000, 1_000, 100, 50, false, false, 255)
        .expect("transform")
        .with_rotation_degrees(90)
        .expect("rotation");

    // At 90 degrees, source-local +X points down on the canvas. Moving
    // the left crop edge down therefore retains the opposite edge by
    // moving the visible rectangle down in canvas space.
    let left = crop_transform(base, 8, 0, 100, CANVAS);
    assert_eq!(left.crop_left(), 100);
    assert_eq!(left.crop_top(), 0);
    assert_eq!((left.translate_x(), left.translate_y()), (100, 150));

    // Source-local +Y points left on the canvas at the same rotation.
    let top = crop_transform(base, 2, -100, 0, CANVAS);
    assert_eq!(top.crop_left(), 0);
    assert_eq!(top.crop_top(), 100);
    assert_eq!((top.translate_x(), top.translate_y()), (0, 50));
}

#[test]
fn rotation_uses_the_rotated_axis_aligned_bounds_for_selection() {
    let transform = FrameTransform::new(1_000, 1_000, 100, 50, false, false, 255)
        .expect("transform")
        .with_rotation_degrees(90)
        .expect("rotation");

    assert_eq!(
        item_rect(transform, CANVAS),
        ItemRect {
            x: 520,
            y: -370,
            width: 1_080,
            height: 1_920,
        }
    );
}

#[test]
fn rotated_selection_overlay_uses_oriented_handles() {
    let transform = FrameTransform::new(500, 500, 100, 50, false, false, 255)
        .expect("transform")
        .with_rotation_degrees(90)
        .expect("rotation");
    let overlay = selection_overlay_for_transforms(&[transform], (400, 300))
        .expect("one transform should create an overlay");

    assert_eq!(
        overlay.rect,
        ItemRect {
            x: 125,
            y: 25,
            width: 150,
            height: 200
        }
    );
    assert_eq!(overlay.handle_x, [275, 275, 275, 200, 125, 125, 125, 200]);
    assert_eq!(overlay.handle_y, [25, 125, 225, 225, 225, 125, 25, 25]);
    assert_eq!(overlay.path, "M 275 25 L 275 225 L 125 225 L 125 25 Z");
    assert_eq!(overlay.rotation_handle, Some((307, 125)));
}

#[test]
fn rotation_handle_is_published_for_single_and_group_selection() {
    let single = selection_overlay_for_transforms(&[FrameTransform::IDENTITY], (400, 300))
        .expect("one transform should create an overlay");
    assert_eq!(single.rotation_handle, Some((200, -32)));

    let second = FrameTransform::new(500, 500, 100, 50, false, false, 255).expect("transform");
    let group = selection_overlay_for_transforms(&[FrameTransform::IDENTITY, second], (400, 300))
        .expect("two transforms should create an overlay");
    assert_eq!(group.rotation_handle, Some((200, -32)));
}

#[test]
fn rotation_pointer_uses_a_stable_base_and_obs_style_modifiers() {
    let base = FrameTransform::IDENTITY;
    let anchor = (200, -32);

    let quarter_turn = rotation_from_pointer(base, anchor, (382, 150), 0, (400, 300));
    assert_eq!(quarter_turn.rotation_milli_degrees(), 90_000);

    // A 22-degree pointer move is rounded to the nearest 15-degree increment
    // while Shift is held.
    let shift_snapped =
        rotation_from_pointer(base, anchor, (268, -19), RESIZE_MODIFIER_SHIFT, (400, 300));
    assert_eq!(shift_snapped.rotation_milli_degrees(), 15_000);

    // The same pointer position is close enough to the ordinary 45-degree
    // guide to snap without Ctrl, but Ctrl preserves the measured angle.
    let default_snapped = rotation_from_pointer(base, anchor, (340, 20), 0, (400, 300));
    let control_free =
        rotation_from_pointer(base, anchor, (340, 20), RESIZE_MODIFIER_CONTROL, (400, 300));
    assert_eq!(default_snapped.rotation_milli_degrees(), 45_000);
    assert_ne!(control_free.rotation_milli_degrees(), 45_000);
    assert!((46_000..=48_000).contains(&control_free.rotation_milli_degrees()));
}

#[test]
fn group_rotation_uses_one_pivot_and_preserves_each_item_geometry() {
    let canvas = (400, 300);
    let first = FrameTransform::new(500, 500, 50, 100, false, false, 255).expect("first transform");
    let second =
        FrameTransform::new(250, 500, 250, 100, false, false, 255).expect("second transform");
    let center = (200, 175);
    let delta = group_rotation_from_pointer(center, (200, 75), (300, 175), 0);
    assert_eq!(delta, 90_000);

    let rotated_first = rotate_transform_around_point(first, center, delta, canvas);
    let rotated_second = rotate_transform_around_point(second, center, delta, canvas);
    assert_eq!(
        (rotated_first.translate_x(), rotated_first.translate_y()),
        (100, 50)
    );
    assert_eq!(
        (rotated_second.translate_x(), rotated_second.translate_y()),
        (150, 200)
    );
    assert_eq!(rotated_first.rotation_milli_degrees(), 90_000);
    assert_eq!(rotated_second.rotation_milli_degrees(), 90_000);
    assert_eq!(
        (rotated_first.scale_x_milli(), rotated_first.scale_y_milli()),
        (first.scale_x_milli(), first.scale_y_milli())
    );
    assert_eq!(
        (
            rotated_second.scale_x_milli(),
            rotated_second.scale_y_milli()
        ),
        (second.scale_x_milli(), second.scale_y_milli())
    );
}

#[test]
fn rotated_resize_maps_pointer_to_local_axes_and_keeps_opposite_corner_fixed() {
    let base = FrameTransform::new(500, 500, 100, 50, false, false, 255)
        .expect("transform")
        .with_rotation_degrees(90)
        .expect("rotation");
    let canvas = (400, 300);
    let old_handles = oriented_handle_points(base, canvas);
    assert_eq!(rotate_canvas_delta(30, 0, 90_000, true), (0, -30));

    let local = drag_rect(local_item_rect(base, canvas), 1, 0, -30);
    let resized = transform_for_rotated_local_rect(base, local, 1, canvas);
    let new_handles = oriented_handle_points(resized, canvas);

    assert_eq!(resized.scale_x_milli(), 500);
    assert_eq!(resized.scale_y_milli(), 600);
    assert_eq!((resized.translate_x(), resized.translate_y()), (115, 35));
    assert_eq!(new_handles[4], old_handles[4]);
}

#[test]
fn multi_selection_overlay_keeps_axis_aligned_group_handles() {
    let first = FrameTransform::new(250, 250, 20, 30, false, false, 255).expect("transform");
    let second = FrameTransform::new(250, 250, 160, 100, false, false, 255).expect("transform");
    let overlay = selection_overlay_for_transforms(&[first, second], (400, 300))
        .expect("two transforms should create an overlay");

    assert_eq!(
        overlay.rect,
        ItemRect {
            x: 20,
            y: 30,
            width: 240,
            height: 145
        }
    );
    assert_eq!(overlay.handle_x, [20, 140, 260, 260, 260, 140, 20, 20]);
    assert_eq!(overlay.handle_y, [30, 30, 30, 102, 175, 175, 175, 102]);
}

#[test]
fn transform_commands_apply_screen_sizing_and_alignment() {
    let canvas = (1_280, 720);
    let base = FrameTransform::IDENTITY
        .with_crop(140, 0, 140, 0)
        .expect("crop")
        .with_rotation_degrees(0)
        .expect("rotation");

    let fit = transform_for_command(base, CanvasTransformCommand::FitToScreen, canvas);
    assert_eq!(fit.scale_x_milli(), 1_000);
    assert_eq!(fit.scale_y_milli(), 1_000);
    assert_eq!(fit.translate_x(), 140);
    assert_eq!(fit.translate_y(), 0);

    let stretch = transform_for_command(base, CanvasTransformCommand::StretchToScreen, canvas);
    assert_eq!(stretch.scale_x_milli(), 1_280);
    assert_eq!(stretch.scale_y_milli(), 1_000);
    assert_eq!(stretch.translate_x(), 0);
    assert_eq!(stretch.translate_y(), 0);

    let positioned = FrameTransform::new(500, 250, 100, 50, true, false, 180).expect("transform");
    let centered =
        transform_for_command(positioned, CanvasTransformCommand::CenterToScreen, canvas);
    assert_eq!(
        item_rect(centered, canvas),
        ItemRect {
            x: 320,
            y: 270,
            width: 640,
            height: 180,
        }
    );
    assert!(centered.flip_x());
    assert_eq!(centered.opacity(), 180);

    let right = transform_for_command(positioned, CanvasTransformCommand::AlignRight, canvas);
    assert_eq!(item_rect(right, canvas).x, 640);
    let bottom = transform_for_command(positioned, CanvasTransformCommand::AlignBottom, canvas);
    assert_eq!(item_rect(bottom, canvas).y, 540);
}

#[test]
fn transform_command_parser_rejects_untrusted_actions() {
    assert_eq!(
        CanvasTransformCommand::from_action("fit-screen"),
        Some(CanvasTransformCommand::FitToScreen)
    );
    assert_eq!(CanvasTransformCommand::from_action("delete-project"), None);
}

#[test]
fn selection_box_normalizes_reverse_drag_and_group_mapping() {
    let state = CanvasState::default().begin_selection(400, 300, true);
    let state = state.update_selection(100, 50);
    assert_eq!(
        state.selection_box(),
        Some(ItemRect {
            x: 100,
            y: 50,
            width: 300,
            height: 250,
        })
    );
    let mapped = map_rect_into_group(
        ItemRect {
            x: 200,
            y: 150,
            width: 100,
            height: 100,
        },
        ItemRect {
            x: 100,
            y: 50,
            width: 400,
            height: 300,
        },
        ItemRect {
            x: 200,
            y: 100,
            width: 800,
            height: 600,
        },
    );
    assert_eq!(
        mapped,
        ItemRect {
            x: 400,
            y: 300,
            width: 200,
            height: 200,
        }
    );
    assert!(ItemRect {
        x: 400,
        y: 300,
        width: 10,
        height: 10,
    }
    .intersects(ItemRect {
        x: 405,
        y: 305,
        width: 10,
        height: 10,
    }));
}

#[test]
fn hit_testing_uses_the_item_rectangle() {
    let rect = rect();

    assert!(rect.contains(100, 50));
    assert!(rect.contains(499, 349));
    assert!(!rect.contains(99, 50));
    assert!(!rect.contains(500, 350));
}

#[test]
fn plain_preview_selection_skips_selected_hits_to_reach_underneath() {
    let hits = [("top", true), ("middle", true), ("bottom", false)];
    assert_eq!(
        first_selectable_hit(hits.iter().copied(), true),
        Some("bottom")
    );
    assert_eq!(
        first_selectable_hit(hits.iter().copied(), false),
        Some("top")
    );

    let hits = [("top", false), ("underneath", true)];
    assert_eq!(
        first_selectable_hit(hits.iter().copied(), true),
        Some("top")
    );

    let hits = [("top", true)];
    assert_eq!(first_selectable_hit(hits.iter().copied(), true), None);
}

/// Measures the bounded group-geometry work used for every multi-select
/// pointer sample. The report is ignored so it can be run on release
/// builds without becoming a machine-dependent pass/fail gate.
#[test]
#[ignore = "timing report, not a pass/fail assertion"]
fn multi_selection_geometry_timing_report() {
    use std::time::Instant;

    let mut items = (0..16)
        .map(|index| TransformDraftItem {
            item: format!("item_{index}"),
            transform: FrameTransform::new(
                400 + index * 20,
                350 + index * 15,
                i32::try_from(index * 73).expect("translation"),
                i32::try_from(index * 41).expect("translation"),
                false,
                false,
                255,
            )
            .expect("transform"),
            parent_transform: FrameTransform::IDENTITY,
        })
        .collect::<Vec<_>>();
    let runs = 200;
    let started = Instant::now();
    let mut checksum = 0_i64;
    for _ in 0..runs {
        let group = items
            .iter()
            .map(|item| item_rect(item.transform, CANVAS))
            .reduce(ItemRect::union)
            .expect("group");
        let moved = drag_rect(group, 0, 12, -7);
        for item in &mut items {
            let old = item_rect(item.transform, CANVAS);
            let next = ItemRect {
                x: old.x.saturating_add(moved.x.saturating_sub(group.x)),
                y: old.y.saturating_add(moved.y.saturating_sub(group.y)),
                ..old
            };
            item.transform = transform_for_rect(item.transform, next, CANVAS);
            checksum = checksum.saturating_add(i64::from(item.transform.translate_x()));
        }
    }
    let rotation_bases = items.iter().map(|item| item.transform).collect::<Vec<_>>();
    let rotation_started = Instant::now();
    let pivot = (960, 540);
    let delta = group_rotation_from_pointer(pivot, (960, 508), (992, 540), 0);
    for _ in 0..runs {
        for base in &rotation_bases {
            let rotated = rotate_transform_around_point(*base, pivot, delta, CANVAS);
            checksum = checksum.saturating_add(i64::from(rotated.translate_x()));
        }
    }
    println!(
        "multi-selection: items={} runs={} move_per_sample={:?} rotation_per_sample={:?} checksum={checksum}",
        items.len(),
        runs,
        started.elapsed() / runs,
        rotation_started.elapsed() / runs,
    );
}
