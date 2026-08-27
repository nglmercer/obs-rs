use super::*;

pub(super) fn visible_source_row_target(ui: &MainWindow, index: usize) -> ElementHandle {
    let rows = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>();
    rows.get(index)
        .expect("visible source row")
        .query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|area| area.size().width > 150.0 && area.size().height > 30.0)
        .find_first()
        .expect("visible source row target")
}

pub(super) fn focus_canvas(ui: &MainWindow) {
    let canvas = ElementHandle::find_by_element_id(ui, "CanvasEditor::surface")
        .find(|canvas| canvas.size().width > 100.0 && canvas.size().height > 100.0)
        .expect("editable canvas focus target");
    canvas.mock_single_click(PointerEventButton::Left);
}

pub(super) fn focus_last_source_row(ui: &MainWindow) {
    let row = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>()
        .pop()
        .expect("keyboard source row");
    let target = row
        .query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|area| area.size().width > 150.0 && area.size().height > 30.0)
        .find_first()
        .expect("keyboard source row focus target");
    target.mock_single_click(PointerEventButton::Left);
}
