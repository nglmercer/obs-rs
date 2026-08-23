use super::*;

const ORDER: [i32; 5] = [1, 0, 2, 3, 4];

#[test]
fn a_dock_swaps_with_its_neighbour() {
    assert_eq!(reorder(&ORDER, 0, -1).expect("left"), [0, 1, 2, 3, 4]);
    assert_eq!(reorder(&ORDER, 0, 1).expect("right"), [1, 2, 0, 3, 4]);
}

#[test]
fn a_dock_at_the_end_of_the_row_stays_put() {
    assert!(reorder(&ORDER, 1, -1).is_none(), "already leftmost");
    assert!(reorder(&ORDER, 4, 1).is_none(), "already rightmost");
    assert!(reorder(&ORDER, 2, 0).is_none(), "no direction");
    assert!(reorder(&ORDER, 9, 1).is_none(), "unknown dock");
}

#[test]
fn a_splitter_drag_trades_width_between_its_neighbours() {
    let weights = [1.0, 1.0, 1.85, 1.0, 1.4];

    // Index 2 of the order sits between dock 0 and dock 2.
    let resized = resize(&weights, &ORDER, 2, 320);

    assert!((resized[0] - 2.0).abs() < 1e-5, "left dock grew");
    assert!((resized[2] - 0.85).abs() < 1e-5, "right dock shrank");
    let total: f32 = resized.iter().sum();
    assert!(
        (total - weights.iter().sum::<f32>()).abs() < 1e-5,
        "the row's total width must not change"
    );
}

fn bounds() -> DesktopBounds {
    desktop_bounds(&[
        crate::fixtures::MonitorChoice {
            id: "DP-1".to_owned(),
            name: "DP-1".to_owned(),
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080,
            primary: true,
        },
        crate::fixtures::MonitorChoice {
            id: "HDMI-1".to_owned(),
            name: "HDMI-1".to_owned(),
            x: -1_280,
            y: 120,
            width: 1_280,
            height: 1_024,
            primary: false,
        },
    ])
}

#[test]
fn restored_dock_position_keeps_a_title_bar_visible() {
    assert_eq!(
        clamp_window_position(5_000, 5_000, 720, 520, bounds()),
        (1_872, 1_096)
    );
    assert_eq!(
        clamp_window_position(-5_000, -5_000, 720, 520, bounds()),
        (-1_952, -472)
    );
}

#[test]
fn restored_dock_preserves_negative_secondary_monitor_offsets() {
    assert_eq!(
        clamp_window_position(-1_920, 84, 720, 520, bounds()),
        (-1_920, 84)
    );
}

#[test]
fn oversized_dock_anchors_to_the_desktop_edge() {
    assert_eq!(
        clamp_window_position(500, 500, 10_000, 10_000, bounds()),
        (-1_280, 0)
    );
}

#[test]
fn a_dock_cannot_be_collapsed_out_of_reach() {
    let weights = [1.0, 1.0, 1.85, 1.0, 1.4];

    let resized = resize(&weights, &ORDER, 2, 10_000);

    assert!(resized[2] >= MINIMUM_WEIGHT - 1e-5);
    assert!(resized[0] <= MAXIMUM_WEIGHT + 1e-5);
}

#[test]
fn a_drag_at_the_row_edge_changes_nothing() {
    let weights = [1.0, 1.0, 1.85, 1.0, 1.4];

    // There is no splitter before the first dock.
    assert_eq!(resize(&weights, &ORDER, 0, 200), weights);
}
