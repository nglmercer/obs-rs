//! Bounded, toolkit-neutral docking layout state.
//!
//! The Slint window consumes a bounded pane projection from this tree. Legacy
//! order/weight arrays remain only as a compatibility boundary, so split/tab
//! operations do not grow another layout representation in the UI.

use std::{cmp::Ordering, fmt::Write as _};

pub(crate) type DockId = i32;

pub(crate) const DOCK_IDS: [DockId; 5] = [0, 1, 2, 3, 4];
pub(crate) const MAX_DOCK_LAYOUT_BYTES: usize = 4_096;
const DOCK_LAYOUT_VERSION: &str = "v1:";
const MAX_DOCK_NODES: usize = 31;
const MAX_DOCK_DEPTH: usize = 8;
const MIN_RATIO_MILLI: u16 = 50;
const MAX_RATIO_MILLI: u16 = 950;

/// Direction of a split between two dock regions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockAxis {
    Horizontal,
    Vertical,
}

/// The five OBS-style drop targets shown while a dock is being dragged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockDropZone {
    Tab,
    Left,
    Right,
    Top,
    Bottom,
}

impl DockDropZone {
    pub(crate) const fn ui_value(self) -> i32 {
        match self {
            Self::Tab => 0,
            Self::Left => 1,
            Self::Right => 2,
            Self::Top => 3,
            Self::Bottom => 4,
        }
    }

    pub(crate) const fn split(self) -> Option<(DockAxis, u16, bool)> {
        match self {
            Self::Tab => None,
            // A split's first child is the existing target by default. The
            // boolean makes the left/top zones place the dragged dock first.
            Self::Left => Some((DockAxis::Horizontal, 350, true)),
            Self::Right => Some((DockAxis::Horizontal, 650, false)),
            Self::Top => Some((DockAxis::Vertical, 350, true)),
            Self::Bottom => Some((DockAxis::Vertical, 650, false)),
        }
    }
}

/// A bounded dock tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DockNode {
    /// Two regions separated along one axis.
    Split {
        axis: DockAxis,
        ratio_milli: u16,
        first: Box<Self>,
        second: Box<Self>,
    },
    /// Several docks sharing one region, with one visible tab.
    Tabs { docks: Vec<DockId>, active: usize },
    /// One leaf dock: 0 scenes, 1 sources, 2 mixer, 3 transitions, 4 controls.
    Dock(DockId),
}

/// A leaf's normalized rectangle and tab metadata for the toolkit adapter.
///
/// The Rust geometry uses a 0..=1000 fixed-point desktop, then the Slint
/// adapter converts it to normalized 0..=1 fractions. This keeps layout
/// updates deterministic and avoids making the Slint layer responsible for
/// tree traversal or ratio arithmetic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DockPaneLayout {
    pub(crate) panel: DockId,
    pub(crate) x_milli: u16,
    pub(crate) y_milli: u16,
    pub(crate) width_milli: u16,
    pub(crate) height_milli: u16,
    pub(crate) tab_group: u8,
    pub(crate) tab_index: u8,
    pub(crate) tab_count: u8,
    pub(crate) tab_ids: [DockId; DOCK_IDS.len()],
    pub(crate) active: bool,
}

/// A bounded splitter projection. `boundary_milli` is the split edge along
/// the splitter's axis in the same 0..=1000 workspace as dock panes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DockSplitterLayout {
    pub(crate) id: u8,
    pub(crate) axis: DockAxis,
    pub(crate) boundary_milli: u16,
}

impl DockNode {
    /// Builds a deterministic tree from the legacy order and per-dock shares.
    ///
    /// This is the compatibility bridge for settings documents written before
    /// tree layouts existed. The input is strictly bounded and validated
    /// before any tree is returned.
    pub(crate) fn from_legacy(order: &[DockId], weights_by_id: &[f32]) -> Option<Self> {
        if order.len() != DOCK_IDS.len() || weights_by_id.len() != DOCK_IDS.len() {
            return None;
        }
        let mut sorted = order.to_vec();
        sorted.sort_unstable();
        if sorted != DOCK_IDS {
            return None;
        }
        if weights_by_id
            .iter()
            .any(|weight| !weight.is_finite() || *weight <= 0.0)
        {
            return None;
        }
        let weights = order
            .iter()
            .map(|id| {
                usize::try_from(*id)
                    .ok()
                    .and_then(|index| weights_by_id.get(index).copied())
            })
            .collect::<Option<Vec<_>>>()?;
        build_balanced(order, &weights)
    }

