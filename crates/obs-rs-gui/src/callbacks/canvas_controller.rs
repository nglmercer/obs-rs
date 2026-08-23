use super::{
    crop_transform, drag_rect, item_rect, local_item_rect, preserve_resize_aspect,
    rotate_canvas_delta, selection_overlay_for_transforms, set_selection_overlay, snap_rect,
    snap_rotated_resize_delta, transform_for_rect, transform_for_rotated_local_rect,
    transform_with_geometry, visible_source_extent, CanvasResizeModifiers, CanvasState, CanvasZoom,
    ComponentHandle, DesktopState, ItemRect, MainWindow, PreviewSurface, Rc, RefCell,
    SceneItemSpec, SelectionOverlay, SnapGuides, SnapSettings, TransformDraft, TransformDraftItem,
    UiCommand, MAX_SNAP_GUIDES, MINIMUM_ITEM_PIXELS,
};

pub(crate) struct CanvasController {
    pub(super) draft: RefCell<Option<TransformDraft>>,
    pub(super) state: RefCell<CanvasState>,
}

impl CanvasController {
    /// Returns the drag in progress, if the pointer is down on an item.
    pub(crate) fn draft(&self) -> Option<TransformDraft> {
        self.draft.borrow().clone()
    }

    /// Returns the transient viewport state used by the canvas presentation.
    pub(crate) fn canvas_state(&self) -> CanvasState {
        *self.state.borrow()
    }

    /// Applies one pointer delta and returns the new viewport state.
    pub(crate) fn pan_by(&self, dx: i32, dy: i32) -> CanvasState {
        let state = self.canvas_state().panned(dx, dy);
        self.state.replace(state);
        state
    }

    /// Applies the persisted canvas snap distance to the one transient canvas
    /// policy shared by move and resize gestures.
    pub(crate) fn set_snap_distance(&self, distance: u16) -> CanvasState {
        let state = self.canvas_state().with_snap_distance(distance);
        self.state.replace(state);
        state
    }

    fn begin_selection(&self, x: i64, y: i64, additive: bool) -> CanvasState {
        let state = self.canvas_state().begin_selection(x, y, additive);
        self.state.replace(state);
        state
    }

    fn update_selection(&self, x: i64, y: i64) -> CanvasState {
        let state = self.canvas_state().update_selection(x, y);
        self.state.replace(state);
        state
    }

    fn finish_selection(&self) -> (CanvasState, Option<ItemRect>, bool) {
        let state = self.canvas_state();
        let result = (state, state.selection_box(), state.selection_additive());
        self.state.replace(state.clear_selection());
        result
    }
}

pub(super) fn set_selection_box_properties(ui: &MainWindow, selection: Option<ItemRect>) {
    if let Some(selection) = selection {
        ui.set_selection_box_active(true);
        ui.set_selection_box_x(i32::try_from(selection.x).unwrap_or(0));
        ui.set_selection_box_y(i32::try_from(selection.y).unwrap_or(0));
        ui.set_selection_box_width(i32::try_from(selection.width).unwrap_or(0));
        ui.set_selection_box_height(i32::try_from(selection.height).unwrap_or(0));
    } else {
        ui.set_selection_box_active(false);
        ui.set_selection_box_x(0);
        ui.set_selection_box_y(0);
        ui.set_selection_box_width(0);
        ui.set_selection_box_height(0);
    }
}

