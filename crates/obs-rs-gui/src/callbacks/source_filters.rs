//! Controller for the standalone source Filters window.
//!
//! The controller owns the open source target, filter selection, and view
//! drafts. Every project mutation is dispatched as a validated
//! `ProjectCommand`, so list order, names, enabled state, and settings all
//! participate in undo/redo together with the rest of the project.

use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_config::Config;
use obs_rs_project::{ProjectCommand, SourceFilterCategory, SourceFilterSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    filter_properties, refresh_ui, source_target, source_target_is_locked, I18n, MainWindow,
    Palette, PreviewSurface, SourceFilterRow, SourceFiltersWindow, SourceTarget,
};

#[derive(Clone, Copy)]
struct FilterDefinition {
    kind: &'static str,
    name: &'static str,
    spanish: &'static str,
    category: SourceFilterCategory,
}

const FILTER_DEFINITIONS: [FilterDefinition; 18] = [
    FilterDefinition {
        kind: "gain",
        name: "Gain",
        spanish: "Ganancia",
        category: SourceFilterCategory::AudioVideo,
    },
    FilterDefinition {
        kind: "invert_polarity",
        name: "Invert Polarity",
        spanish: "Invertir polaridad",
        category: SourceFilterCategory::AudioVideo,
    },
    FilterDefinition {
        kind: "limiter",
        name: "Limiter",
        spanish: "Limitador",
        category: SourceFilterCategory::AudioVideo,
    },
    FilterDefinition {
        kind: "expander",
        name: "Expander",
        spanish: "Expansor",
        category: SourceFilterCategory::AudioVideo,
    },
    FilterDefinition {
        kind: "noise_gate",
        name: "Noise Gate",
        spanish: "Puerta de ruido",
        category: SourceFilterCategory::AudioVideo,
    },
    FilterDefinition {
        kind: "compressor",
        name: "Compressor",
        spanish: "Compresor",
        category: SourceFilterCategory::AudioVideo,
    },
    FilterDefinition {
        kind: "grayscale",
        name: "Grayscale",
        spanish: "Escala de grises",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "brightness",
        name: "Brightness",
        spanish: "Brillo",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "opacity",
        name: "Opacity",
        spanish: "Opacidad",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "crop_pad",
        name: "Crop/Pad",
        spanish: "Recorte/Relleno",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "color_correction",
        name: "Color Correction",
        spanish: "Corrección de color",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "color_multiply_add",
        name: "Color Multiply/Add",
        spanish: "Multiplicar/Añadir color",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "luma_key",
        name: "Luma Key",
        spanish: "Clave de luma",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "color_key",
        name: "Color Key",
        spanish: "Clave de color",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "chroma_key",
        name: "Chroma Key",
        spanish: "Clave de croma",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "sharpen",
        name: "Sharpen",
        spanish: "Nitidez",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "scroll",
        name: "Scroll",
        spanish: "Desplazamiento",
        category: SourceFilterCategory::Effect,
    },
    FilterDefinition {
        kind: "render_delay",
        name: "Render Delay",
        spanish: "Retardo de renderizado",
        category: SourceFilterCategory::Effect,
    },
];

impl FilterDefinition {
    fn display_name(self, locale: UiLocale) -> &'static str {
        match locale {
            UiLocale::English => self.name,
            UiLocale::Spanish => self.spanish,
        }
    }
}

/// Owns the standalone Filters window and its selected instance ID.
pub(crate) struct SourceFiltersController {
    window: SourceFiltersWindow,
    selected: RefCell<String>,
    target: RefCell<Option<SourceTarget>>,
}

impl SourceFiltersController {
    /// Repaints this window when the studio theme changes.
    pub(crate) fn set_tokens(&self, tokens: crate::ThemeTokens) {
        self.window.global::<Palette>().set_tokens(tokens);
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> &SourceFiltersWindow {
        &self.window
    }
}

/// Creates and wires the standalone Filters window.
pub(crate) fn install_source_filters_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Result<Rc<SourceFiltersController>, slint::PlatformError> {
    let controller = Rc::new(SourceFiltersController {
        window: SourceFiltersWindow::new()?,
        selected: RefCell::new(String::new()),
        target: RefCell::new(None),
    });
    install_open(ui, state, surface, &controller);
    install_actions(ui, state, surface, &controller);
    Ok(controller)
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    _surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SourceFiltersController>,
) {
    let weak = ui.as_weak();
    let window_state = Rc::clone(state);
    let window_controller = Rc::clone(controller);
    ui.on_open_source_filters_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let target = ui.get_selected_source().to_string();
        open_for_target(&ui, &window_state, &window_controller, &target);
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let controller = Rc::clone(controller);
    ui.on_open_source_filters_for(move |target| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        open_for_target(&ui, &state, &controller, target.as_str());
    });
}