    /// Returns the dock IDs in deterministic display order.
    pub(crate) fn leaf_order(&self) -> Vec<DockId> {
        let mut order = Vec::with_capacity(DOCK_IDS.len());
        self.collect_order(&mut order);
        order
    }

    /// Computes the bounded pane projection consumed by the UI renderer.
    pub(crate) fn pane_layout(&self) -> Vec<DockPaneLayout> {
        let mut panes = Vec::with_capacity(DOCK_IDS.len());
        let mut next_group = 0;
        self.collect_panes(
            LayoutRect {
                x: 0,
                y: 0,
                width: 1_000,
                height: 1_000,
            },
            &mut next_group,
            &mut panes,
        );
        panes
    }

    /// Computes the visible splitter edges without exposing the tree to the
    /// toolkit. There can be at most four splitters for the five built-in
    /// docks, so the projection remains bounded by construction.
    pub(crate) fn splitter_layout(&self) -> Vec<DockSplitterLayout> {
        let mut splitters = Vec::new();
        let mut next_id = 0;
        self.collect_splitters(
            LayoutRect {
                x: 0,
                y: 0,
                width: 1_000,
                height: 1_000,
            },
            &mut next_id,
            &mut splitters,
        );
        splitters
    }

    /// Moves one splitter by a fixed-point delta, clamped to the same safe
    /// ratio bounds used by settings and drag/drop insertion.
    pub(crate) fn resize_splitter(&mut self, id: u8, delta_milli: i32) -> bool {
        let mut next_id = 0;
        resize_splitter_inner(self, id, delta_milli, &mut next_id)
    }

    /// Activates a dock within its tab group, if it belongs to one.
    pub(crate) fn activate_tab(&mut self, panel: DockId) -> bool {
        match self {
            Self::Split { first, second, .. } => {
                first.activate_tab(panel) || second.activate_tab(panel)
            }
            Self::Tabs { docks, active } => {
                let Some(index) = docks.iter().position(|dock| *dock == panel) else {
                    return false;
                };
                *active = index;
                true
            }
            Self::Dock(_) => false,
        }
    }

    /// Reports whether the current tree is the legacy horizontal row adapter.
    pub(crate) fn is_flat_horizontal(&self) -> bool {
        match self {
            Self::Split {
                axis: DockAxis::Horizontal,
                first,
                second,
                ..
            } => first.is_flat_horizontal() && second.is_flat_horizontal(),
            Self::Dock(_) => true,
            Self::Split {
                axis: DockAxis::Vertical,
                ..
            }
            | Self::Tabs { .. } => false,
        }
    }

    /// Moves `panel` into the tab group containing `target`.
    pub(crate) fn tab_dock_with(&mut self, panel: DockId, target: DockId) -> bool {
        self.move_dock(panel, |tree, moved| insert_tab(tree, target, moved))
    }

    /// Splits the region containing `target` and places `panel` in the new
    /// region. The operation is atomic from the caller's perspective: an
    /// unsuccessful target or invalid result leaves the original tree intact.
    pub(crate) fn split_dock_with(
        &mut self,
        panel: DockId,
        target: DockId,
        axis: DockAxis,
        ratio_milli: u16,
    ) -> bool {
        if !(MIN_RATIO_MILLI..=MAX_RATIO_MILLI).contains(&ratio_milli) || panel == target {
            return false;
        }
        self.move_dock(panel, |tree, moved| {
            insert_split(tree, target, axis, ratio_milli, moved)
        })
    }

    /// Resolves a normalized pointer location to the active pane beneath it.
    /// The outer quarter of a pane is a split insertion zone; its center is a
    /// tab target. Inactive tabs are excluded so a hidden tab cannot steal a
    /// drop from the visible region.
    pub(crate) fn drop_target(&self, x_milli: u16, y_milli: u16) -> Option<(DockId, DockDropZone)> {
        let x = u32::from(x_milli.min(1_000));
        let y = u32::from(y_milli.min(1_000));
        self.pane_layout().into_iter().find_map(|pane| {
            if !pane.active
                || x < u32::from(pane.x_milli)
                || y < u32::from(pane.y_milli)
                || x >= u32::from(pane.x_milli.saturating_add(pane.width_milli))
                || y >= u32::from(pane.y_milli.saturating_add(pane.height_milli))
            {
                return None;
            }
            let local_x =
                (x - u32::from(pane.x_milli)) * 1_000 / u32::from(pane.width_milli.max(1));
            let local_y =
                (y - u32::from(pane.y_milli)) * 1_000 / u32::from(pane.height_milli.max(1));
            let zone = if local_x < 250 {
                DockDropZone::Left
            } else if local_x >= 750 {
                DockDropZone::Right
            } else if local_y < 250 {
                DockDropZone::Top
            } else if local_y >= 750 {
                DockDropZone::Bottom
            } else {
                DockDropZone::Tab
            };
            Some((pane.panel, zone))
        })
    }

