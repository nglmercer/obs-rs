use obs_rs_config::Config;

use super::{
    flag, selection_component, selection_component_decode, DockNode, DOCK_IDS,
    MAX_PERSISTED_PROJECTOR_MONITORS, MAX_PERSISTED_PROJECTOR_TARGETS,
    MAX_PROJECTOR_MONITOR_ID_BYTES, MAX_PROJECTOR_TARGET_COMPONENT_BYTES, PROJECTOR_MONITORS_KEY,
    PROJECTOR_TARGETS_KEY,
};

/// Window layout state, restored so a session reopens where it was left.
///
/// This is the desktop's own state rather than project data, which is why it
/// belongs to the settings document instead of the project file. Width shares
/// are floats, so the type compares by value rather than deriving `Eq`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutSettings {
    /// Legacy projection of the tree's dock IDs: 0 scenes, 1 sources, 2 mixer,
    /// 3 transitions, 4 controls. New layout code must mutate `dock_tree` and
    /// refresh this projection at the toolkit boundary.
    pub(crate) panel_order: Vec<i32>,
    pub(crate) show_scenes: bool,
    pub(crate) show_sources: bool,
    pub(crate) show_mixer: bool,
    pub(crate) show_transitions: bool,
    pub(crate) show_controls: bool,
    /// 0 is studio mode, 1 the single-canvas default, and 2 is multiview.
    pub(crate) view_mode: i32,
    /// Height of the dock row in logical pixels.
    pub(crate) dock_height: u32,
    /// Legacy width shares per dock kind, as adjusted by the row splitter
    /// adapter. Tree-native layouts retain this for old settings readers.
    pub(crate) panel_weights: Vec<f32>,
    /// Dock kinds that were left detached in their own windows.
    pub(crate) floating_panels: Vec<i32>,
    /// Last known physical desktop geometry for detached docks. The scale is
    /// retained so a window can keep its logical size when it is restored on
    /// a display with a different DPI.
    pub(crate) floating_geometry: Vec<FloatingGeometry>,
    /// Last known physical desktop geometry for projector feeds. Fullscreen
    /// projectors retain bounded dimensions for a later return to windowed
    /// mode; display state is stored in the same record.
    pub(crate) projector_geometry: Vec<ProjectorGeometry>,
    /// Bounded target identities for source and scene projector reopening.
    pub(crate) projector_targets: Vec<ProjectorTarget>,
    /// Bounded monitor identities observed for projector windows.
    pub(crate) projector_monitors: Vec<ProjectorMonitor>,
    /// Versioned tree representation of the dock arrangement.
    pub(crate) dock_tree: DockNode,
}

/// Bounded geometry for one detached dock window.
///
/// Positions and dimensions use the windowing backend's physical pixel space,
/// which is the only space that is stable across multi-monitor desktops. A
/// saved scale factor lets restore adjust the dimensions without silently
/// turning a 320 logical-pixel dock into a tiny window after a DPI change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FloatingGeometry {
    pub(crate) panel: i32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_milli: u32,
}

impl FloatingGeometry {
    pub(crate) const MIN_POSITION: i32 = -2_000_000;
    pub(crate) const MAX_POSITION: i32 = 2_000_000;
    pub(crate) const MIN_WIDTH: u32 = 240;
    pub(crate) const MAX_WIDTH: u32 = 8_192;
    pub(crate) const MIN_HEIGHT: u32 = 160;
    pub(crate) const MAX_HEIGHT: u32 = 8_192;
    pub(crate) const MIN_SCALE_MILLI: u32 = 500;
    pub(crate) const MAX_SCALE_MILLI: u32 = 4_000;

