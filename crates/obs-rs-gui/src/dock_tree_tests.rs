use super::*;

const ORDER: [DockId; 6] = [1, 0, 2, 3, 4, 5];
const WEIGHTS: [f32; 6] = [1.0, 1.0, 1.85, 1.0, 1.4, 1.1];

#[test]
fn legacy_layout_becomes_a_valid_horizontal_tree() {
    let tree = DockNode::from_legacy(&ORDER, &WEIGHTS).expect("tree");
    assert!(tree.is_valid());
    assert_eq!(tree.leaf_order(), ORDER);
    assert!(tree.encode().expect("encoding").starts_with("v1:S(H,"));
}

#[test]
fn tree_encoding_round_trips_splits_tabs_and_axes() {
    let tree = DockNode::Split {
        axis: DockAxis::Vertical,
        ratio_milli: 600,
        first: Box::new(DockNode::Tabs {
            docks: vec![1, 0],
            active: 1,
        }),
        second: Box::new(DockNode::Split {
            axis: DockAxis::Horizontal,
            ratio_milli: 400,
            first: Box::new(DockNode::Dock(2)),
            second: Box::new(DockNode::Tabs {
                docks: vec![3, 4, 5],
                active: 0,
            }),
        }),
    };
    let encoded = tree.encode().expect("encoding");
    assert_eq!(DockNode::decode(&encoded), Some(tree));
}

#[test]
fn invalid_or_oversized_layouts_are_rejected_before_use() {
    assert!(DockNode::decode("S(H,500,D0,D1)").is_none());
    assert!(DockNode::decode("D9").is_none());
    assert!(DockNode::decode("S(H,1,D0,D1)").is_none());
    assert!(DockNode::decode("T(2;0,1)").is_none());
    assert!(DockNode::decode(&"D0".repeat(MAX_DOCK_LAYOUT_BYTES)).is_none());
}

#[test]
fn pane_layout_keeps_split_rectangles_and_tab_metadata_bounded() {
    let tree = DockNode::Split {
        axis: DockAxis::Vertical,
        ratio_milli: 600,
        first: Box::new(DockNode::Tabs {
            docks: vec![1, 0],
            active: 1,
        }),
        second: Box::new(DockNode::Split {
            axis: DockAxis::Horizontal,
            ratio_milli: 400,
            first: Box::new(DockNode::Dock(2)),
            second: Box::new(DockNode::Tabs {
                docks: vec![3, 4, 5],
                active: 0,
            }),
        }),
    };

    let panes = tree.pane_layout();

    assert_eq!(panes.len(), DOCK_IDS.len());
    assert_eq!((panes[0].x_milli, panes[0].y_milli), (0, 0));
    assert_eq!((panes[0].width_milli, panes[0].height_milli), (1_000, 600));
    assert!(!panes[0].active);
    assert!(panes[1].active);
    assert_eq!(panes[1].tab_ids, [1, 0, 0, 0, 0, 0]);
    assert_eq!((panes[2].x_milli, panes[2].y_milli), (0, 600));
    assert_eq!((panes[2].width_milli, panes[2].height_milli), (400, 400));
    assert_eq!((panes[3].x_milli, panes[3].y_milli), (400, 600));
    assert_eq!((panes[3].width_milli, panes[3].height_milli), (600, 400));
}

#[test]
fn drop_target_uses_only_the_active_pane_and_reports_insertion_zones() {
    let tree = DockNode::Split {
        axis: DockAxis::Horizontal,
        ratio_milli: 600,
        first: Box::new(DockNode::Tabs {
            docks: vec![0, 1],
            active: 1,
        }),
        second: Box::new(DockNode::Dock(2)),
    };

    assert_eq!(
        tree.drop_target(50, 500),
        Some((1, DockDropZone::Left)),
        "the inactive tab must not be a drop target"
    );
    assert_eq!(tree.drop_target(300, 500), Some((1, DockDropZone::Tab)));
    assert_eq!(tree.drop_target(950, 500), Some((2, DockDropZone::Right)));
}

#[test]
fn drag_drop_can_place_a_dock_before_or_after_the_target() {
    let mut tree = DockNode::from_legacy(&ORDER, &WEIGHTS).expect("tree");

    assert!(tree.drop_dock_with(4, 1, DockDropZone::Left));
    assert_eq!(tree.leaf_order(), [4, 1, 0, 2, 3, 5]);
    assert!(tree.drop_dock_with(4, 1, DockDropZone::Right));
    assert_eq!(tree.leaf_order(), [1, 4, 0, 2, 3, 5]);
    assert!(tree.is_valid());
}

#[test]
fn splitter_projection_and_resize_keep_ratios_bounded() {
    let mut tree = DockNode::from_legacy(&ORDER, &WEIGHTS).expect("tree");
    let before = tree.splitter_layout();
    assert_eq!(before.len(), 5);
    assert_eq!(before[0].axis, DockAxis::Horizontal);
    assert!(tree.resize_splitter(before[0].id, 100));
    assert_eq!(
        tree.splitter_layout()[0].boundary_milli,
        before[0].boundary_milli + 100
    );
    assert!(tree.resize_splitter(before[0].id, 10_000));
    assert_eq!(tree.splitter_layout()[0].boundary_milli, 950);
    assert!(!tree.resize_splitter(before[0].id, 10));
}

#[test]
fn tab_and_split_mutations_are_atomic_and_preserve_all_docks() {
    let mut tree = DockNode::from_legacy(&ORDER, &WEIGHTS).expect("tree");
    let original = tree.clone();

    assert!(!tree.tab_dock_with(1, 1));
    assert_eq!(tree, original);
    assert!(tree.tab_dock_with(4, 3));
    assert!(tree.is_valid());
    assert!(tree.activate_tab(3));
    assert!(tree.split_dock_with(2, 3, DockAxis::Vertical, 500));
    assert!(tree.is_valid());
    assert_eq!(tree.leaf_order(), [1, 0, 3, 4, 2, 5]);
    assert!(!tree.split_dock_with(0, 1, DockAxis::Horizontal, 10));
    assert!(tree.is_valid());
}