    /// Applies a drag drop zone, placing the moved dock on the requested side
    /// of the target rather than always appending it after the target.
    pub(crate) fn drop_dock_with(
        &mut self,
        panel: DockId,
        target: DockId,
        zone: DockDropZone,
    ) -> bool {
        if zone == DockDropZone::Tab {
            return self.tab_dock_with(panel, target);
        }
        let Some((axis, ratio_milli, moved_first)) = zone.split() else {
            return false;
        };
        self.move_dock(panel, |tree, moved| {
            insert_split_ordered(tree, target, axis, ratio_milli, moved, moved_first)
        })
    }

    fn move_dock<F>(&mut self, panel: DockId, insert: F) -> bool
    where
        F: FnOnce(&mut Self, DockNode) -> bool,
    {
        if !DOCK_IDS.contains(&panel) || !self.is_valid() {
            return false;
        }
        let original = self.clone();
        let (Some(mut remaining), Some(moved)) = take_dock(original, panel) else {
            return false;
        };
        if insert(&mut remaining, moved) && remaining.is_valid() {
            *self = remaining;
            true
        } else {
            false
        }
    }

    /// Validates IDs, tab state, ratios, depth, and total node count.
    pub(crate) fn is_valid(&self) -> bool {
        let mut seen = [false; DOCK_IDS.len()];
        let mut leaves = 0;
        let mut nodes = 0;
        self.validate_inner(0, &mut seen, &mut leaves, &mut nodes)
            && leaves == DOCK_IDS.len()
            && nodes <= MAX_DOCK_NODES
            && seen.into_iter().all(|present| present)
    }

    /// Encodes a validated tree into the bounded settings representation.
    pub(crate) fn encode(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        let mut encoded = String::from(DOCK_LAYOUT_VERSION);
        encode_inner(self, &mut encoded);
        (encoded.len() <= MAX_DOCK_LAYOUT_BYTES).then_some(encoded)
    }

    /// Decodes a settings representation and rejects malformed or oversized
    /// layouts before they can allocate an unbounded tree.
    pub(crate) fn decode(encoded: &str) -> Option<Self> {
        if encoded.is_empty() || encoded.len() > MAX_DOCK_LAYOUT_BYTES {
            return None;
        }
        let encoded = encoded.strip_prefix(DOCK_LAYOUT_VERSION)?;
        let mut parser = Parser::new(encoded);
        let node = parser.node(0)?;
        if !parser.at_end() || !node.is_valid() {
            return None;
        }
        Some(node)
    }

    fn collect_order(&self, order: &mut Vec<DockId>) {
        match self {
            Self::Split { first, second, .. } => {
                first.collect_order(order);
                second.collect_order(order);
            }
            Self::Tabs { docks, .. } => order.extend(docks.iter().copied()),
            Self::Dock(id) => order.push(*id),
        }
    }

    fn validate_inner(
        &self,
        depth: usize,
        seen: &mut [bool; DOCK_IDS.len()],
        leaves: &mut usize,
        nodes: &mut usize,
    ) -> bool {
        *nodes = nodes.saturating_add(1);
        if depth > MAX_DOCK_DEPTH || *nodes > MAX_DOCK_NODES {
            return false;
        }
        match self {
            Self::Split {
                ratio_milli,
                first,
                second,
                ..
            } => {
                (MIN_RATIO_MILLI..=MAX_RATIO_MILLI).contains(ratio_milli)
                    && first.validate_inner(depth + 1, seen, leaves, nodes)
                    && second.validate_inner(depth + 1, seen, leaves, nodes)
            }
            Self::Tabs { docks, active } => {
                !docks.is_empty()
                    && docks.len() <= DOCK_IDS.len()
                    && *active < docks.len()
                    && docks.iter().all(|id| mark_dock(*id, seen, leaves))
            }
            Self::Dock(id) => mark_dock(*id, seen, leaves),
        }
    }