fn open_for_target(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
    item: &str,
) {
    let Some(target) = source_target(&state.borrow(), item) else {
        ui.set_status_message("Source filter failed: the target is not a source".into());
        return;
    };
    controller.target.replace(Some(target));
    controller.set_tokens(ui.global::<Palette>().get_tokens());
    refresh_window(state, controller);
    match controller.window.show() {
        Ok(()) => controller.window.invoke_focus_keyboard_boundary(),
        Err(error) => ui.set_status_message(format!("Filters window: {error}").into()),
    }
}

fn install_actions(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SourceFiltersController>,
) {
    let select_controller = Rc::clone(controller);
    let select_state = Rc::clone(state);
    controller.window.on_select_filter(move |id| {
        id.to_string()
            .clone_into(&mut select_controller.selected.borrow_mut());
        refresh_window(&select_state, &select_controller);
    });

    let weak = ui.as_weak();
    let add_state = Rc::clone(state);
    let add_surface = Rc::clone(surface);
    let add_controller = Rc::clone(controller);
    controller.window.on_add_filter(move |kind| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let kind = kind.to_string();
        let result = add_filter(&add_state, &add_controller, &kind);
        report(&ui, &add_state, &add_surface, result);
        refresh_window(&add_state, &add_controller);
    });

    let weak = ui.as_weak();
    let remove_state = Rc::clone(state);
    let remove_surface = Rc::clone(surface);
    let remove_controller = Rc::clone(controller);
    controller.window.on_remove_filter(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = remove_filter(&remove_state, &remove_controller);
        report(&ui, &remove_state, &remove_surface, result);
        refresh_window(&remove_state, &remove_controller);
    });

    let weak = ui.as_weak();
    let toggle_state = Rc::clone(state);
    let toggle_surface = Rc::clone(surface);
    let toggle_controller = Rc::clone(controller);
    controller.window.on_toggle_filter(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = toggle_filter(&toggle_state, &toggle_controller);
        report(&ui, &toggle_state, &toggle_surface, result);
        refresh_window(&toggle_state, &toggle_controller);
    });

    let weak = ui.as_weak();
    let move_state = Rc::clone(state);
    let move_surface = Rc::clone(surface);
    let move_controller = Rc::clone(controller);
    controller.window.on_move_filter(move |delta| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = move_filter(&move_state, &move_controller, delta);
        report(&ui, &move_state, &move_surface, result);
        refresh_window(&move_state, &move_controller);
    });

    let weak = ui.as_weak();
    let rename_state = Rc::clone(state);
    let rename_surface = Rc::clone(surface);
    let rename_controller = Rc::clone(controller);
    controller.window.on_rename_filter(move |name| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = rename_filter(&rename_state, &rename_controller, name.as_str());
        report(&ui, &rename_state, &rename_surface, result);
        refresh_window(&rename_state, &rename_controller);
    });

    let weak = ui.as_weak();
    let property_state = Rc::clone(state);
    let property_surface = Rc::clone(surface);
    let property_controller = Rc::clone(controller);
    controller.window.on_edit_property(move |key, value| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = edit_property(
            &property_state,
            &property_controller,
            key.as_str(),
            value.as_str(),
        );
        report(&ui, &property_state, &property_surface, result);
        refresh_window(&property_state, &property_controller);
    });

    let close_controller = Rc::clone(controller);
    controller.window.on_close_window(move || {
        let _ = close_controller.window.hide();
    });

    // Keep native window-manager dismissal on the same close path as the
    // editor's Cancel/Escape boundary. Filter mutations are already
    // immediate; this only guarantees that the editor is hidden explicitly.
    let native_close_controller = Rc::clone(controller);
    controller.window.window().on_close_requested(move || {
        let _ = native_close_controller.window.hide();
        slint::CloseRequestResponse::HideWindow
    });
}

