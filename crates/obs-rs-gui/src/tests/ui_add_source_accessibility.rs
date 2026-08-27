use super::*;
use i_slint_backend_testing::AccessibleRole;

/// Verifies the visible source-kind list and its existing category-selection
/// callback through the testing backend's accessibility action.
pub(super) fn exercise_add_source_kind_accessibility(window: &crate::AddSourceWindow) {
    let recent = ElementHandle::find_by_accessible_label(window, "@recent")
        .find(|row| {
            row.accessible_role() == Some(AccessibleRole::ListItem)
                && row.size().width > 100.0
                && row.size().height > 20.0
        })
        .expect("the Recently added source kind is accessible");
    assert_eq!(
        recent.accessible_description().as_deref(),
        Some("Recently added")
    );
    assert_eq!(recent.accessible_enabled(), Some(true));
    assert_eq!(recent.accessible_item_selectable(), Some(true));
    assert_eq!(recent.accessible_item_selected(), Some(true));
    assert_eq!(recent.accessible_item_index(), Some(0));
    assert_eq!(
        recent.accessible_item_count(),
        Some(window.get_kind_rows().row_count())
    );
    recent.invoke_accessible_default_action();
    assert_eq!(window.get_selected_kind(), "@recent");
    assert_eq!(recent.accessible_item_selected(), Some(true));
}

/// Verifies one visible candidate card and exercises its existing toggle path
/// through the testing backend's accessibility action.
pub(super) fn exercise_existing_candidate_accessibility(
    window: &crate::AddSourceWindow,
    candidate_id: &str,
    candidate_name: &str,
    candidate_scene: &str,
) {
    let candidate_card = ElementHandle::find_by_accessible_label(window, candidate_id)
        .find(|card| {
            card.accessible_role() == Some(AccessibleRole::ListItem)
                && card.size().width > 100.0
                && card.size().height > 100.0
        })
        .expect("visible existing source candidate card is accessible");
    let description = candidate_card
        .accessible_description()
        .expect("candidate card has a human-readable description");
    assert!(description.contains(candidate_name));
    assert!(description.contains(candidate_scene));
    assert_eq!(candidate_card.accessible_enabled(), Some(true));
    assert_eq!(candidate_card.accessible_item_selectable(), Some(true));
    assert_eq!(candidate_card.accessible_item_selected(), Some(false));
    let candidate_count = window.get_candidates().row_count();
    assert_eq!(
        candidate_card.accessible_item_count(),
        Some(candidate_count)
    );
    assert!(candidate_card
        .accessible_item_index()
        .is_some_and(|index| index < candidate_count));
    candidate_card.invoke_accessible_default_action();
    assert_eq!(window.get_selected_count(), 1);
    assert_eq!(candidate_card.accessible_item_selected(), Some(true));
}