    /// Creates a geometry record only when every value is safe to restore.
    pub(crate) fn new(
        panel: i32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale_milli: u32,
    ) -> Option<Self> {
        if !DEFAULT_PANEL_ORDER.contains(&panel)
            || !(Self::MIN_POSITION..=Self::MAX_POSITION).contains(&x)
            || !(Self::MIN_POSITION..=Self::MAX_POSITION).contains(&y)
            || !(Self::MIN_WIDTH..=Self::MAX_WIDTH).contains(&width)
            || !(Self::MIN_HEIGHT..=Self::MAX_HEIGHT).contains(&height)
            || !(Self::MIN_SCALE_MILLI..=Self::MAX_SCALE_MILLI).contains(&scale_milli)
        {
            return None;
        }
        Some(Self {
            panel,
            x,
            y,
            width,
            height,
            scale_milli,
        })
    }
}

/// The stable IDs used by the projector settings record.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ProjectorKind {
    Program,
    Preview,
    Multiview,
    Source,
    Scene,
}

/// Stable target identity for a source or scene projector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProjectorTarget {
    Source { scene: String, item: String },
    Scene { scene: String },
}

/// Stable platform monitor identity observed for one projector feed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectorMonitor {
    pub(crate) projector: ProjectorKind,
    pub(crate) monitor: String,
}

impl ProjectorMonitor {
    pub(crate) fn new(projector: ProjectorKind, monitor: String) -> Option<Self> {
        if monitor.is_empty() || monitor.len() > MAX_PROJECTOR_MONITOR_ID_BYTES {
            return None;
        }
        Some(Self { projector, monitor })
    }
}

impl ProjectorKind {
    pub(crate) const ALL: [Self; 5] = [
        Self::Program,
        Self::Preview,
        Self::Multiview,
        Self::Source,
        Self::Scene,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Preview => "preview",
            Self::Multiview => "multiview",
            Self::Source => "source",
            Self::Scene => "scene",
        }
    }

    fn from_id(value: &str) -> Option<Self> {
        match value {
            "program" => Some(Self::Program),
            "preview" => Some(Self::Preview),
            "multiview" => Some(Self::Multiview),
            "source" => Some(Self::Source),
            "scene" => Some(Self::Scene),
            _ => None,
        }
    }
}

/// Bounded geometry and display state for one projector feed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProjectorGeometry {
    pub(crate) projector: ProjectorKind,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_milli: u32,
    pub(crate) fullscreen: bool,
    pub(crate) open: bool,
}

impl ProjectorGeometry {
    /// Creates a geometry record only when every value is safe to restore.
    pub(crate) fn new(
        projector: ProjectorKind,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale_milli: u32,
    ) -> Option<Self> {
        if !(FloatingGeometry::MIN_POSITION..=FloatingGeometry::MAX_POSITION).contains(&x)
            || !(FloatingGeometry::MIN_POSITION..=FloatingGeometry::MAX_POSITION).contains(&y)
            || !(FloatingGeometry::MIN_WIDTH..=FloatingGeometry::MAX_WIDTH).contains(&width)
            || !(FloatingGeometry::MIN_HEIGHT..=FloatingGeometry::MAX_HEIGHT).contains(&height)
            || !(FloatingGeometry::MIN_SCALE_MILLI..=FloatingGeometry::MAX_SCALE_MILLI)
                .contains(&scale_milli)
        {
            return None;
        }
        Some(Self {
            projector,
            x,
            y,
            width,
            height,
            scale_milli,
            fullscreen: false,
            open: false,
        })
    }

    pub(crate) const fn with_fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    pub(crate) const fn with_open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
}

/// Scales a bounded physical dimension when a window is restored on another
/// DPI, keeping the result inside the same safe window-size range.
pub(crate) fn scale_window_dimension(value: u32, ratio: f32, minimum: u32, maximum: u32) -> u32 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "dimensions are bounded by the geometry record before scaling"
    )]
    let scaled = (value as f32 * ratio).round() as u32;
    scaled.clamp(minimum, maximum)
}

/// The dock IDs a layout must contain, in the order OBS ships them.
pub(super) const DEFAULT_PANEL_ORDER: [i32; 5] = [1, 0, 2, 3, 4];