    fn collect_panes(
        &self,
        rect: LayoutRect,
        next_group: &mut u8,
        panes: &mut Vec<DockPaneLayout>,
    ) {
        match self {
            Self::Split {
                axis,
                ratio_milli,
                first,
                second,
            } => {
                let (first_rect, second_rect) = match axis {
                    DockAxis::Horizontal => rect.split_horizontal(*ratio_milli),
                    DockAxis::Vertical => rect.split_vertical(*ratio_milli),
                };
                first.collect_panes(first_rect, next_group, panes);
                second.collect_panes(second_rect, next_group, panes);
            }
            Self::Tabs { docks, active } => {
                let group = *next_group;
                *next_group = next_group.saturating_add(1);
                let mut tab_ids = [0; DOCK_IDS.len()];
                for (index, dock) in docks.iter().enumerate() {
                    tab_ids[index] = *dock;
                }
                for (index, dock) in docks.iter().enumerate() {
                    panes.push(DockPaneLayout {
                        panel: *dock,
                        x_milli: fixed(rect.x),
                        y_milli: fixed(rect.y),
                        width_milli: fixed(rect.width),
                        height_milli: fixed(rect.height),
                        tab_group: group,
                        tab_index: u8::try_from(index).unwrap_or(u8::MAX),
                        tab_count: u8::try_from(docks.len()).unwrap_or(u8::MAX),
                        tab_ids,
                        active: index == *active,
                    });
                }
            }
            Self::Dock(dock_id) => panes.push(DockPaneLayout {
                panel: *dock_id,
                x_milli: fixed(rect.x),
                y_milli: fixed(rect.y),
                width_milli: fixed(rect.width),
                height_milli: fixed(rect.height),
                tab_group: u8::MAX,
                tab_index: 0,
                tab_count: 1,
                tab_ids: [*dock_id, 0, 0, 0, 0],
                active: true,
            }),
        }
    }

    fn collect_splitters(
        &self,
        rect: LayoutRect,
        next_id: &mut u8,
        splitters: &mut Vec<DockSplitterLayout>,
    ) {
        let Self::Split {
            axis,
            ratio_milli,
            first,
            second,
        } = self
        else {
            return;
        };
        let id = *next_id;
        *next_id = next_id.saturating_add(1);
        let boundary_milli = match axis {
            DockAxis::Horizontal => fixed(rect.x + rect.width * u32::from(*ratio_milli) / 1_000),
            DockAxis::Vertical => fixed(rect.y + rect.height * u32::from(*ratio_milli) / 1_000),
        };
        splitters.push(DockSplitterLayout {
            id,
            axis: *axis,
            boundary_milli,
        });
        let (first_rect, second_rect) = match axis {
            DockAxis::Horizontal => rect.split_horizontal(*ratio_milli),
            DockAxis::Vertical => rect.split_vertical(*ratio_milli),
        };
        first.collect_splitters(first_rect, next_id, splitters);
        second.collect_splitters(second_rect, next_id, splitters);
    }
}

fn resize_splitter_inner(
    node: &mut DockNode,
    target_id: u8,
    delta_milli: i32,
    next_id: &mut u8,
) -> bool {
    let DockNode::Split {
        ratio_milli,
        first,
        second,
        ..
    } = node
    else {
        return false;
    };
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    if id == target_id {
        let current = i32::from(*ratio_milli);
        let next = current
            .saturating_add(delta_milli)
            .clamp(i32::from(MIN_RATIO_MILLI), i32::from(MAX_RATIO_MILLI));
        let Ok(next) = u16::try_from(next) else {
            return false;
        };
        if next == *ratio_milli {
            return false;
        }
        *ratio_milli = next;
        return true;
    }
    resize_splitter_inner(first, target_id, delta_milli, next_id)
        || resize_splitter_inner(second, target_id, delta_milli, next_id)
}

