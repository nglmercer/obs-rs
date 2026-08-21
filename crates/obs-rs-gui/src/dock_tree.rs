//! Bounded, toolkit-neutral docking layout state.
//!
//! The Slint window currently consumes legacy parallel models while the dock
//! interaction packets are migrated. This module is the single tree model
//! those adapters serialize and validate, so future split/tab operations do
//! not grow another layout representation in the UI.

use std::fmt::Write as _;

pub(crate) type DockId = i32;

pub(crate) const DOCK_IDS: [DockId; 5] = [0, 1, 2, 3, 4];
pub(crate) const MAX_DOCK_LAYOUT_BYTES: usize = 4_096;
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
        let mut encoded = String::new();
        encode_inner(self, &mut encoded);
        (encoded.len() <= MAX_DOCK_LAYOUT_BYTES).then_some(encoded)
    }

    /// Decodes a settings representation and rejects malformed or oversized
    /// layouts before they can allocate an unbounded tree.
    pub(crate) fn decode(encoded: &str) -> Option<Self> {
        if encoded.is_empty() || encoded.len() > MAX_DOCK_LAYOUT_BYTES {
            return None;
        }
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
mod tests {
    use super::*;

    const ORDER: [DockId; 5] = [1, 0, 2, 3, 4];
    const WEIGHTS: [f32; 5] = [1.0, 1.0, 1.85, 1.0, 1.4];

    #[test]
    fn legacy_layout_becomes_a_valid_horizontal_tree() {
        let tree = DockNode::from_legacy(&ORDER, &WEIGHTS).expect("tree");
        assert!(tree.is_valid());
        assert_eq!(tree.leaf_order(), ORDER);
        assert!(tree.encode().expect("encoding").starts_with("S(H,"));
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
                    docks: vec![3, 4],
                    active: 0,
                }),
            }),
        };
        let encoded = tree.encode().expect("encoding");
        assert_eq!(DockNode::decode(&encoded), Some(tree));
    }

    #[test]
    fn invalid_or_oversized_layouts_are_rejected_before_use() {
        assert!(DockNode::decode("D9").is_none());
        assert!(DockNode::decode("S(H,1,D0,D1)").is_none());
        assert!(DockNode::decode("T(2;0,1)").is_none());
        assert!(DockNode::decode(&"D0".repeat(MAX_DOCK_LAYOUT_BYTES)).is_none());
    }
}