/// Connects the UI's transient zoom controls to the one canvas state owner.
pub(super) fn install_zoom_callbacks(ui: &MainWindow, controller: &Rc<CanvasController>) {
    let state = controller.canvas_state();
    ui.set_canvas_zoom(state.zoom().ui_value());
    ui.set_canvas_pan_x(state.pan().0);
    ui.set_canvas_pan_y(state.pan().1);

    let weak = ui.as_weak();
    let zoom_controller = Rc::clone(controller);
    ui.on_canvas_zoom_changed(move |value| {
        let Some(zoom) = CanvasZoom::from_ui_value(value) else {
            return;
        };
        zoom_controller
            .state
            .replace(zoom_controller.canvas_state().with_zoom(zoom));
        if let Some(ui) = weak.upgrade() {
            ui.set_canvas_zoom(zoom.ui_value());
        }
    });

    let weak = ui.as_weak();
    let zoom_controller = Rc::clone(controller);
    ui.on_canvas_zoom_step(move |direction| {
        let zoom = zoom_controller.canvas_state().zoom().stepped(direction);
        zoom_controller
            .state
            .replace(zoom_controller.canvas_state().with_zoom(zoom));
        if let Some(ui) = weak.upgrade() {
            ui.set_canvas_zoom(zoom.ui_value());
        }
    });

    let weak = ui.as_weak();
    let zoom_controller = Rc::clone(controller);
    ui.on_canvas_zoom_at(
        move |direction, anchor_x, anchor_y, pointer_x, pointer_y, view_x, view_y, scale| {
            let state = zoom_controller.canvas_state().zoomed_at(
                direction,
                (anchor_x, anchor_y),
                (pointer_x, pointer_y),
                (view_x, view_y),
                scale,
            );
            zoom_controller.state.replace(state);
            if let Some(ui) = weak.upgrade() {
                ui.set_canvas_zoom(state.zoom().ui_value());
                ui.set_canvas_pan_x(state.pan().0);
                ui.set_canvas_pan_y(state.pan().1);
            }
        },
    );

    let weak = ui.as_weak();
    let pan_controller = Rc::clone(controller);
    ui.on_canvas_pan_dragged(move |dx, dy| {
        let state = pan_controller.pan_by(dx, dy);
        if let Some(ui) = weak.upgrade() {
            ui.set_canvas_pan_x(state.pan().0);
            ui.set_canvas_pan_y(state.pan().1);
        }
    });
}