fn report(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    result: Result<(), Box<dyn Error>>,
) {
    match result {
        Ok(()) => refresh_ui(ui, state, surface),
        Err(error) => ui.set_status_message(format!("Source filter failed: {error}").into()),
    }
}

fn add_filter(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
    kind: &str,
) -> Result<(), Box<dyn Error>> {
    let definition =
        definition(kind).ok_or_else(|| std::io::Error::other("unknown filter kind"))?;
    let locale = state.borrow().locale();
    let (profile, source, locked) = source_context(state, controller)?;
    ensure_unlocked(locked)?;
    let id = unique_filter_id(state, &source, kind);
    let name = filter_instance_name(definition.display_name(locale), definition.kind, &id);
    let filter = SourceFilterSpec::with_category(
        &id,
        &name,
        definition.kind,
        definition.category,
        filter_properties::default_settings(kind),
    )?;
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSourceFilter {
            profile,
            source,
            filter,
        }))?;
    id.clone_into(&mut controller.selected.borrow_mut());
    Ok(())
}

fn remove_filter(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
) -> Result<(), Box<dyn Error>> {
    let (profile, source, locked) = source_context(state, controller)?;
    ensure_unlocked(locked)?;
    let filter = selected_id(controller)?;
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::RemoveSourceFilter {
            profile,
            source,
            filter,
        }))?;
    controller.selected.borrow_mut().clear();
    Ok(())
}

fn toggle_filter(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
) -> Result<(), Box<dyn Error>> {
    let (profile, source, locked) = source_context(state, controller)?;
    ensure_unlocked(locked)?;
    let filter_id = selected_id(controller)?;
    let enabled = filter_snapshot(state, &source, &filter_id)
        .ok_or_else(|| std::io::Error::other("selected filter is missing"))?
        .enabled();
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSourceFilterEnabled {
            profile,
            source,
            filter: filter_id,
            enabled: !enabled,
        }))?;
    Ok(())
}

fn move_filter(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
    delta: i32,
) -> Result<(), Box<dyn Error>> {
    let (profile, source, locked) = source_context(state, controller)?;
    ensure_unlocked(locked)?;
    let filter_id = selected_id(controller)?;
    let index = filter_index(state, &source, &filter_id)
        .ok_or_else(|| std::io::Error::other("selected filter is missing"))?;
    let target = match delta.cmp(&0) {
        std::cmp::Ordering::Less => index.checked_sub(1),
        std::cmp::Ordering::Greater => Some(index.saturating_add(1)),
        std::cmp::Ordering::Equal => Some(index),
    }
    .ok_or_else(|| std::io::Error::other("filter is already at the top"))?;
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::MoveSourceFilter {
            profile,
            source,
            filter: filter_id,
            target_index: target,
        }))?;
    Ok(())
}

fn rename_filter(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let (profile, source, locked) = source_context(state, controller)?;
    ensure_unlocked(locked)?;
    let filter = selected_id(controller)?;
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSourceFilterName {
            profile,
            source,
            filter,
            name: name.to_owned(),
        }))?;
    Ok(())
}

fn edit_property(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
    key: &str,
    value: &str,
) -> Result<(), Box<dyn Error>> {
    let (profile, source, locked) = source_context(state, controller)?;
    ensure_unlocked(locked)?;
    let filter_id = selected_id(controller)?;
    let filter = filter_snapshot(state, &source, &filter_id)
        .ok_or_else(|| std::io::Error::other("selected filter is missing"))?;
    let settings = filter_properties::apply(
        filter.kind().as_str(),
        &filter.settings().serialize(),
        key,
        value,
    )
    .ok_or_else(|| std::io::Error::other("invalid filter property"))?;
    let settings = Config::parse(&settings)?;
    state.borrow_mut().dispatch(UiCommand::Project(
        ProjectCommand::SetSourceFilterSettings {
            profile,
            source,
            filter: filter_id,
            settings,
        },
    ))?;
    Ok(())
}

