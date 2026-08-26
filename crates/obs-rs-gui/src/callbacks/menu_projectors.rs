#[allow(
    clippy::wildcard_imports,
    reason = "menu submodules share the callback boundary namespace"
)]
use super::*;

/// Owns the open projector windows.
///
/// A projector renders nothing itself: it mirrors the composited image the
/// studio window already produced, so a projector can never show a different
/// frame from the one the operator is watching.
pub(crate) struct ProjectorController {
    program: RefCell<Option<ProjectorWindow>>,
    preview: RefCell<Option<ProjectorWindow>>,
    multiview: RefCell<Option<ProjectorWindow>>,
    source: RefCell<Option<ProjectorWindow>>,
    scene: RefCell<Option<ProjectorWindow>>,
    source_target: RefCell<Option<SourceProjectorTarget>>,
    scene_target: RefCell<Option<SceneProjectorTarget>>,
    geometry: RefCell<Vec<ProjectorGeometry>>,
    monitors: RefCell<Vec<ProjectorMonitor>>,
}

#[derive(Clone, Copy)]
enum ProjectorFeed {
    Program,
    Preview,
    Multiview,
    Source,
    Scene,
}

impl ProjectorFeed {
    const fn kind(self) -> ProjectorKind {
        match self {
            Self::Program => ProjectorKind::Program,
            Self::Preview => ProjectorKind::Preview,
            Self::Multiview => ProjectorKind::Multiview,
            Self::Source => ProjectorKind::Source,
            Self::Scene => ProjectorKind::Scene,
        }
    }

    const fn is_fullscreen(self) -> bool {
        matches!(self, Self::Program | Self::Multiview)
    }
}

impl ProjectorController {
    pub(super) fn new() -> Self {
        Self {
            program: RefCell::new(None),
            preview: RefCell::new(None),
            multiview: RefCell::new(None),
            source: RefCell::new(None),
            scene: RefCell::new(None),
            source_target: RefCell::new(None),
            scene_target: RefCell::new(None),
            geometry: RefCell::new(Vec::new()),
            monitors: RefCell::new(Vec::new()),
        }
    }
    /// Returns whether a program projector needs the program canvas rendered.
    ///
    /// Single-canvas editing skips the program render to save a full-size
    /// composite per frame, so an open program projector has to ask for it back.
    pub(crate) fn wants_program(&self) -> bool {
        self.slot(ProjectorFeed::Program).borrow().is_some()
    }

    /// Returns whether a preview projector needs the preview feed rendered.
    pub(crate) fn wants_preview(&self) -> bool {
        self.slot(ProjectorFeed::Preview).borrow().is_some()
    }

    /// Returns whether a multiview projector needs the bounded scene grid
    /// rendered, even when the main window is in another view mode.
    pub(crate) fn wants_multiview(&self) -> bool {
        self.slot(ProjectorFeed::Multiview).borrow().is_some()
    }

    /// Returns the selected source target while its projector is open.
    pub(crate) fn source_target(&self) -> Option<SourceProjectorTarget> {
        self.slot(ProjectorFeed::Source)
            .borrow()
            .as_ref()
            .and(self.source_target.borrow().as_ref())
            .cloned()
    }

    /// Returns the stable scene target while its projector is open.
    pub(crate) fn scene_target(&self) -> Option<SceneProjectorTarget> {
        self.slot(ProjectorFeed::Scene)
            .borrow()
            .as_ref()
            .and(self.scene_target.borrow().as_ref())
            .cloned()
    }

    /// Loads bounded window geometry captured from the previous session.
    pub(crate) fn restore_geometry(&self, geometry: &[ProjectorGeometry]) {
        let mut stored = self.geometry.borrow_mut();
        stored.clear();
        for entry in geometry.iter().copied() {
            if stored
                .iter()
                .all(|other| other.projector != entry.projector)
                && stored.len() < ProjectorKind::ALL.len()
            {
                stored.push(entry);
            }
        }
        stored.sort_unstable_by_key(|entry| entry.projector);
    }

    /// Loads bounded monitor identities captured from the previous session.
    ///
    /// The platform is asked to resolve the identity only when a projector is
    /// reopened. A missing monitor therefore falls back to the current virtual
    /// desktop instead of making startup fail.
    pub(crate) fn restore_monitors(&self, monitors: &[ProjectorMonitor]) {
        let mut stored = self.monitors.borrow_mut();
        stored.clear();
        for entry in monitors.iter().take(ProjectorKind::ALL.len()) {
            if stored
                .iter()
                .all(|other| other.projector != entry.projector)
            {
                stored.push(entry.clone());
            }
        }
        stored.sort_unstable_by_key(|entry| entry.projector);
    }

