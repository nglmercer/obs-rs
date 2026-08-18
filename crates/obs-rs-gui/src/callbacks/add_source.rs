//! Controller for the standalone "Add source" window.
//!
//! The kind list is built from the source factories the runtime actually
//! registered, so a new plugin appears here without touching this file. The
//! card grid lists sources that already exist in the project, which is what
//! separates "create new" from "add existing".

use std::{cell::RefCell, collections::BTreeSet, error::Error, rc::Rc};

use obs_rs_project::{ProjectCommand, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    refresh_ui, source_settings, AddSourceWindow, I18n, MainWindow, Palette, PreviewRenderer,
    SourceCandidate, SourceKindRow,
};

/// Sentinel kind id for the "Recently added" entry at the top of the list.
const RECENT_KIND: &str = "@recent";

/// Owns the window plus the selection that spans its refreshes.
pub(crate) struct AddSourceController {
    window: AddSourceWindow,
    kind: RefCell<String>,
    selected: RefCell<BTreeSet<String>>,
}

impl AddSourceController {
    /// Keeps the window's catalog and palette in step with the studio.
    fn sync_theme(&self, locale: obs_rs_ui::UiLocale, tokens: crate::ThemeTokens) {
        self.window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(locale));
        self.set_tokens(tokens);
    }

    /// Repaints this window when the studio's theme changes.
    pub(crate) fn set_tokens(&self, tokens: crate::ThemeTokens) {
        self.window.global::<Palette>().set_tokens(tokens);
    }
}

/// Creates the Add Source window and wires it to the studio window.
///
/// The returned controller must outlive the event loop; dropping it closes the
/// window.
pub(crate) fn install_add_source_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) -> Result<Rc<AddSourceController>, slint::PlatformError> {
    let controller = Rc::new(AddSourceController {
        window: AddSourceWindow::new()?,
        kind: RefCell::new(RECENT_KIND.to_owned()),
        selected: RefCell::new(BTreeSet::new()),
    });

    install_open(ui, state, renderer, &controller);
    install_selection(state, renderer, &controller);
    install_actions(ui, state, renderer, &controller);
    Ok(controller)
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<AddSourceController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let controller = Rc::clone(controller);
    ui.on_open_add_source_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        controller.selected.borrow_mut().clear();
        RECENT_KIND.clone_into(&mut controller.kind.borrow_mut());
        // Mirror whatever theme and language the studio is showing right now.
        controller.sync_theme(state.borrow().locale(), ui.global::<Palette>().get_tokens());
        refresh_window(&state, &renderer, &controller);
        if let Err(error) = controller.window.show() {
            ui.set_status_message(format!("Add source window: {error}").into());
        }
    });
}

fn install_selection(
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<AddSourceController>,
) {
    let kind_state = Rc::clone(state);
    let kind_renderer = Rc::clone(renderer);
    let kind_controller = Rc::clone(controller);
    controller.window.on_select_kind(move |kind| {
        // Switching kinds drops the selection: the cards it referred to are no
        // longer on screen, so keeping it would add invisible sources.
        kind_controller.selected.borrow_mut().clear();
        *kind_controller.kind.borrow_mut() = kind.to_string();
        refresh_window(&kind_state, &kind_renderer, &kind_controller);
    });

    let toggle_state = Rc::clone(state);
    let toggle_renderer = Rc::clone(renderer);
    let toggle_controller = Rc::clone(controller);
    controller.window.on_toggle_candidate(move |id| {
        let id = id.to_string();
        let mut selected = toggle_controller.selected.borrow_mut();
        if !selected.remove(&id) {
            selected.insert(id);
        }
        drop(selected);
        refresh_window(&toggle_state, &toggle_renderer, &toggle_controller);
    });

    let close_controller = Rc::clone(controller);
    controller.window.on_close_window(move || {
        let _ = close_controller.window.hide();
    });
}