/// Installs the canvas selection and transform callbacks.
#[allow(
    clippy::too_many_lines,
    reason = "the callback installation keeps draft, commit, and selection lifetimes together"
)]
pub(crate) fn install_canvas_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Rc<CanvasController> {
    let controller = Rc::new(CanvasController {
        draft: RefCell::new(None),
        state: RefCell::new(CanvasState::default()),
    });
    install_zoom_callbacks(ui, &controller);

    let weak = ui.as_weak();
    let nudge_state = Rc::clone(state);
    let nudge_surface = Rc::clone(surface);
    ui.on_canvas_nudged(move |dx, dy| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(scene) = nudge_state.borrow().preview_scene().map(str::to_owned) else {
            return;
        };
        let (profile, transforms) = {
            let state = nudge_state.borrow();
            let profile = state
                .project_session()
                .project()
                .active_profile()
                .to_string();
            let Some(scene_spec) = state
                .project_session()
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene.as_str()))
            else {
                return;
            };
            let transforms = state
                .selected_sources()
                .filter_map(|id| scene_spec.item(id))
                .filter(|item| !item.locked())
                .map(|item| {
                    let transform = item.transform();
                    (
                        item.id().to_string(),
                        transform_with_geometry(
                            transform,
                            transform.scale_x_milli(),
                            transform.scale_y_milli(),
                            i64::from(transform.translate_x()).saturating_add(i64::from(dx)),
                            i64::from(transform.translate_y()).saturating_add(i64::from(dy)),
                        ),
                    )
                })
                .collect::<Vec<_>>();
            (profile, transforms)
        };
        if transforms.is_empty() {
            return;
        }
        crate::apply_source_transforms_to(
            &ui,
            &nudge_state,
            &nudge_surface,
            &profile,
            &scene,
            transforms,
        );
    });

    let weak = ui.as_weak();
    let pointer_state = Rc::clone(state);
    let pointer_surface = Rc::clone(surface);
    let pointer_controller = Rc::clone(&controller);
    ui.on_canvas_pointer_pressed(move |x, y, additive| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        pointer_controller.draft.borrow_mut().take();
        let canvas = canvas_size(&ui);
        // A plain click follows OBS's preview behavior: if the current
        // topmost item is already selected, walk down through the hit stack
        // so an obscured source can be reached without first moving it.
        // Ctrl-click remains an explicit toggle of the topmost hit.
        let hit = source_at(
            &pointer_state,
            canvas,
            i64::from(x),
            i64::from(y),
            !additive,
        );
        if let Some(id) = hit {
            let command = if additive {
                UiCommand::ToggleSourceSelection { id }
            } else {
                UiCommand::SelectSource { id }
            };
            crate::dispatch_and_refresh(&ui.as_weak(), &pointer_state, &pointer_surface, command);
            ui.set_canvas_pointer_mode(1);
            set_selection_box_properties(&ui, None);
        } else {
            if !additive {
                crate::dispatch_and_refresh(
                    &ui.as_weak(),
                    &pointer_state,
                    &pointer_surface,
                    UiCommand::SelectSources {
                        ids: Vec::new(),
                        additive: false,
                    },
                );
            }
            let state = pointer_controller.begin_selection(i64::from(x), i64::from(y), additive);
            ui.set_canvas_pointer_mode(2);
            set_selection_box_properties(&ui, state.selection_box());
        }
    });

    let weak = ui.as_weak();
    let selection_controller = Rc::clone(&controller);
    ui.on_canvas_selection_dragged(move |x, y| {
        let state = selection_controller.update_selection(i64::from(x), i64::from(y));
        if let Some(ui) = weak.upgrade() {
            set_selection_box_properties(&ui, state.selection_box());
        }
    });

    let weak = ui.as_weak();
    let selection_state = Rc::clone(state);
    let selection_surface = Rc::clone(surface);
    let selection_controller = Rc::clone(&controller);
    ui.on_canvas_selection_committed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let (_, selection, additive) = selection_controller.finish_selection();
        ui.set_canvas_pointer_mode(0);
        set_selection_box_properties(&ui, None);
        let ids = selection
            .map(|selection| source_ids_in_rect(&selection_state, canvas_size(&ui), selection))
            .unwrap_or_default();
        crate::dispatch_and_refresh(
            &ui.as_weak(),
            &selection_state,
            &selection_surface,
            UiCommand::SelectSources { ids, additive },
        );
    });

    let weak = ui.as_weak();
    let drag_state = Rc::clone(state);
    let drag_controller = Rc::clone(&controller);
    ui.on_transform_dragged(move |handle, dx, dy, modifier_mask| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if selected_is_locked(&drag_state) {
            return;
        }
        let Some(scene) = drag_state.borrow().preview_scene().map(str::to_owned) else {
            return;
        };
        let canvas = canvas_size(&ui);
        let mut draft_slot = drag_controller.draft.borrow_mut();
        let needs_new_draft = draft_slot.as_ref().is_none_or(|draft| draft.scene != scene);
        if needs_new_draft {
            let items = selected_transforms(&drag_state, &scene);
            if items.is_empty() {
                return;
            }
            *draft_slot = Some(TransformDraft { scene, items });
        }
        let Some(draft) = draft_slot.as_mut() else {
            return;
        };
        let Some(group) = draft_rect(draft, canvas) else {
            return;
        };
        let modifiers = CanvasResizeModifiers::from_mask(modifier_mask);
        if (9..=16).contains(&handle) {
            if draft.items.len() != 1 {
                return;
            }
            let item = &mut draft.items[0];
            item.transform = crop_transform(
                item.transform,
                handle - 8,
                i64::from(dx),
                i64::from(dy),
                canvas,
            );
        } else if draft.items.len() == 1
            && draft.items[0].transform.is_rotated()
            && (1..=8).contains(&handle)
        {
            let snap_settings = SnapSettings {
                enabled: drag_controller.canvas_state().snapping().enabled && modifiers.snapping,
                ..drag_controller.canvas_state().snapping()
            };
            let guides = scene_snap_guides(&drag_state, &draft.scene, &draft.items, canvas);
            let base = draft.items[0].transform;
            let (adjusted_x, adjusted_y) = snap_rotated_resize_delta(
                base,
                handle,
                i64::from(dx),
                i64::from(dy),
                canvas,
                &guides,
                snap_settings,
            );
            let (axis_x, axis_y) =
                rotate_canvas_delta(adjusted_x, adjusted_y, base.rotation_milli_degrees(), true);
            let local_base = local_item_rect(base, canvas);
            let raw = drag_rect(local_base, handle, axis_x, axis_y);
            let (aspect_width, aspect_height) = visible_source_extent(base, canvas);
            let local = if modifiers.preserve_aspect {
                preserve_resize_aspect(local_base, raw, handle, aspect_width, aspect_height)
            } else {
                raw
            };
            draft.items[0].transform =
                transform_for_rotated_local_rect(base, local, handle, canvas);
        } else {
            let snap_settings = SnapSettings {
                enabled: drag_controller.canvas_state().snapping().enabled && modifiers.snapping,
                ..drag_controller.canvas_state().snapping()
            };
            let guides = scene_snap_guides(&drag_state, &draft.scene, &draft.items, canvas);
            let snapped = snap_rect(
                drag_rect(group, handle, i64::from(dx), i64::from(dy)),
                handle,
                &guides,
                snap_settings,
            );
            let (aspect_width, aspect_height) = if draft.items.len() == 1 {
                visible_source_extent(draft.items[0].transform, canvas)
            } else {
                (group.width, group.height)
            };
            let rect = if modifiers.preserve_aspect {
                preserve_resize_aspect(group, snapped, handle, aspect_width, aspect_height)
            } else {
                snapped
            };
            for item in &mut draft.items {
                let old_rect = item_rect(item.transform, canvas);
                let next_rect = if handle == 0 {
                    ItemRect {
                        x: old_rect.x.saturating_add(rect.x.saturating_sub(group.x)),
                        y: old_rect.y.saturating_add(rect.y.saturating_sub(group.y)),
                        ..old_rect
                    }
                } else {
                    map_rect_into_group(old_rect, group, rect)
                };
                item.transform = transform_for_rect(item.transform, next_rect, canvas);
            }
        }
        let overlay = draft_overlay(draft, canvas);
        drop(draft_slot);
        // The drag stays out of the project until the pointer is released: the
        // preview timer feeds this straight to the compositor, and the overlay
        // handles follow it here. A revision per mouse move would fill the undo
        // history with a hundred entries for one gesture — and, before the
        // runtime learned to update in place, restart every capture device in
        // the scene along the way.
        if let Some(overlay) = overlay {
            set_selection_overlay(&ui, Some(&overlay));
        }
    });

    let weak = ui.as_weak();
    let commit_state = Rc::clone(state);
    let commit_surface = Rc::clone(surface);
    let commit_controller = Rc::clone(&controller);
    ui.on_transform_committed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(draft) = commit_controller.draft.borrow_mut().take() else {
            return;
        };
        let profile = commit_state
            .borrow()
            .project_session()
            .project()
            .active_profile()
            .to_string();
        let transforms = draft
            .items
            .into_iter()
            .map(|item| (item.item, item.transform))
            .collect::<Vec<_>>();
        ui.set_canvas_pointer_mode(0);
        if transforms.is_empty() {
            return;
        }
        crate::apply_source_transforms_to(
            &ui,
            &commit_state,
            &commit_surface,
            &profile,
            &draft.scene,
            transforms,
        );
    });

    controller
}