/// Relative dock widths OBS ships with: the mixer is the widest strip and the
/// controls column the narrowest.
pub(super) const DEFAULT_PANEL_WEIGHTS: [f32; 5] = [1.0, 1.0, 1.85, 1.0, 1.4];

/// Bounds a stored width share must lie inside to be used.
const WEIGHT_RANGE: std::ops::RangeInclusive<f32> = 0.2..=8.0;

impl Default for LayoutSettings {
    fn default() -> Self {
        let panel_order = DEFAULT_PANEL_ORDER.to_vec();
        let panel_weights = DEFAULT_PANEL_WEIGHTS.to_vec();
        let dock_tree = DockNode::from_legacy(&panel_order, &panel_weights)
            .expect("the built-in dock layout must be valid");
        Self {
            panel_order,
            show_scenes: true,
            show_sources: true,
            show_mixer: true,
            show_transitions: true,
            show_controls: true,
            view_mode: 1,
            dock_height: 248,
            panel_weights,
            floating_panels: Vec::new(),
            floating_geometry: Vec::new(),
            projector_geometry: Vec::new(),
            projector_targets: Vec::new(),
            projector_monitors: Vec::new(),
            dock_tree,
        }
    }
}

impl LayoutSettings {
    /// Parses `1,0,2,3,4` into a complete dock order.
    ///
    /// A document that names a dock twice, omits one, or contains an unknown ID
    /// is rejected wholesale: a partial layout would hide docks with no way for
    /// the user to tell why.
    pub(super) fn parse_panel_order(value: &str) -> Option<Vec<i32>> {
        let order = value
            .split(',')
            .map(|entry| entry.trim().parse::<i32>().ok())
            .collect::<Option<Vec<_>>>()?;
        let mut sorted = order.clone();
        sorted.sort_unstable();
        (sorted == [0, 1, 2, 3, 4]).then_some(order)
    }