fn install_actions(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<AddSourceController>,
) {
    let weak = ui.as_weak();
    let create_state = Rc::clone(state);
    let create_renderer = Rc::clone(renderer);
    let create_controller = Rc::clone(controller);
    controller.window.on_create_source(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let kind = create_controller.kind.borrow().clone();
        let visible = create_controller.window.get_make_visible();
        let result = create_source(&create_state, &kind, visible);
        let created = result.is_ok();
        report(&ui, &create_state, &create_renderer, result);
        refresh_window(&create_state, &create_renderer, &create_controller);
        // A new screen source captures every monitor until it is told which one
        // to read, so the picker is offered as part of creating it.
        if created && crate::kind_selects_monitor(&kind) {
            ui.invoke_open_monitor_window();
        }
    });

    let weak = ui.as_weak();
    let add_state = Rc::clone(state);
    let add_renderer = Rc::clone(renderer);
    let add_controller = Rc::clone(controller);
    controller.window.on_add_selected(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let selected = add_controller.selected.borrow().clone();
        let visible = add_controller.window.get_make_visible();
        let result = add_existing(&add_state, &selected, visible);
        add_controller.selected.borrow_mut().clear();
        report(&ui, &add_state, &add_renderer, result);
        refresh_window(&add_state, &add_renderer, &add_controller);
    });
}

fn report(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    result: Result<String, Box<dyn Error>>,
) {
    match result {
        Ok(message) => {
            refresh_ui(ui, state, renderer);
            ui.set_status_message(message.into());
        }
        Err(error) => ui.set_status_message(format!("Add source failed: {error}").into()),
    }
}

/// Populates the window from a live project so a test can render it.
#[cfg(test)]
pub(crate) fn populate_add_source_window(
    controller: &Rc<AddSourceController>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    kind: &str,
) {
    *controller.kind.borrow_mut() = kind.to_owned();
    refresh_window(state, renderer, controller);
}

/// Exposes the window handle for rendering assertions.
#[cfg(test)]
pub(crate) fn add_source_window(controller: &Rc<AddSourceController>) -> &AddSourceWindow {
    &controller.window
}

/// Rebuilds both models plus the derived footer state.
fn refresh_window(
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<AddSourceController>,
) {
    let window = &controller.window;
    let text = window.global::<I18n>().get_text().add_source_ui;
    let active_kind = controller.kind.borrow().clone();

    let mut kind_rows = vec![SourceKindRow {
        id: RECENT_KIND.into(),
        label: text.recently_added.clone(),
        icon: "source-generic".into(),
        obsolete: false,
    }];
    let mut listed = renderer
        .borrow()
        .runtime
        .source_kinds()
        // A screen kind that cannot work in this session is hidden rather than
        // offered: the X11 adapter under Wayland only sees Xwayland's own
        // surfaces, which is a black frame, and the portal needs a compositor.
        .filter(|kind| crate::kind_runs_in_this_session(kind.as_str()))
        .map(|kind| SourceKindRow {
            id: kind.as_str().into(),
            label: kind_label(&text, kind.as_str()),
            icon: kind_icon(kind.as_str()).into(),
            obsolete: false,
        })
        .collect::<Vec<_>>();
    // OBS lists kinds by their displayed name, which is locale dependent.
    listed.sort_by(|left, right| left.label.cmp(&right.label));
    kind_rows.extend(listed);
    window.set_kind_rows(ModelRc::new(VecModel::from(kind_rows)));

    let target_scene = state.borrow().preview_scene().map(str::to_owned);
    let candidates = collect_candidates(
        state,
        &text,
        &active_kind,
        target_scene.as_deref(),
        &controller.selected.borrow(),
    );
    let selected_count = i32::try_from(
        candidates
            .iter()
            .filter(|candidate| candidate.selected)
            .count(),
    )
    .unwrap_or(0);
    window.set_candidates(ModelRc::new(VecModel::from(candidates)));
    window.set_selected_kind(active_kind.as_str().into());
    window.set_selected_kind_label(if active_kind == RECENT_KIND {
        text.recently_added.clone()
    } else {
        kind_label(&text, &active_kind)
    });
    window.set_can_create(active_kind != RECENT_KIND);
    window.set_selected_count(selected_count);
    window.set_target_scene(
        state
            .borrow()
            .preview_scene()
            .map(SharedString::from)
            .unwrap_or_default(),
    );
}