fn source_context(
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceFiltersController,
) -> Result<(String, String, bool), Box<dyn Error>> {
    let target = controller
        .target
        .borrow()
        .clone()
        .ok_or_else(|| std::io::Error::other("no source target is open"))?;
    let state = state.borrow();
    let profile = state
        .project_session()
        .project()
        .profile(target.profile.as_str());
    if profile
        .and_then(|profile| profile.source(target.source.as_str()))
        .is_none()
    {
        return Err(std::io::Error::other("source definition is missing").into());
    }
    let locked = source_target_is_locked(&state, &target);
    Ok((target.profile, target.source, locked))
}

fn filter_snapshot(
    state: &Rc<RefCell<DesktopState>>,
    source: &str,
    filter: &str,
) -> Option<SourceFilterSpec> {
    let state = state.borrow();
    state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source(source))
        .and_then(|source| {
            source
                .filters()
                .iter()
                .find(|item| item.id().as_str() == filter)
        })
        .cloned()
}

fn filter_index(state: &Rc<RefCell<DesktopState>>, source: &str, filter: &str) -> Option<usize> {
    let state = state.borrow();
    state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source(source))
        .and_then(|source| {
            source
                .filters()
                .iter()
                .position(|item| item.id().as_str() == filter)
        })
}

fn selected_id(controller: &SourceFiltersController) -> Result<String, Box<dyn Error>> {
    let id = controller.selected.borrow().clone();
    if id.is_empty() {
        Err(std::io::Error::other("no filter is selected").into())
    } else {
        Ok(id)
    }
}

fn ensure_unlocked(locked: bool) -> Result<(), Box<dyn Error>> {
    if locked {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "source is locked").into())
    } else {
        Ok(())
    }
}

fn unique_filter_id(state: &Rc<RefCell<DesktopState>>, source: &str, kind: &str) -> String {
    let state = state.borrow();
    let existing = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source(source));
    let is_taken = |candidate: &str| {
        existing.is_some_and(|source| {
            source
                .filters()
                .iter()
                .any(|filter| filter.id().as_str() == candidate)
        })
    };
    if !is_taken(kind) {
        return kind.to_owned();
    }
    for ordinal in 2..=10_000 {
        let candidate = format!("{kind}_{ordinal}");
        if !is_taken(&candidate) {
            return candidate;
        }
    }
    format!("{kind}_overflow")
}

fn definition(kind: &str) -> Option<FilterDefinition> {
    FILTER_DEFINITIONS
        .iter()
        .copied()
        .find(|definition| definition.kind == kind)
}

fn filter_instance_name(base: &str, kind: &str, id: &str) -> String {
    id.strip_prefix(kind)
        .and_then(|suffix| suffix.strip_prefix('_'))
        .and_then(|ordinal| ordinal.parse::<usize>().ok())
        .map_or_else(|| base.to_owned(), |ordinal| format!("{base} {ordinal}"))
}