    /// Loads bounded source/scene target identities captured from the previous
    /// session. Geometry and target state remain separate so a stale target
    /// cannot make an otherwise valid window record unsafe to parse.
    pub(crate) fn restore_targets(&self, targets: &[ProjectorTarget]) {
        self.source_target.borrow_mut().take();
        self.scene_target.borrow_mut().take();
        for target in targets.iter().take(2) {
            match target {
                ProjectorTarget::Source { scene, item }
                    if self.source_target.borrow().is_none() =>
                {
                    *self.source_target.borrow_mut() = Some(SourceProjectorTarget {
                        scene: scene.clone(),
                        item: item.clone(),
                    });
                }
                ProjectorTarget::Scene { scene } if self.scene_target.borrow().is_none() => {
                    *self.scene_target.borrow_mut() = Some(SceneProjectorTarget {
                        scene: scene.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    /// Captures only targets belonging to currently open source/scene feeds.
    /// A closed feed's geometry remains useful for the next manual open, but
    /// its target must not be resurrected without an explicit user choice.
    pub(crate) fn capture_targets(&self) -> Vec<ProjectorTarget> {
        let mut targets = Vec::with_capacity(2);
        if self.source.borrow().is_some() {
            if let Some(target) = self.source_target.borrow().as_ref() {
                targets.push(ProjectorTarget::Source {
                    scene: target.scene.clone(),
                    item: target.item.clone(),
                });
            }
        }
        if self.scene.borrow().is_some() {
            if let Some(target) = self.scene_target.borrow().as_ref() {
                targets.push(ProjectorTarget::Scene {
                    scene: target.scene.clone(),
                });
            }
        }
        targets
    }

    /// Captures open projectors while retaining the last known state for feeds
    /// that are currently closed.
    pub(crate) fn capture_geometry(&self) -> Vec<ProjectorGeometry> {
        let mut geometry = self.geometry.borrow().clone();
        for (feed, slot) in [
            (ProjectorFeed::Program, &self.program),
            (ProjectorFeed::Preview, &self.preview),
            (ProjectorFeed::Multiview, &self.multiview),
            (ProjectorFeed::Source, &self.source),
            (ProjectorFeed::Scene, &self.scene),
        ] {
            let window = slot.borrow();
            if let Some(window) = window.as_ref() {
                if let Some(entry) = capture_projector_geometry(feed, window) {
                    replace_projector_geometry(&mut geometry, entry);
                }
            }
        }
        geometry.sort_unstable_by_key(|entry| entry.projector);
        geometry
    }

    /// Captures the monitor containing each open projector's window center,
    /// while retaining the last known identity for closed feeds.
    pub(crate) fn capture_monitors(&self) -> Vec<ProjectorMonitor> {
        let mut monitors = self.monitors.borrow().clone();
        let choices = screen_monitors();
        for (feed, slot) in [
            (ProjectorFeed::Program, &self.program),
            (ProjectorFeed::Preview, &self.preview),
            (ProjectorFeed::Multiview, &self.multiview),
            (ProjectorFeed::Source, &self.source),
            (ProjectorFeed::Scene, &self.scene),
        ] {
            let window = slot.borrow();
            if let Some(window) = window.as_ref() {
                if let Some(entry) = capture_projector_monitor(feed, window, &choices) {
                    replace_projector_monitor(&mut monitors, entry);
                }
            }
        }
        monitors.sort_unstable_by_key(|entry| entry.projector);
        monitors
    }

    fn remember_geometry(&self, feed: ProjectorFeed, window: &ProjectorWindow) {
        let Some(entry) = capture_projector_geometry(feed, window) else {
            return;
        };
        replace_projector_geometry(&mut self.geometry.borrow_mut(), entry);
    }

    fn remember_monitor(&self, feed: ProjectorFeed, window: &ProjectorWindow) {
        let choices = screen_monitors();
        let Some(entry) = capture_projector_monitor(feed, window, &choices) else {
            return;
        };
        replace_projector_monitor(&mut self.monitors.borrow_mut(), entry);
    }

    fn stored_geometry(&self, feed: ProjectorFeed) -> Option<ProjectorGeometry> {
        self.geometry
            .borrow()
            .iter()
            .find(|entry| entry.projector == feed.kind())
            .copied()
    }

    fn stored_monitor(&self, feed: ProjectorFeed) -> Option<ProjectorMonitor> {
        self.monitors
            .borrow()
            .iter()
            .find(|entry| entry.projector == feed.kind())
            .cloned()
    }

    /// Moves an existing projector to one of the monitors reported by the
    /// platform adapter and remembers the stable identity for the next
    /// restart. The monitor list is resolved again at activation time so a
    /// display removed after the menu opened is a typed failure, not a stale
    /// coordinate write.
    fn move_to_monitor(
        &self,
        feed: ProjectorFeed,
        window: &ProjectorWindow,
        monitor_id: &str,
    ) -> Result<(), String> {
        let monitor = screen_monitors()
            .into_iter()
            .find(|monitor| monitor.id == monitor_id)
            .ok_or_else(|| format!("display '{monitor_id}' is no longer available"))?;
        let stored = ProjectorMonitor::new(feed.kind(), monitor.id.clone())
            .ok_or_else(|| "display identity is invalid".to_owned())?;

        if window.window().is_fullscreen() {
            // Native fullscreen follows the window's current display on the
            // supported desktop backend. Temporarily leaving fullscreen lets
            // the new physical target be selected without opening another
            // projector window or capture runtime.
            window.window().set_fullscreen(false);
            window
                .window()
                .set_position(PhysicalPosition::new(monitor.x, monitor.y));
            window
                .window()
                .set_size(PhysicalSize::new(monitor.width, monitor.height));
            window.window().set_fullscreen(true);
        } else {
            let size = window.window().size();
            let (x, y) = clamp_window_position(
                monitor.x,
                monitor.y,
                size.width,
                size.height,
                monitor_bounds(&monitor),
            );
            window.window().set_position(PhysicalPosition::new(x, y));
        }

        replace_projector_monitor(&mut self.monitors.borrow_mut(), stored);
        Ok(())
    }

    fn set_open(&self, feed: ProjectorFeed, open: bool) {
        if let Some(entry) = self
            .geometry
            .borrow_mut()
            .iter_mut()
            .find(|entry| entry.projector == feed.kind())
        {
            entry.open = open;
        }
    }

    /// Reopens projectors that were open at the previous clean shutdown. A
    /// source or scene target is reopened only when it still resolves in the
    /// active project; fixed feeds need no target record.
    pub(crate) fn reopen_persisted(
        self: &Rc<Self>,
        ui: &MainWindow,
        state: &Rc<RefCell<DesktopState>>,
    ) {
        for feed in [
            ProjectorFeed::Program,
            ProjectorFeed::Preview,
            ProjectorFeed::Multiview,
            ProjectorFeed::Source,
            ProjectorFeed::Scene,
        ] {
            if !self.stored_geometry(feed).is_some_and(|entry| entry.open) {
                continue;
            }
            if !self.target_is_available(feed, state) {
                self.set_open(feed, false);
                continue;
            }
            match open_projector(ui, state, self, feed) {
                Ok(window) => {
                    *self.slot(feed).borrow_mut() = Some(window);
                    self.sync(ui);
                }
                Err(error) => {
                    ui.set_status_message(format!("Projector restore: {error}").into());
                }
            }
        }
    }

    fn target_is_available(&self, feed: ProjectorFeed, state: &Rc<RefCell<DesktopState>>) -> bool {
        let state = state.borrow();
        let profile = state.project_session().project().active_profile_spec();
        match feed {
            ProjectorFeed::Source => self.source_target.borrow().as_ref().is_some_and(|target| {
                profile.is_some_and(|profile| {
                    crate::callbacks::canvas::canvas_item_for_target(
                        profile,
                        target.scene.as_str(),
                        target.item.as_str(),
                    )
                    .is_some_and(obs_rs_project::SceneItemSpec::is_source)
                })
            }),
            ProjectorFeed::Scene => self.scene_target.borrow().as_ref().is_some_and(|target| {
                profile
                    .and_then(|profile| profile.scene(target.scene.as_str()))
                    .is_some()
            }),
            ProjectorFeed::Program | ProjectorFeed::Preview | ProjectorFeed::Multiview => true,
        }
    }

    /// Pushes the studio's current images into any open projector.
    pub(crate) fn sync(&self, ui: &MainWindow) {
        if let Some(window) = self.program.borrow().as_ref() {
            window.set_source_image(ui.get_program_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.preview.borrow().as_ref() {
            window.set_source_image(ui.get_preview_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.multiview.borrow().as_ref() {
            window.set_source_image(ui.get_multiview_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.source.borrow().as_ref() {
            window.set_source_image(ui.get_source_projector_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
        if let Some(window) = self.scene.borrow().as_ref() {
            window.set_source_image(ui.get_scene_projector_image());
            window.set_canvas_width(ui.get_canvas_width());
            window.set_canvas_height(ui.get_canvas_height());
        }
    }

    /// Repaints open projectors when the studio theme changes.
    pub(crate) fn set_tokens(&self, tokens: &crate::ThemeTokens) {
        for window in [
            &self.program,
            &self.preview,
            &self.multiview,
            &self.source,
            &self.scene,
        ] {
            if let Some(window) = window.borrow().as_ref() {
                window.global::<crate::Palette>().set_tokens(tokens.clone());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_open(&self, program: bool) -> bool {
        self.slot(if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        })
        .borrow()
        .is_some()
    }

    #[cfg(test)]
    pub(crate) fn projector_window(&self, program: bool) -> Option<slint::Weak<ProjectorWindow>> {
        self.slot(if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        })
        .borrow()
        .as_ref()
        .map(ComponentHandle::as_weak)
    }

    #[cfg(test)]
    pub(crate) fn is_multiview_open(&self) -> bool {
        self.slot(ProjectorFeed::Multiview).borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_multiview_fullscreen(&self) -> bool {
        self.slot(ProjectorFeed::Multiview)
            .borrow()
            .as_ref()
            .is_some_and(|window| window.window().is_fullscreen())
    }

    #[cfg(test)]
    pub(crate) fn is_source_open(&self) -> bool {
        self.slot(ProjectorFeed::Source).borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_scene_open(&self) -> bool {
        self.slot(ProjectorFeed::Scene).borrow().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_fullscreen(&self, program: bool) -> bool {
        self.slot(if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        })
        .borrow()
        .as_ref()
        .is_some_and(|window| window.window().is_fullscreen())
    }

    const fn slot(&self, feed: ProjectorFeed) -> &RefCell<Option<ProjectorWindow>> {
        match feed {
            ProjectorFeed::Program => &self.program,
            ProjectorFeed::Preview => &self.preview,
            ProjectorFeed::Multiview => &self.multiview,
            ProjectorFeed::Source => &self.source,
            ProjectorFeed::Scene => &self.scene,
        }
    }
}
pub(super) fn install_projectors(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
) {
    let weak = ui.as_weak();
    let preview_state = Rc::clone(state);
    let preview_projectors = Rc::clone(projectors);
    ui.on_open_projector(move |program| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Selecting an open projector again closes it, so the menu entry is a
        // toggle rather than a way to stack duplicate windows.
        let feed = if program {
            ProjectorFeed::Program
        } else {
            ProjectorFeed::Preview
        };
        if preview_projectors.slot(feed).borrow().is_some() {
            close_projector(&preview_projectors, feed);
            return;
        }
        match open_projector(&ui, &preview_state, &preview_projectors, feed) {
            Ok(window) => {
                *preview_projectors.slot(feed).borrow_mut() = Some(window);
                preview_projectors.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Projector: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let multiview_state = Rc::clone(state);
    let multiview_projectors = Rc::clone(projectors);
    ui.on_open_multiview_projector(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let feed = ProjectorFeed::Multiview;
        if multiview_projectors.slot(feed).borrow().is_some() {
            close_projector(&multiview_projectors, feed);
            return;
        }
        match open_projector(&ui, &multiview_state, &multiview_projectors, feed) {
            Ok(window) => {
                *multiview_projectors.slot(feed).borrow_mut() = Some(window);
                multiview_projectors.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Projector: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_projectors = Rc::clone(projectors);
    ui.on_open_source_projector(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let feed = ProjectorFeed::Source;
        if source_projectors.slot(feed).borrow().is_some() {
            close_projector(&source_projectors, feed);
            return;
        }
        let item = ui.get_selected_source().to_string();
        let target = source_target(&source_state.borrow(), &item);
        let Some(target) = target else {
            ui.set_status_message("Select a source before opening its projector".into());
            return;
        };
        *source_projectors.source_target.borrow_mut() = Some(SourceProjectorTarget {
            scene: target.scene,
            item: target.item,
        });
        match open_projector(&ui, &source_state, &source_projectors, feed) {
            Ok(window) => {
                *source_projectors.slot(feed).borrow_mut() = Some(window);
                source_projectors.sync(&ui);
            }
            Err(error) => {
                source_projectors.source_target.borrow_mut().take();
                ui.set_status_message(format!("Projector: {error}").into());
            }
        }
    });

    install_scene_projector(ui, state, projectors);
}

fn install_scene_projector(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
) {
    let weak = ui.as_weak();
    let scene_state = Rc::clone(state);
    let scene_projectors = Rc::clone(projectors);
    ui.on_open_scene_projector(move |scene| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let feed = ProjectorFeed::Scene;
        if scene_projectors.slot(feed).borrow().is_some() {
            close_projector(&scene_projectors, feed);
            return;
        }
        let scene = scene.to_string();
        let exists = scene_state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene.as_str()))
            .is_some();
        if !exists {
            ui.set_status_message("Scene projector target is unavailable".into());
            return;
        }
        *scene_projectors.scene_target.borrow_mut() = Some(SceneProjectorTarget { scene });
        match open_projector(&ui, &scene_state, &scene_projectors, feed) {
            Ok(window) => {
                *scene_projectors.slot(feed).borrow_mut() = Some(window);
                scene_projectors.sync(&ui);
            }
            Err(error) => {
                scene_projectors.scene_target.borrow_mut().take();
                ui.set_status_message(format!("Projector: {error}").into());
            }
        }
    });
}

fn replace_projector_geometry(geometry: &mut Vec<ProjectorGeometry>, entry: ProjectorGeometry) {
    if let Some(existing) = geometry
        .iter_mut()
        .find(|existing| existing.projector == entry.projector)
    {
        *existing = entry;
    } else if geometry.len() < ProjectorKind::ALL.len() {
        geometry.push(entry);
    }
}

fn replace_projector_monitor(monitors: &mut Vec<ProjectorMonitor>, entry: ProjectorMonitor) {
    if let Some(existing) = monitors
        .iter_mut()
        .find(|existing| existing.projector == entry.projector)
    {
        *existing = entry;
    } else if monitors.len() < ProjectorKind::ALL.len() {
        monitors.push(entry);
    }
}

fn capture_projector_geometry(
    feed: ProjectorFeed,
    window: &ProjectorWindow,
) -> Option<ProjectorGeometry> {
    let fullscreen = window.window().is_fullscreen();
    let position = window.window().position();
    let size = window.window().size();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the scale factor is finite and stored as bounded thousandths"
    )]
    let scale_milli = (window.window().scale_factor().max(0.5) * 1_000.0).round() as u32;
    ProjectorGeometry::new(
        feed.kind(),
        position.x,
        position.y,
        size.width,
        size.height,
        scale_milli,
    )
    .map(|entry| entry.with_fullscreen(fullscreen).with_open(true))
}

fn capture_projector_monitor(
    feed: ProjectorFeed,
    window: &ProjectorWindow,
    monitors: &[MonitorChoice],
) -> Option<ProjectorMonitor> {
    let position = window.window().position();
    let size = window.window().size();
    let center_x = i64::from(position.x).saturating_add(i64::from(size.width) / 2);
    let center_y = i64::from(position.y).saturating_add(i64::from(size.height) / 2);
    let monitor = monitor_containing_point(monitors, center_x, center_y)?;
    ProjectorMonitor::new(feed.kind(), monitor.id.clone())
}

pub(super) fn monitor_containing_point(
    monitors: &[MonitorChoice],
    x: i64,
    y: i64,
) -> Option<&MonitorChoice> {
    monitors.iter().find(|monitor| {
        let right = i64::from(monitor.x).saturating_add(i64::from(monitor.width));
        let bottom = i64::from(monitor.y).saturating_add(i64::from(monitor.height));
        x >= i64::from(monitor.x) && x < right && y >= i64::from(monitor.y) && y < bottom
    })
}

fn monitor_bounds(monitor: &MonitorChoice) -> DesktopBounds {
    desktop_bounds(std::slice::from_ref(monitor))
}

/// Projects the current platform display capability into the projector menu's
/// typed rows. A missing or stale saved identity selects the primary display
/// (or the first reported display) as the visible default without rewriting
/// persistence until the user explicitly chooses a target.
fn projector_monitor_rows(selected: Option<&str>) -> Vec<MonitorRow> {
    let monitors = screen_monitors();
    projector_monitor_rows_for(&monitors, selected)
}

pub(super) fn projector_monitor_rows_for(
    monitors: &[MonitorChoice],
    selected: Option<&str>,
) -> Vec<MonitorRow> {
    let bounds = desktop_bounds(monitors);
    let selected_id = selected
        .filter(|id| monitors.iter().any(|monitor| monitor.id.as_str() == *id))
        .map(str::to_owned)
        .or_else(|| {
            monitors
                .iter()
                .find(|monitor| monitor.primary)
                .or_else(|| monitors.first())
                .map(|monitor| monitor.id.clone())
        });

    monitors
        .iter()
        .map(|monitor| {
            let width = i32::try_from(monitor.width).unwrap_or(i32::MAX);
            let height = i32::try_from(monitor.height).unwrap_or(i32::MAX);
            MonitorRow {
                id: monitor.id.as_str().into(),
                name: monitor.name.as_str().into(),
                geometry: monitor.geometry().into(),
                primary: monitor.primary,
                selected: selected_id.as_deref().is_some_and(|id| id == monitor.id),
                normalized_x: normalized_monitor(
                    monitor.x.saturating_sub(bounds.left),
                    bounds.width,
                ),
                normalized_y: normalized_monitor(
                    monitor.y.saturating_sub(bounds.top),
                    bounds.height,
                ),
                normalized_width: normalized_monitor(width, bounds.width),
                normalized_height: normalized_monitor(height, bounds.height),
            }
        })
        .collect()
}

fn normalized_monitor(value: i32, extent: i32) -> f32 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "desktop geometry is far below f32's exact integer range"
    )]
    let fraction = value as f32 / extent.max(1) as f32;
    fraction.clamp(0.0, 1.0)
}

fn restore_projector_geometry(
    window: &ProjectorWindow,
    geometry: ProjectorGeometry,
    stored_monitor: Option<&ProjectorMonitor>,
) {
    let monitor = stored_monitor.and_then(|stored| {
        screen_monitors()
            .into_iter()
            .find(|monitor| monitor.id == stored.monitor)
    });
    let bounds = monitor
        .as_ref()
        .map(monitor_bounds)
        .or_else(current_desktop_bounds);
    if geometry.fullscreen {
        if let Some(monitor) = monitor.as_ref() {
            let bounds = monitor_bounds(monitor);
            window
                .window()
                .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
        }
        window.window().set_fullscreen(true);
        return;
    }
    window.window().set_fullscreen(false);
    let current_scale = window.window().scale_factor().max(0.5);
    #[allow(
        clippy::cast_precision_loss,
        reason = "the stored scale is bounded thousandths and f32 is sufficient for DPI"
    )]
    let saved_scale = (geometry.scale_milli as f32 / 1_000.0).max(0.5);
    let ratio = (current_scale / saved_scale).clamp(0.5, 2.0);
    let width = scale_window_dimension(
        geometry.width,
        ratio,
        FloatingGeometry::MIN_WIDTH,
        FloatingGeometry::MAX_WIDTH,
    );
    let height = scale_window_dimension(
        geometry.height,
        ratio,
        FloatingGeometry::MIN_HEIGHT,
        FloatingGeometry::MAX_HEIGHT,
    );
    let (x, y) = bounds.map_or((geometry.x, geometry.y), |bounds| {
        clamp_window_position(geometry.x, geometry.y, width, height, bounds)
    });
    window
        .window()
        .set_position(slint::PhysicalPosition::new(x, y));
    window
        .window()
        .set_size(slint::PhysicalSize::new(width, height));
}

fn open_projector(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    projectors: &Rc<ProjectorController>,
    feed: ProjectorFeed,
) -> Result<ProjectorWindow, slint::PlatformError> {
    let window = ProjectorWindow::new()?;
    let locale = state.borrow().locale();
    window
        .global::<crate::I18n>()
        .set_text(crate::i18n::catalog(locale));
    window
        .global::<crate::Palette>()
        .set_tokens(ui.global::<crate::Palette>().get_tokens());
    window.set_feed_label(crate::i18n::with_catalog(locale, |text| match feed {
        ProjectorFeed::Program => text.program.clone(),
        ProjectorFeed::Preview => text.preview.clone(),
        ProjectorFeed::Multiview => text.menu_multiview_projector.clone(),
        ProjectorFeed::Source => text.menu_source_projector.clone(),
        ProjectorFeed::Scene => text.scene_projector.clone(),
    }));
    window.set_source_image(match feed {
        ProjectorFeed::Program => ui.get_program_image(),
        ProjectorFeed::Preview => ui.get_preview_image(),
        ProjectorFeed::Multiview => ui.get_multiview_image(),
        ProjectorFeed::Source => ui.get_source_projector_image(),
        ProjectorFeed::Scene => ui.get_scene_projector_image(),
    });
    // OBS presents program and multiview projectors as borderless fullscreen
    // feeds by default. A stored toggle wins, so F11 survives a restart while
    // a first open still follows the feed's reference default.
    if let Some(geometry) = projectors.stored_geometry(feed) {
        let monitor = projectors.stored_monitor(feed);
        restore_projector_geometry(&window, geometry, monitor.as_ref());
    } else {
        window.window().set_fullscreen(feed.is_fullscreen());
    }
    let selected_monitor = projectors.stored_monitor(feed);
    window.set_monitor_rows(ModelRc::new(VecModel::from(projector_monitor_rows(
        selected_monitor
            .as_ref()
            .map(|monitor| monitor.monitor.as_str()),
    ))));

    let close_projectors = Rc::clone(projectors);
    window.on_close_requested(move || close_projector(&close_projectors, feed));
    let fullscreen_window = window.as_weak();
    window.on_toggle_fullscreen(move || {
        if let Some(window) = fullscreen_window.upgrade() {
            window
                .window()
                .set_fullscreen(!window.window().is_fullscreen());
        }
    });

    let move_projectors = Rc::clone(projectors);
    let move_window = window.as_weak();
    let move_ui = ui.as_weak();
    window.on_move_to_monitor(move |monitor_id| {
        let Some(window) = move_window.upgrade() else {
            return;
        };
        match move_projectors.move_to_monitor(feed, &window, monitor_id.as_str()) {
            Ok(()) => {
                let selected_monitor = move_projectors.stored_monitor(feed);
                window.set_monitor_rows(ModelRc::new(VecModel::from(projector_monitor_rows(
                    selected_monitor
                        .as_ref()
                        .map(|monitor| monitor.monitor.as_str()),
                ))));
            }
            Err(error) => {
                if let Some(ui) = move_ui.upgrade() {
                    ui.set_status_message(format!("Projector monitor: {error}").into());
                }
            }
        }
    });

    window.show()?;
    window.invoke_focus_keyboard_boundary();
    Ok(window)
}

fn close_projector(projectors: &Rc<ProjectorController>, feed: ProjectorFeed) {
    if let Some(window) = projectors.slot(feed).borrow_mut().take() {
        projectors.remember_geometry(feed, &window);
        projectors.remember_monitor(feed, &window);
        projectors.set_open(feed, false);
        let _ = window.hide();
    }
    match feed {
        ProjectorFeed::Source => {
            projectors.source_target.borrow_mut().take();
        }
        ProjectorFeed::Scene => {
            projectors.scene_target.borrow_mut().take();
        }
        ProjectorFeed::Program | ProjectorFeed::Preview | ProjectorFeed::Multiview => {}
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;
    use obs_rs_project::{ProjectCommand, SceneItemSpec, SceneSpec};
    use obs_rs_ui::UiCommand;

    #[test]
    fn source_projector_accepts_a_scene_reference_leaf() {
        let (state, _) = crate::tests::canvas_fixture();
        let mut child = SceneSpec::new("projector-child", "Projector child").expect("child scene");
        child
            .add_item(SceneItemSpec::for_source("background").expect("child source"))
            .expect("child source attach");
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: child,
            }))
            .expect("add projector child scene");
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: SceneItemSpec::for_scene("projector-child-ref", "projector-child")
                    .expect("scene reference"),
            }))
            .expect("add projector scene reference");

        let controller = ProjectorController::new();
        *controller.source_target.borrow_mut() = Some(SourceProjectorTarget {
            scene: "preview".to_owned(),
            item: "projector-child-ref/background".to_owned(),
        });
        assert!(controller.target_is_available(ProjectorFeed::Source, &state));

        controller
            .source_target
            .borrow_mut()
            .as_mut()
            .expect("source projector target")
            .item = "projector-child-ref".to_owned();
        assert!(!controller.target_is_available(ProjectorFeed::Source, &state));
    }
}