/// Existing sources shown as cards, filtered to `kind` unless it is the
/// "recently added" sentinel.
///
/// Sources that the target scene already holds are left out. OBS never offers
/// to add a source to the scene it is already in, and offering it here produced
/// a second identical row rather than anything the user could use.
fn collect_candidates(
    state: &Rc<RefCell<DesktopState>>,
    text: &crate::AddSourceText,
    kind: &str,
    target_scene: Option<&str>,
    selected: &BTreeSet<String>,
) -> Vec<SourceCandidate> {
    let state = state.borrow();
    let session = state.project_session();
    let project = session.project();
    let Some(profile) = project.active_profile_spec() else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for source in profile.sources() {
        if kind != RECENT_KIND && source.kind().as_str() != kind {
            continue;
        }
        if already_in_scene(profile, target_scene, source) {
            continue;
        }
        let id = source.id().as_str().to_owned();
        let owner_scene = profile
            .scenes()
            .find(|scene| scene.has_source(source.id()))
            .map_or("Source collection", obs_rs_project::SceneSpec::name);
        candidates.push(SourceCandidate {
            selected: selected.contains(&id),
            id: id.into(),
            name: source.name().into(),
            kind: source.kind().as_str().into(),
            kind_label: kind_label(text, source.kind().as_str()),
            scene: owner_scene.into(),
            icon: kind_icon(source.kind().as_str()).into(),
        });
    }
    candidates
}

/// Returns whether the target scene already shows this source.
fn already_in_scene(
    profile: &obs_rs_project::Profile,
    target_scene: Option<&str>,
    source: &SourceSpec,
) -> bool {
    target_scene
        .and_then(|scene| profile.scene(scene))
        .is_some_and(|scene| scene.has_source(source.id()))
}

fn create_source(
    state: &Rc<RefCell<DesktopState>>,
    kind: &str,
    visible: bool,
) -> Result<String, Box<dyn Error>> {
    let (profile, scene) = target(state)?;
    let id = unique_source_id(state, &scene, kind);
    let name = format!(
        "{} {}",
        kind_display(kind),
        next_ordinal(state, &scene, kind)
    );
    let source = SourceSpec::new(&id, kind, &name, source_settings(kind)?)?;
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: profile.clone(),
            scene: scene.clone(),
            source,
        }))?;
    set_visibility(state, &profile, &scene, &id, visible)?;
    // Select the new item immediately so Properties opens on the source the
    // user just created, which is especially important for choosing a camera
    // or screen device after creation.
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource { id: id.clone() })?;
    Ok(format!("Source added: {name}"))
}

fn add_existing(
    state: &Rc<RefCell<DesktopState>>,
    selected: &BTreeSet<String>,
    visible: bool,
) -> Result<String, Box<dyn Error>> {
    if selected.is_empty() {
        return Err(std::io::Error::other("no source is selected").into());
    }
    let (profile, scene) = target(state)?;
    let mut added = 0_usize;
    let mut skipped = 0_usize;
    for candidate in selected {
        let source_id = candidate.as_str();
        // The card list already excludes duplicates, but the selection can
        // outlive an edit that added the same source another way.
        if is_duplicate(state, source_id, &scene) {
            skipped += 1;
            continue;
        }
        let Some(item_id) = unique_item_id(state, &scene, source_id) else {
            continue;
        };
        let item = obs_rs_project::SceneItemSpec::new(&item_id, source_id)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
                profile: profile.clone(),
                scene: scene.clone(),
                item,
            }))?;
        set_visibility(state, &profile, &scene, &item_id, visible)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::SelectSource { id: item_id })?;
        added += 1;
    }
    if skipped > 0 {
        return Ok(format!(
            "Sources added: {added} ({skipped} already in this scene)"
        ));
    }
    Ok(format!("Sources added: {added}"))
}

