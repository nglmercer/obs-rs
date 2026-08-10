use std::collections::BTreeMap;

use obs_rs_audio::AudioFormat;
use obs_rs_audio::{AudioMixer, AudioSourceId};
use obs_rs_project::Project;
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

pub(crate) fn localized_label(locale: UiLocale, key: &str) -> &'static str {
    match (locale, key) {
        (UiLocale::Spanish, "desktop_state") => "estado de escritorio",
        (UiLocale::Spanish, "project") => "Proyecto",
        (UiLocale::Spanish, "profile") => "Perfil",
        (UiLocale::Spanish, "preview_scene") => "Escena de vista previa",
        (UiLocale::Spanish, "program_scene") => "Escena al aire",
        (UiLocale::Spanish, "selected_source") => "Fuente seleccionada",
        (UiLocale::Spanish, "transition") => "Transición",
        (UiLocale::Spanish, "recording") => "Grabación",
        (UiLocale::Spanish, "streaming") => "Transmisión",
        (UiLocale::Spanish, "project_changes") => "Cambios del proyecto",
        (UiLocale::Spanish, "audio_mixer") => "Mezclador de audio",
        (UiLocale::Spanish, "scenes") => "Escenas",
        (UiLocale::Spanish, "shortcuts") => "Atajos",
        (UiLocale::Spanish, "recent_notices") => "Avisos recientes",
        (UiLocale::English, "desktop_state") => "desktop state",
        (UiLocale::English, "project") => "Project",
        (UiLocale::English, "profile") => "Profile",
        (UiLocale::English, "preview_scene") => "Preview scene",
        (UiLocale::English, "program_scene") => "Program scene",
        (UiLocale::English, "selected_source") => "Selected source",
        (UiLocale::English, "transition") => "Transition",
        (UiLocale::English, "recording") => "Recording",
        (UiLocale::English, "streaming") => "Streaming",
        (UiLocale::English, "project_changes") => "Project changes",
        (UiLocale::English, "audio_mixer") => "Audio mixer",
        (UiLocale::English, "scenes") => "Scenes",
        (UiLocale::English, "shortcuts") => "Shortcuts",
        (UiLocale::English, "recent_notices") => "Recent notices",
        (_, _) => "State",
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

pub(crate) fn first_scene_id(project: &Project) -> Option<Identifier> {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().next())
        .map(|scene| scene.id().clone())
}

pub(crate) fn project_has_scene(project: &Project, scene_id: &Identifier) -> bool {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .is_some_and(|profile| profile.scenes().any(|scene| scene.id() == scene_id))
}

pub(crate) fn project_has_source(
    project: &Project,
    scene_id: &Identifier,
    source_id: &Identifier,
) -> bool {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().find(|scene| scene.id() == scene_id))
        .is_some_and(|scene| {
            scene
                .sources()
                .iter()
                .any(|source| source.id() == source_id)
        })
}

pub(crate) fn first_source_id(project: &Project, scene_id: &Identifier) -> Option<Identifier> {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().find(|scene| scene.id() == scene_id))
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
                muted: false,
                peak_milli: 0,
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