fn fixed(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[derive(Clone, Copy)]
struct LayoutRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl LayoutRect {
    fn split_horizontal(self, ratio_milli: u16) -> (Self, Self) {
        let first_width = self.width * u32::from(ratio_milli) / 1_000;
        (
            Self {
                width: first_width,
                ..self
            },
            Self {
                x: self.x + first_width,
                width: self.width - first_width,
                ..self
            },
        )
    }

    fn split_vertical(self, ratio_milli: u16) -> (Self, Self) {
        let first_height = self.height * u32::from(ratio_milli) / 1_000;
        (
            Self {
                height: first_height,
                ..self
            },
            Self {
                y: self.y + first_height,
                height: self.height - first_height,
                ..self
            },
        )
    }
}

fn mark_dock(id: DockId, seen: &mut [bool; DOCK_IDS.len()], leaves: &mut usize) -> bool {
    let Some(index) = DOCK_IDS.iter().position(|known| *known == id) else {
        return false;
    };
    if seen[index] {
        return false;
    }
    seen[index] = true;
    *leaves = leaves.saturating_add(1);
    true
}

fn build_balanced(order: &[DockId], weights: &[f32]) -> Option<DockNode> {
    if order.len() == 1 {
        return Some(DockNode::Dock(order[0]));
    }
    let split = order.len() / 2;
    let left_weight: f32 = weights[..split].iter().sum();
    let total_weight: f32 = weights.iter().sum();
    if !left_weight.is_finite() || !total_weight.is_finite() || total_weight <= 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "legacy weights are bounded UI shares and are quantized for persistence"
    )]
    let ratio_milli = (left_weight / total_weight * 1_000.0)
        .round()
        .clamp(f32::from(MIN_RATIO_MILLI), f32::from(MAX_RATIO_MILLI)) as u16;
    Some(DockNode::Split {
        axis: DockAxis::Horizontal,
        ratio_milli,
        first: Box::new(build_balanced(&order[..split], &weights[..split])?),
        second: Box::new(build_balanced(&order[split..], &weights[split..])?),
    })
}

