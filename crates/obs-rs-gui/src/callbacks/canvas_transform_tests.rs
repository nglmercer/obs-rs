use super::*;

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