    /// Parses `1.0,1.0,1.85,1.0,1.4` into one share per dock.
    ///
    /// A document with the wrong count, or a share outside the range a splitter
    /// can produce, falls back wholesale rather than leaving a dock unusable.
    pub(super) fn parse_panel_weights(value: &str) -> Option<Vec<f32>> {
        let weights = value
            .split(',')
            .map(|entry| entry.trim().parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        (weights.len() == DEFAULT_PANEL_WEIGHTS.len()
            && weights
                .iter()
                .all(|weight| weight.is_finite() && WEIGHT_RANGE.contains(weight)))
        .then_some(weights)
    }

    /// Parses the comma-separated list of detached dock IDs.
    pub(super) fn parse_floating(value: &str) -> Vec<i32> {
        let mut panels = value
            .split(',')
            .filter_map(|entry| entry.trim().parse::<i32>().ok())
            .filter(|panel| DEFAULT_PANEL_ORDER.contains(panel))
            .collect::<Vec<_>>();
        panels.sort_unstable();
        panels.dedup();
        panels
    }

    pub(super) fn panel_weights_text(&self) -> String {
        self.panel_weights
            .iter()
            .map(|weight| format!("{weight:.3}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub(super) fn floating_text(&self) -> String {
        self.floating_panels
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Parses the versioned `v1:panel:x:y:width:height:scale;...` geometry
    /// list. Individual bad records are discarded so one unplugged monitor
    /// cannot destroy the positions of every other detached dock.
    pub(super) fn parse_floating_geometry(value: &str) -> Vec<FloatingGeometry> {
        let Some(records) = value.strip_prefix("v1:") else {
            return Vec::new();
        };
        let mut geometry: Vec<FloatingGeometry> = Vec::new();
        for record in records.split(';').filter(|record| !record.is_empty()) {
            let fields = record.split(':').collect::<Vec<_>>();
            if fields.len() != 6 {
                continue;
            }
            let [panel, x, y, width, height, scale] = [
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
            ];
            let (Some(panel), Some(x), Some(y), Some(width), Some(height), Some(scale)) = (
                panel.parse().ok(),
                x.parse().ok(),
                y.parse().ok(),
                width.parse().ok(),
                height.parse().ok(),
                scale.parse().ok(),
            ) else {
                continue;
            };
            let Some(entry) = FloatingGeometry::new(panel, x, y, width, height, scale) else {
                continue;
            };
            if geometry.iter().all(|other| other.panel != entry.panel)
                && geometry.len() < DEFAULT_PANEL_ORDER.len()
            {
                geometry.push(entry);
            }
        }
        geometry.sort_unstable_by_key(|entry| entry.panel);
        geometry
    }

    /// Parses the versioned projector geometry list. Version one records did
    /// not carry display state; version two adds fullscreen; version three
    /// adds one bounded open-state bit without invalidating old settings.
    pub(super) fn parse_projector_geometry(value: &str) -> Vec<ProjectorGeometry> {
        let (version, records) = if let Some(records) = value.strip_prefix("v3:") {
            (3_u8, records)
        } else if let Some(records) = value.strip_prefix("v2:") {
            (2_u8, records)
        } else if let Some(records) = value.strip_prefix("v1:") {
            (1_u8, records)
        } else {
            return Vec::new();
        };
        let mut geometry: Vec<ProjectorGeometry> = Vec::new();
        for record in records.split(';').filter(|record| !record.is_empty()) {
            let fields = record.split(':').collect::<Vec<_>>();
            if (version == 1 && fields.len() != 6)
                || (version == 2 && fields.len() != 7)
                || (version == 3 && fields.len() != 8)
            {
                continue;
            }
            let [projector, x, y, width, height, scale] = [
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
            ];
            let (Some(projector), Some(x), Some(y), Some(width), Some(height), Some(scale)) = (
                ProjectorKind::from_id(projector),
                x.parse().ok(),
                y.parse().ok(),
                width.parse().ok(),
                height.parse().ok(),
                scale.parse().ok(),
            ) else {
                continue;
            };
            let fullscreen = match version {
                1 => false,
                2 | 3 => match fields[6] {
                    "0" => false,
                    "1" => true,
                    _ => continue,
                },
                _ => continue,
            };
            let open = match version {
                3 => match fields[7] {
                    "0" => false,
                    "1" => true,
                    _ => continue,
                },
                1 | 2 => false,
                _ => continue,
            };
            let Some(entry) = ProjectorGeometry::new(projector, x, y, width, height, scale)
                .map(|entry| entry.with_fullscreen(fullscreen).with_open(open))
            else {
                continue;
            };
            if geometry
                .iter()
                .all(|other| other.projector != entry.projector)
                && geometry.len() < ProjectorKind::ALL.len()
            {
                geometry.push(entry);
            }
        }
        geometry.sort_unstable_by_key(|entry| entry.projector);
        geometry
    }

    pub(super) fn floating_geometry_text(&self) -> String {
        let mut geometry = self.floating_geometry.clone();
        geometry.sort_unstable_by_key(|entry| entry.panel);
        let records = geometry
            .into_iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}:{}:{}",
                    entry.panel, entry.x, entry.y, entry.width, entry.height, entry.scale_milli
                )
            })
            .collect::<Vec<_>>();
        format!("v1:{}", records.join(";"))
    }

    pub(super) fn projector_geometry_text(&self) -> String {
        let mut geometry = self.projector_geometry.clone();
        geometry.sort_unstable_by_key(|entry| entry.projector);
        let records = geometry
            .into_iter()
            .map(|entry| {
                format!(
                    "{}:{}:{}:{}:{}:{}:{}:{}",
                    entry.projector.id(),
                    entry.x,
                    entry.y,
                    entry.width,
                    entry.height,
                    entry.scale_milli,
                    u8::from(entry.fullscreen),
                    u8::from(entry.open),
                )
            })
            .collect::<Vec<_>>();
        format!("v3:{}", records.join(";"))
    }