/// Moves the selection overlay with the pointer during a drag.
///
/// The dock refresh derives these from the project, which the drag has not
/// reached yet, so the handles are placed from the draft instead.
/// Copies the selected scene-item transforms once, at the start of a gesture.
/// Subsequent pointer samples mutate this bounded draft in place rather than
/// rebuilding the selection from project state.
pub(super) fn selected_transforms(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
) -> Vec<TransformDraftItem> {
    let state = state.borrow();
    let Some(scene) = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
    else {
        return Vec::new();
    };
    state
        .selected_sources()
        .filter_map(|id| {
            scene.item(id).map(|item| TransformDraftItem {
                item: id.to_owned(),
                transform: item.transform(),
            })
        })
        .take(obs_rs_ui::MAX_CANVAS_SELECTIONS)
        .collect()
}

pub(super) fn draft_rect(draft: &TransformDraft, canvas: (u32, u32)) -> Option<ItemRect> {
    draft
        .items
        .iter()
        .map(|item| item_rect(item.transform, canvas))
        .reduce(ItemRect::union)
}

pub(super) fn draft_overlay(
    draft: &TransformDraft,
    canvas: (u32, u32),
) -> Option<SelectionOverlay> {
    let transforms = draft
        .items
        .iter()
        .map(|item| item.transform)
        .collect::<Vec<_>>();
    selection_overlay_for_transforms(&transforms, canvas)
}

