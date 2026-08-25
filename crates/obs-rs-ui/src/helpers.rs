use std::collections::BTreeMap;

use obs_rs_audio::AudioFormat;
use obs_rs_audio::{AudioMixer, AudioSourceId};
use obs_rs_project::{Profile, Project, SceneItemSpec, SceneSpec};
use obs_rs_util::Identifier;

use super::{
    error::UiError,
    types::{MixerChannel, UiLocale},
};

pub(crate) fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            character => escaped.push(character),
        }
    }
    escaped
}

/// Returns one localized snapshot label.
///
/// The locale is resolved before the key, so a lookup compares against one
/// language's keys rather than walking every (locale, key) pair.
pub(crate) fn localized_label(locale: UiLocale, key: &str) -> &'static str {
    match locale {
        UiLocale::Spanish => match key {
            "desktop_state" => "estado de escritorio",
            "project" => "Proyecto",
            "profile" => "Perfil",
            "preview_scene" => "Escena de vista previa",
            "program_scene" => "Escena al aire",
            "selected_source" => "Fuente seleccionada",
            "transition" => "Transición",
            "recording" => "Grabación",
            "streaming" => "Transmisión",
            "project_changes" => "Cambios del proyecto",
            "audio_mixer" => "Mezclador de audio",
            "scenes" => "Escenas",
            "shortcuts" => "Atajos",
            "recent_notices" => "Avisos recientes",
            _ => "State",
        },
        UiLocale::English => match key {
            "desktop_state" => "desktop state",
            "project" => "Project",
            "profile" => "Profile",
            "preview_scene" => "Preview scene",
            "program_scene" => "Program scene",
            "selected_source" => "Selected source",
            "transition" => "Transition",
            "recording" => "Recording",
            "streaming" => "Streaming",
            "project_changes" => "Project changes",
            "audio_mixer" => "Audio mixer",
            "scenes" => "Scenes",
            "shortcuts" => "Shortcuts",
            "recent_notices" => "Recent notices",
            _ => "State",
        },
    }
}

pub(crate) fn localized_state(active: bool, locale: UiLocale) -> &'static str {
    match (active, locale) {
        (true, UiLocale::English) => "active",
        (false, UiLocale::English) => "stopped",
        (true, UiLocale::Spanish) => "activa",
        (false, UiLocale::Spanish) => "detenida",
    }
}

pub(crate) fn localized_saved_state(dirty: bool, locale: UiLocale) -> &'static str {
    match (dirty, locale) {
        (true, UiLocale::English) => "unsaved",
        (false, UiLocale::English) => "saved",
        (true, UiLocale::Spanish) => "sin guardar",
        (false, UiLocale::Spanish) => "guardado",
    }
}

// These helpers resolve the active profile and a scene by key rather than
// scanning the profile and scene lists, which the UI command path did several
// times per dispatch.

pub(crate) fn first_scene_id(project: &Project) -> Option<Identifier> {
    project
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview").or_else(|| profile.scenes().next()))
        .map(|scene| scene.id().clone())
}

pub(crate) fn project_has_scene(project: &Project, scene_id: &Identifier) -> bool {
    project
        .active_profile_spec()
        .is_some_and(|profile| profile.scene(scene_id).is_some())
}

pub(crate) fn project_has_source(
    project: &Project,
    scene_id: &Identifier,
    source_target: &str,
) -> bool {
    project.active_profile_spec().is_some_and(|profile| {
        profile_scene_item_at_target(profile, scene_id.as_str(), source_target).is_some()
    })
}

const MAX_SCENE_ITEM_PATH_DEPTH: usize = 64;

/// Splits one bounded outer-to-inner scene-item path.
pub(crate) fn scene_item_target_parts(target: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::with_capacity(4);
    for part in target.split('/') {
        if part.is_empty() || parts.len() >= MAX_SCENE_ITEM_PATH_DEPTH {
            return None;
        }
        parts.push(part);
    }
    (!parts.is_empty()).then_some(parts)
}

/// Resolves one flattened scene-item path through groups and Scene references.
/// The project model remains the only source of truth; this is just a bounded
/// read-only lookup for UI validation and selection reconciliation.
pub(crate) fn profile_scene_item_at_target<'a>(
    profile: &'a Profile,
    scene_id: &str,
    target: &str,
) -> Option<&'a SceneItemSpec> {
    let parts = scene_item_target_parts(target)?;
    let mut items = profile.scene(scene_id)?.items();
    for (index, part) in parts.iter().enumerate() {
        let item = items.iter().find(|item| item.id().as_str() == *part)?;
        if index + 1 == parts.len() {
            return Some(item);
        }
        if let Some(group) = item.group() {
            items = group.items();
        } else {
            items = profile.scene(item.scene_id()?)?.items();
        }
    }
    None
}

/// Resolves one outer-to-inner scene-item path.
pub(crate) fn scene_item_at_parts<'a>(
    scene: &'a SceneSpec,
    parts: &[&str],
) -> Option<&'a SceneItemSpec> {
    let mut items = scene.items();
    for (index, part) in parts.iter().enumerate() {
        let item = items.iter().find(|item| item.id().as_str() == *part)?;
        if index + 1 == parts.len() {
            return Some(item);
        }
        items = item.group()?.items();
    }
    None
}

pub(crate) fn first_source_id(project: &Project, scene_id: &Identifier) -> Option<Identifier> {
    project
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.sources().first())
        .map(|source| source.id().clone())
}

pub(crate) fn default_mixer() -> (
    AudioMixer,
    BTreeMap<String, AudioSourceId>,
    BTreeMap<String, MixerChannel>,
) {
    let format = AudioFormat::new(48_000, 2).expect("default mixer format is valid");
    let mut mixer = AudioMixer::new(format);
    let mut sources = BTreeMap::new();
    let mut channels = BTreeMap::new();
    for (id, name) in [("desktop", "Desktop Audio"), ("mic", "Mic/Aux")] {
        let source = mixer
            .add_source(1.0)
            .expect("default mixer source ID is available");
        sources.insert(id.to_owned(), source);
        channels.insert(
            id.to_owned(),
            MixerChannel {
                id: id.to_owned(),
                name: name.to_owned(),
                gain_milli: 1_000,
                pan_milli: 0,
                muted: false,
                peak_milli: 0,
                peak_hold_milli: 0,
                clipped: false,
            },
        );
    }
    (mixer, sources, channels)
}

pub(crate) fn identifier(input: &str, kind: &'static str) -> Result<Identifier, UiError> {
    Identifier::new(input).map_err(|_| UiError::UnknownSelection {
        kind,
        id: input.to_owned(),
    })
}