/// Returns whether copying `source_id` into `target_scene` would duplicate a
/// source that scene already shows.
fn is_duplicate(state: &Rc<RefCell<DesktopState>>, source_id: &str, target_scene: &str) -> bool {
    let state = state.borrow();
    let session = state.project_session();
    let project = session.project();
    let Some(profile) = project.active_profile_spec() else {
        return false;
    };
    profile
        .source(source_id)
        .is_some_and(|source| already_in_scene(profile, Some(target_scene), source))
}

fn set_visibility(
    state: &Rc<RefCell<DesktopState>>,
    profile: &str,
    scene: &str,
    item: &str,
    visible: bool,
) -> Result<(), Box<dyn Error>> {
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemVisibility {
            profile: profile.to_owned(),
            scene: scene.to_owned(),
            item: item.to_owned(),
            visible,
        }))?;
    Ok(())
}

fn target(state: &Rc<RefCell<DesktopState>>) -> Result<(String, String), Box<dyn Error>> {
    let state = state.borrow();
    let profile = state
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let scene = state
        .preview_scene()
        .ok_or_else(|| std::io::Error::other("no scene is selected"))?
        .to_owned();
    Ok((profile, scene))
}

/// Returns `kind`, `kind_2`, `kind_3`… — the first form free in the profile registry.
fn unique_source_id(state: &Rc<RefCell<DesktopState>>, _scene: &str, kind: &str) -> String {
    let state = state.borrow();
    let profile = state.project_session().project().active_profile_spec();
    if profile.is_none_or(|profile| !profile.has_source(kind)) {
        return kind.to_owned();
    }
    (2_u32..=10_000)
        .map(|suffix| format!("{kind}_{suffix}"))
        .find(|candidate| profile.is_none_or(|profile| !profile.has_source(candidate.as_str())))
        .unwrap_or_else(|| kind.to_owned())
}

fn unique_item_id(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
    source_id: &str,
) -> Option<String> {
    let state = state.borrow();
    let scene = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))?;
    if !scene.has_item(source_id) {
        return Some(source_id.to_owned());
    }
    (2_u32..=10_000)
        .map(|suffix| format!("{source_id}_{suffix}"))
        .find(|candidate| !scene.has_item(candidate.as_str()))
}

/// One-based count of sources of `kind` already in the profile, used for names.
fn next_ordinal(state: &Rc<RefCell<DesktopState>>, _scene: &str, kind: &str) -> usize {
    let state = state.borrow();
    let ordinal = state
        .project_session()
        .project()
        .active_profile_spec()
        .map_or(1, |profile| {
            profile
                .sources()
                .filter(|source| source.kind().as_str() == kind)
                .count()
                + 1
        });
    ordinal
}

/// Maps a runtime kind to its translated label, falling back to the raw id so
/// a plugin kind this build does not know still shows something usable.
pub(crate) fn kind_label(text: &crate::AddSourceText, kind: &str) -> SharedString {
    match kind {
        "color_source" => text.kind_color_source.clone(),
        "test_pattern" => text.kind_test_pattern.clone(),
        "screen_capture" => text.kind_screen_capture.clone(),
        "window_capture" => text.kind_window_capture.clone(),
        "camera_capture" => text.kind_camera_capture.clone(),
        "x11_screen_capture" => text.kind_x11_screen_capture.clone(),
        "x11_window_capture" => text.kind_x11_window_capture.clone(),
        "wayland_screen_capture" => text.kind_wayland_screen_capture.clone(),
        other => other.into(),
    }
}

/// Untranslated name used when generating a source name.
fn kind_display(kind: &str) -> &str {
    match kind {
        "color_source" => "Color",
        "test_pattern" => "Test pattern",
        "screen_capture" | "x11_screen_capture" | "wayland_screen_capture" => "Screen capture",
        "window_capture" | "x11_window_capture" => "Window capture",
        "camera_capture" => "Video capture device",
        other => other,
    }
}

fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "color_source" => "source-color",
        "test_pattern" => "source-pattern",
        "screen_capture" | "x11_screen_capture" | "wayland_screen_capture" => "source-screen",
        "window_capture" | "x11_window_capture" => "source-window",
        "camera_capture" => "source-camera",
        _ => "source-generic",
    }
}