/// Maps one item rectangle from the old group bounds into the new bounds.
pub(super) fn map_rect_into_group(
    rect: ItemRect,
    old_group: ItemRect,
    new_group: ItemRect,
) -> ItemRect {
    let map = |value: i64, old_start: i64, old_extent: i64, new_start: i64, new_extent: i64| {
        let offset = i128::from(value.saturating_sub(old_start));
        let scaled = offset * i128::from(new_extent) / i128::from(old_extent.max(1));
        i64::try_from(i128::from(new_start) + scaled).unwrap_or_else(|_| {
            if scaled.is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        })
    };
    let x = map(
        rect.x,
        old_group.x,
        old_group.width,
        new_group.x,
        new_group.width,
    );
    let y = map(
        rect.y,
        old_group.y,
        old_group.height,
        new_group.y,
        new_group.height,
    );
    let right = map(
        rect.x.saturating_add(rect.width),
        old_group.x,
        old_group.width,
        new_group.x,
        new_group.width,
    );
    let bottom = map(
        rect.y.saturating_add(rect.height),
        old_group.y,
        old_group.height,
        new_group.y,
        new_group.height,
    );
    ItemRect {
        x,
        y,
        width: right.saturating_sub(x).max(MINIMUM_ITEM_PIXELS),
        height: bottom.saturating_sub(y).max(MINIMUM_ITEM_PIXELS),
    }
}

pub(super) fn source_ids_in_rect(
    state: &Rc<RefCell<DesktopState>>,
    canvas: (u32, u32),
    selection: ItemRect,
) -> Vec<String> {
    let state = state.borrow();
    let Some(scene_id) = state.preview_scene() else {
        return Vec::new();
    };
    let Some(scene) = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
    else {
        return Vec::new();
    };
    scene
        .items()
        .iter()
        .filter(|item| item.visible() && item_rect(item.transform(), canvas).intersects(selection))
        .map(|item| item.id().as_str().to_owned())
        .take(obs_rs_ui::MAX_CANVAS_SELECTIONS)
        .collect()
}

/// Returns the first hit that a plain preview click may select.
///
/// OBS walks down through already-selected hits when a normal click lands on
/// a stack of overlapping sources. Ctrl-click calls the same hit test with
/// `select_below = false`, preserving the topmost-toggle behavior.
pub(super) fn first_selectable_hit<'a, I>(hits: I, select_below: bool) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a str, bool)>,
{
    hits.into_iter().find_map(|(id, selected)| {
        if select_below && selected {
            None
        } else {
            Some(id)
        }
    })
}

/// Returns the canvas size the studio is currently rendering at.
pub(super) fn canvas_size(ui: &MainWindow) -> (u32, u32) {
    (
        u32::try_from(ui.get_canvas_width()).unwrap_or(1_920).max(1),
        u32::try_from(ui.get_canvas_height())
            .unwrap_or(1_080)
            .max(1),
    )
}

/// Returns whether the selected source is locked against editing.
pub(super) fn selected_is_locked(state: &Rc<RefCell<DesktopState>>) -> bool {
    let state = state.borrow();
    let Some(scene) = state.preview_scene() else {
        return false;
    };
    let session = state.project_session();
    session
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene))
        .is_some_and(|scene| {
            scene
                .items()
                .iter()
                .filter(|item| state.is_source_selected(item.id().as_str()))
                .any(SceneItemSpec::locked)
        })
}

/// Returns the topmost visible source covering a canvas point.
pub(super) fn source_at(
    state: &Rc<RefCell<DesktopState>>,
    canvas: (u32, u32),
    x: i64,
    y: i64,
    select_below: bool,
) -> Option<String> {
    let state = state.borrow();
    let scene = state.preview_scene()?;
    let session = state.project_session();
    let scene = session.project().active_profile_spec()?.scene(scene)?;
    first_selectable_hit(
        scene.items().iter().rev().filter_map(|item| {
            if !item.visible() || !item_rect(item.transform(), canvas).contains(x, y) {
                return None;
            }
            Some((
                item.id().as_str(),
                state.is_source_selected(item.id().as_str()),
            ))
        }),
        select_below,
    )
    .map(str::to_owned)
}

/// Builds the bounded canvas and source guides for one transform gesture.
pub(super) fn scene_snap_guides(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
    excluded: &[TransformDraftItem],
    canvas: (u32, u32),
) -> SnapGuides {
    let mut guides = SnapGuides::with_canvas(canvas);
    let state = state.borrow();
    let Some(scene) = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
    else {
        return guides;
    };
    for item in scene.items() {
        if excluded
            .iter()
            .any(|selected| selected.item == item.id().as_str())
            || !item.visible()
        {
            continue;
        }
        guides.push_rect(item_rect(item.transform(), canvas));
        if guides.x_len == MAX_SNAP_GUIDES && guides.y_len == MAX_SNAP_GUIDES {
            break;
        }
    }
    guides
}