    pub(super) fn parse_projector_targets(value: &str) -> Vec<ProjectorTarget> {
        let Some(records) = value.strip_prefix("v1") else {
            return Vec::new();
        };
        let records = records.strip_prefix(';').unwrap_or_default();
        let mut targets = Vec::with_capacity(MAX_PERSISTED_PROJECTOR_TARGETS);
        for record in records.split(';').filter(|record| !record.is_empty()) {
            if targets.len() == MAX_PERSISTED_PROJECTOR_TARGETS {
                break;
            }
            let mut fields = record.split('|');
            let Some(kind) = fields.next() else {
                continue;
            };
            let target = match kind {
                "source" => {
                    let (Some(scene), Some(item)) = (
                        fields.next().and_then(selection_component_decode),
                        fields.next().and_then(selection_component_decode),
                    ) else {
                        continue;
                    };
                    if fields.next().is_some()
                        || scene.is_empty()
                        || item.is_empty()
                        || scene.len() > MAX_PROJECTOR_TARGET_COMPONENT_BYTES
                        || item.len() > MAX_PROJECTOR_TARGET_COMPONENT_BYTES
                    {
                        continue;
                    }
                    ProjectorTarget::Source { scene, item }
                }
                "scene" => {
                    let Some(scene) = fields.next().and_then(selection_component_decode) else {
                        continue;
                    };
                    if fields.next().is_some()
                        || scene.is_empty()
                        || scene.len() > MAX_PROJECTOR_TARGET_COMPONENT_BYTES
                    {
                        continue;
                    }
                    ProjectorTarget::Scene { scene }
                }
                _ => continue,
            };
            let duplicate = targets.iter().any(|existing| {
                matches!(
                    (existing, &target),
                    (
                        ProjectorTarget::Source { .. },
                        ProjectorTarget::Source { .. }
                    ) | (ProjectorTarget::Scene { .. }, ProjectorTarget::Scene { .. })
                )
            });
            if !duplicate {
                targets.push(target);
            }
        }
        targets
    }

    pub(super) fn projector_targets_text(&self) -> String {
        let mut targets = self.projector_targets.clone();
        targets.sort_by_key(|target| matches!(target, ProjectorTarget::Scene { .. }));
        let mut encoded = String::from("v1");
        for target in targets.into_iter().take(MAX_PERSISTED_PROJECTOR_TARGETS) {
            let record = match target {
                ProjectorTarget::Source { scene, item } => format!(
                    "source|{}|{}",
                    selection_component(&scene),
                    selection_component(&item)
                ),
                ProjectorTarget::Scene { scene } => {
                    format!("scene|{}", selection_component(&scene))
                }
            };
            let required = 1_usize.saturating_add(record.len());
            if encoded.len().saturating_add(required) > obs_rs_config::MAX_VALUE_BYTES {
                break;
            }
            encoded.push(';');
            encoded.push_str(&record);
        }
        encoded
    }

    /// Parses `v1;program|DP-1;preview|HDMI-1` into bounded monitor records.
    ///
    /// Monitor IDs come from a platform capability and may contain separators,
    /// so they use the same escaped component format as other session-scoped
    /// identities. Duplicate feed records are ignored after the first valid
    /// record, keeping restoration deterministic.
    pub(super) fn parse_projector_monitors(value: &str) -> Vec<ProjectorMonitor> {
        let Some(records) = value.strip_prefix("v1") else {
            return Vec::new();
        };
        let records = records.strip_prefix(';').unwrap_or_default();
        let mut monitors: Vec<ProjectorMonitor> =
            Vec::with_capacity(MAX_PERSISTED_PROJECTOR_MONITORS);
        for record in records.split(';').filter(|record| !record.is_empty()) {
            if monitors.len() == MAX_PERSISTED_PROJECTOR_MONITORS {
                break;
            }
            let mut fields = record.split('|');
            let (Some(projector), Some(monitor)) = (
                fields.next().and_then(ProjectorKind::from_id),
                fields.next().and_then(selection_component_decode),
            ) else {
                continue;
            };
            if fields.next().is_some() {
                continue;
            }
            let Some(monitor) = ProjectorMonitor::new(projector, monitor) else {
                continue;
            };
            if monitors
                .iter()
                .all(|existing| existing.projector != monitor.projector)
            {
                monitors.push(monitor);
            }
        }
        monitors.sort_unstable_by_key(|entry| entry.projector);
        monitors
    }