#[allow(
    clippy::too_many_lines,
    reason = "the filter editor refresh keeps all dependent rows synchronized"
)]
fn refresh_window(state: &Rc<RefCell<DesktopState>>, controller: &SourceFiltersController) {
    let locale = state.borrow().locale();
    controller
        .window
        .global::<I18n>()
        .set_text(crate::i18n::catalog(locale));

    let target = controller.target.borrow().clone();
    let (source_name, filters) = {
        let state = state.borrow();
        let source = target.as_ref().and_then(|target| {
            state
                .project_session()
                .project()
                .profile(target.profile.as_str())
                .and_then(|profile| profile.source(target.source.as_str()))
        });
        (
            source.map_or_else(String::new, |source| source.name().to_owned()),
            source.map_or_else(Vec::new, |source| source.filters().to_vec()),
        )
    };
    controller.window.set_source_name(source_name.into());

    if !filters
        .iter()
        .any(|filter| filter.id().as_str() == controller.selected.borrow().as_str())
    {
        controller.selected.replace(
            filters
                .first()
                .map_or_else(String::new, |filter| filter.id().to_string()),
        );
    }
    let selected = controller.selected.borrow().clone();
    let make_row = |filter: &SourceFilterSpec, index: usize, count: i32| SourceFilterRow {
        id: filter.id().as_str().into(),
        name: filter.name().into(),
        kind: filter_display_name(filter.kind().as_str(), locale).into(),
        category: filter.category().id().into(),
        index: i32::try_from(index).unwrap_or(i32::MAX),
        count,
        enabled: filter.enabled(),
        selected: filter.id().as_str() == selected,
    };
    let audio_filters = filters
        .iter()
        .filter(|filter| filter.category() == SourceFilterCategory::AudioVideo)
        .collect::<Vec<_>>();
    let audio_count = i32::try_from(audio_filters.len()).unwrap_or(i32::MAX);
    let audio = audio_filters
        .into_iter()
        .enumerate()
        .map(|(index, filter)| make_row(filter, index, audio_count))
        .collect::<Vec<_>>();
    let effect_filters = filters
        .iter()
        .filter(|filter| filter.category() == SourceFilterCategory::Effect)
        .collect::<Vec<_>>();
    let effect_count = i32::try_from(effect_filters.len()).unwrap_or(i32::MAX);
    let effects = effect_filters
        .into_iter()
        .enumerate()
        .map(|(index, filter)| make_row(filter, index, effect_count))
        .collect::<Vec<_>>();
    controller
        .window
        .set_audio_video_rows(ModelRc::new(VecModel::from(audio)));
    controller
        .window
        .set_effect_rows(ModelRc::new(VecModel::from(effects)));

    let selected_filter = filters
        .iter()
        .find(|filter| filter.id().as_str() == selected);
    controller
        .window
        .set_selected_filter(selected_filter.is_some());
    controller
        .window
        .set_selected_filter_id(selected.clone().into());
    controller.window.set_selected_filter_name(
        selected_filter
            .map_or_else(String::new, |filter| filter.name().to_owned())
            .into(),
    );
    controller.window.set_selected_filter_kind(
        selected_filter
            .map_or_else(String::new, |filter| {
                filter_display_name(filter.kind().as_str(), locale).to_owned()
            })
            .into(),
    );
    controller
        .window
        .set_selected_filter_enabled(selected_filter.is_some_and(SourceFilterSpec::enabled));
    controller
        .window
        .set_property_rows(ModelRc::new(VecModel::from(selected_filter.map_or_else(
            Vec::new,
            |filter| {
                filter_properties::rows(
                    filter.kind().as_str(),
                    &filter.settings().serialize(),
                    locale,
                )
            },
        ))));
    let index = filters
        .iter()
        .position(|filter| filter.id().as_str() == selected);
    controller
        .window
        .set_can_move_up(index.is_some_and(|index| index > 0));
    controller
        .window
        .set_can_move_down(index.is_some_and(|index| index + 1 < filters.len()));

    let names = FILTER_DEFINITIONS
        .iter()
        .map(|definition| SharedString::from(definition.display_name(locale)))
        .collect::<Vec<_>>();
    let kinds = FILTER_DEFINITIONS
        .iter()
        .map(|definition| SharedString::from(definition.kind))
        .collect::<Vec<_>>();
    controller
        .window
        .set_available_filter_names(ModelRc::new(VecModel::from(names)));
    controller
        .window
        .set_available_filter_kinds(ModelRc::new(VecModel::from(kinds)));
}

fn filter_display_name(kind: &str, locale: UiLocale) -> &str {
    definition(kind).map_or(kind, |definition| definition.display_name(locale))
}

// Keep the public test surface small while allowing headless GUI tests to
// instantiate and inspect the standalone window.
#[cfg(test)]
pub(crate) fn source_filters_window(
    controller: &Rc<SourceFiltersController>,
) -> &SourceFiltersWindow {
    controller.window()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_catalog_has_bounded_bilingual_names() {
        assert_eq!(FILTER_DEFINITIONS.len(), 18);
        assert!(FILTER_DEFINITIONS
            .iter()
            .all(|definition| !definition.name.is_empty() && !definition.spanish.is_empty()));
        assert_eq!(filter_display_name("gain", UiLocale::English), "Gain");
        assert_eq!(filter_display_name("gain", UiLocale::Spanish), "Ganancia");
        assert_eq!(
            filter_display_name("invert_polarity", UiLocale::Spanish),
            "Invertir polaridad"
        );
        assert_eq!(filter_display_name("unknown", UiLocale::Spanish), "unknown");
    }
}
