use super::{scene_drop_target_index, source_navigation_index, source_selection_range};

#[test]
fn scene_drop_indices_account_for_removing_the_source() {
    assert_eq!(scene_drop_target_index(2, 0, 1, 5), Some(0));
    assert_eq!(scene_drop_target_index(2, 1, 2, 5), Some(2));
    assert_eq!(scene_drop_target_index(0, 2, 1, 5), Some(1));
    assert_eq!(scene_drop_target_index(0, 2, 2, 5), Some(2));
    assert_eq!(scene_drop_target_index(1, 1, 1, 5), Some(1));
    assert_eq!(scene_drop_target_index(1, 1, 2, 5), Some(1));
}

#[test]
fn scene_drop_indices_reject_invalid_modes_and_rows() {
    assert_eq!(scene_drop_target_index(0, 1, 0, 3), None);
    assert_eq!(scene_drop_target_index(0, 1, 3, 3), None);
    assert_eq!(scene_drop_target_index(3, 1, 1, 3), None);
    assert_eq!(scene_drop_target_index(0, 3, 1, 3), None);
    assert_eq!(scene_drop_target_index(0, 0, 2, 0), None);
}

#[test]
fn source_navigation_is_bounded_and_non_wrapping() {
    assert_eq!(source_navigation_index(None, 3, 1), Some(0));
    assert_eq!(source_navigation_index(None, 3, -1), Some(2));
    assert_eq!(source_navigation_index(Some(1), 3, -1), Some(0));
    assert_eq!(source_navigation_index(Some(1), 3, 1), Some(2));
    assert_eq!(source_navigation_index(Some(0), 3, -1), None);
    assert_eq!(source_navigation_index(Some(2), 3, 1), None);
    assert_eq!(source_navigation_index(Some(1), 3, -2), Some(0));
    assert_eq!(source_navigation_index(Some(1), 3, 2), Some(2));
    assert_eq!(source_navigation_index(Some(9), 3, 1), Some(0));
    assert_eq!(source_navigation_index(None, 0, 1), None);
    assert_eq!(source_navigation_index(Some(1), 3, 99), None);
}

#[test]
fn source_selection_range_is_contiguous_and_bounded() {
    let targets = vec![
        "first".to_owned(),
        "second".to_owned(),
        "third".to_owned(),
        "fourth".to_owned(),
    ];
    assert_eq!(
        source_selection_range(Some("second"), "fourth", &targets),
        Some(
            vec!["second", "third", "fourth"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        )
    );
    assert_eq!(
        source_selection_range(Some("fourth"), "second", &targets),
        Some(
            vec!["second", "third", "fourth"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        )
    );
    assert_eq!(
        source_selection_range(None, "third", &targets),
        Some(vec!["third".to_owned()])
    );
    assert_eq!(
        source_selection_range(Some("missing"), "third", &targets),
        Some(vec!["third".to_owned()])
    );
    assert_eq!(
        source_selection_range(Some("first"), "missing", &targets),
        None
    );
}