    pub(super) fn projector_monitors_text(&self) -> String {
        let mut monitors = self.projector_monitors.clone();
        monitors.sort_unstable_by_key(|entry| entry.projector);
        let mut encoded = String::from("v1");
        for monitor in monitors.into_iter().take(MAX_PERSISTED_PROJECTOR_MONITORS) {
            let record = format!(
                "{}|{}",
                monitor.projector.id(),
                selection_component(&monitor.monitor)
            );
            let required = 1_usize.saturating_add(record.len());
            if encoded.len().saturating_add(required) > obs_rs_config::MAX_VALUE_BYTES {
                break;
            }
            encoded.push(';');
            encoded.push_str(&record);
        }
        encoded
    }

    pub(super) fn panel_order_text(&self) -> String {
        self.dock_tree
            .leaf_order()
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl LayoutSettings {
    /// Reads the layout keys, falling back per key so one unreadable value
    /// cannot discard the rest of the stored layout.
    /// Reads every key the settings window owns.
    ///
    /// The document is a flat list of independent keys and this is its
    /// per-key fallback table, so splitting it would only scatter one
    /// mapping across several functions.
    #[allow(clippy::too_many_lines, reason = "one fallback arm per stored key")]
    pub(super) fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        let legacy_order = config
            .get("layout_panel_order")
            .and_then(LayoutSettings::parse_panel_order)
            .unwrap_or_else(|| defaults.panel_order.clone());
        let legacy_weights = config
            .get("layout_panel_weights")
            .and_then(Self::parse_panel_weights)
            .unwrap_or_else(|| defaults.panel_weights.clone());
        let dock_tree = config
            .get("layout_dock_tree")
            .and_then(DockNode::decode)
            .filter(|tree| tree.leaf_order().len() == DOCK_IDS.len())
            .or_else(|| DockNode::from_legacy(&legacy_order, &legacy_weights))
            .unwrap_or_else(|| defaults.dock_tree.clone());
        Self {
            panel_order: dock_tree.leaf_order(),
            show_scenes: flag(config, "layout_show_scenes", defaults.show_scenes),
            show_sources: flag(config, "layout_show_sources", defaults.show_sources),
            show_mixer: flag(config, "layout_show_mixer", defaults.show_mixer),
            show_transitions: flag(config, "layout_show_transitions", defaults.show_transitions),
            show_controls: flag(config, "layout_show_controls", defaults.show_controls),
            view_mode: config
                .get("layout_view_mode")
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|mode| (0..=2).contains(mode))
                .unwrap_or(defaults.view_mode),
            dock_height: config
                .get("layout_dock_height")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|height| (120..=1_200).contains(height))
                .unwrap_or(defaults.dock_height),
            panel_weights: legacy_weights,
            floating_panels: config
                .get("layout_floating_panels")
                .map(Self::parse_floating)
                .unwrap_or(defaults.floating_panels),
            floating_geometry: config
                .get("layout_floating_geometry")
                .map(Self::parse_floating_geometry)
                .unwrap_or(defaults.floating_geometry),
            projector_geometry: config
                .get("layout_projector_geometry")
                .map(Self::parse_projector_geometry)
                .unwrap_or(defaults.projector_geometry),
            projector_targets: config
                .get(PROJECTOR_TARGETS_KEY)
                .map(Self::parse_projector_targets)
                .unwrap_or(defaults.projector_targets),
            projector_monitors: config
                .get(PROJECTOR_MONITORS_KEY)
                .map(Self::parse_projector_monitors)
                .unwrap_or(defaults.projector_monitors),
            dock_tree,
        }
    }
}