fn take_dock(node: DockNode, panel: DockId) -> (Option<DockNode>, Option<DockNode>) {
    match node {
        DockNode::Dock(id) if id == panel => (None, Some(DockNode::Dock(id))),
        DockNode::Dock(_) => (Some(node), None),
        DockNode::Tabs { mut docks, active } => {
            let Some(index) = docks.iter().position(|dock| *dock == panel) else {
                return (Some(DockNode::Tabs { docks, active }), None);
            };
            docks.remove(index);
            let moved = Some(DockNode::Dock(panel));
            match docks.len() {
                0 => (None, moved),
                1 => (Some(DockNode::Dock(docks[0])), moved),
                _ => {
                    let active = match active.cmp(&index) {
                        Ordering::Equal => active.min(docks.len() - 1),
                        Ordering::Greater => active - 1,
                        Ordering::Less => active,
                    };
                    (Some(DockNode::Tabs { docks, active }), moved)
                }
            }
        }
        DockNode::Split {
            axis,
            ratio_milli,
            first,
            second,
        } => {
            let (first, moved) = take_dock(*first, panel);
            if moved.is_some() {
                return (
                    Some(match first {
                        Some(first) => DockNode::Split {
                            axis,
                            ratio_milli,
                            first: Box::new(first),
                            second,
                        },
                        None => *second,
                    }),
                    moved,
                );
            }
            let (second, moved) = take_dock(*second, panel);
            if moved.is_some() {
                let remaining = match (first, second) {
                    (Some(first), Some(second)) => DockNode::Split {
                        axis,
                        ratio_milli,
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                    (Some(first), None) | (None, Some(first)) => first,
                    (None, None) => return (None, moved),
                };
                return (Some(remaining), moved);
            }
            let remaining = match (first, second) {
                (Some(first), Some(second)) => DockNode::Split {
                    axis,
                    ratio_milli,
                    first: Box::new(first),
                    second: Box::new(second),
                },
                (Some(first), None) | (None, Some(first)) => first,
                (None, None) => return (None, None),
            };
            (Some(remaining), None)
        }
    }
}

fn insert_tab(node: &mut DockNode, target: DockId, moved: DockNode) -> bool {
    let moved_id = moved_dock_id(&moved);
    match node {
        DockNode::Dock(id) if *id == target => {
            *node = DockNode::Tabs {
                docks: vec![target, moved_id],
                active: 1,
            };
            true
        }
        DockNode::Dock(_) => false,
        DockNode::Tabs { docks, active } => {
            if !docks.contains(&target) {
                return false;
            }
            docks.push(moved_id);
            *active = docks.len() - 1;
            true
        }
        DockNode::Split { first, second, .. } => {
            insert_tab(first, target, moved.clone()) || insert_tab(second, target, moved)
        }
    }
}

fn insert_split(
    node: &mut DockNode,
    target: DockId,
    axis: DockAxis,
    ratio_milli: u16,
    moved: DockNode,
) -> bool {
    insert_split_ordered(node, target, axis, ratio_milli, moved, false)
}

fn insert_split_ordered(
    node: &mut DockNode,
    target: DockId,
    axis: DockAxis,
    ratio_milli: u16,
    moved: DockNode,
    moved_first: bool,
) -> bool {
    match node {
        DockNode::Dock(id) if *id == target => {
            let current = std::mem::replace(node, DockNode::Dock(target));
            let (first, second) = if moved_first {
                (moved, current)
            } else {
                (current, moved)
            };
            *node = DockNode::Split {
                axis,
                ratio_milli,
                first: Box::new(first),
                second: Box::new(second),
            };
            true
        }
        DockNode::Tabs { docks, .. } if docks.contains(&target) => {
            let current = std::mem::replace(node, DockNode::Dock(target));
            let (first, second) = if moved_first {
                (moved, current)
            } else {
                (current, moved)
            };
            *node = DockNode::Split {
                axis,
                ratio_milli,
                first: Box::new(first),
                second: Box::new(second),
            };
            true
        }
        DockNode::Dock(_) | DockNode::Tabs { .. } => false,
        DockNode::Split { first, second, .. } => {
            insert_split_ordered(first, target, axis, ratio_milli, moved.clone(), moved_first)
                || insert_split_ordered(second, target, axis, ratio_milli, moved, moved_first)
        }
    }
}

fn moved_dock_id(node: &DockNode) -> DockId {
    match node {
        DockNode::Dock(id) => *id,
        _ => unreachable!("dock moves always detach one leaf"),
    }
}

fn encode_inner(node: &DockNode, output: &mut String) {
    match node {
        DockNode::Split {
            axis,
            ratio_milli,
            first,
            second,
        } => {
            let axis = match axis {
                DockAxis::Horizontal => 'H',
                DockAxis::Vertical => 'V',
            };
            let _ = write!(output, "S({axis},{ratio_milli},");
            encode_inner(first, output);
            output.push(',');
            encode_inner(second, output);
            output.push(')');
        }
        DockNode::Tabs { docks, active } => {
            let _ = write!(output, "T({active};");
            for (index, dock) in docks.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "{dock}");
            }
            output.push(')');
        }
        DockNode::Dock(id) => {
            let _ = write!(output, "D{id}");
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            position: 0,
        }
    }

    fn at_end(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn node(&mut self, depth: usize) -> Option<DockNode> {
        if depth > MAX_DOCK_DEPTH {
            return None;
        }
        match self.take()? {
            b'D' => Some(DockNode::Dock(i32::try_from(self.number()?).ok()?)),
            b'S' => self.split(depth),
            b'T' => self.tabs(),
            _ => None,
        }
    }

    fn split(&mut self, depth: usize) -> Option<DockNode> {
        self.expect(b'(')?;
        let axis = match self.take()? {
            b'H' => DockAxis::Horizontal,
            b'V' => DockAxis::Vertical,
            _ => return None,
        };
        self.expect(b',')?;
        let ratio_milli = u16::try_from(self.number()?).ok()?;
        self.expect(b',')?;
        let first = self.node(depth + 1)?;
        self.expect(b',')?;
        let second = self.node(depth + 1)?;
        self.expect(b')')?;
        Some(DockNode::Split {
            axis,
            ratio_milli,
            first: Box::new(first),
            second: Box::new(second),
        })
    }

    fn tabs(&mut self) -> Option<DockNode> {
        self.expect(b'(')?;
        let active = self.number()?;
        self.expect(b';')?;
        let mut docks = Vec::with_capacity(DOCK_IDS.len());
        loop {
            docks.push(i32::try_from(self.number()?).ok()?);
            match self.take()? {
                b',' => {}
                b')' => break,
                _ => return None,
            }
        }
        Some(DockNode::Tabs { docks, active })
    }

    fn number(&mut self) -> Option<usize> {
        let start = self.position;
        while self.position < self.bytes.len()
            && self.bytes[self.position].is_ascii_digit()
            && self.position.saturating_sub(start) < 10
        {
            self.position += 1;
        }
        (self.position > start).then(|| {
            std::str::from_utf8(&self.bytes[start..self.position])
                .ok()?
                .parse()
                .ok()
        })?
    }

    fn take(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.position)?;
        self.position += 1;
        Some(byte)
    }

    fn expect(&mut self, expected: u8) -> Option<()> {
        (self.take()? == expected).then_some(())
    }
}

#[cfg(test)]
#[path = "dock_tree_tests.rs"]
mod tests;
