use std::fmt::Write;

use obs_rs_project::Project;
use obs_rs_util::Identifier;

use super::{
    helpers::{escape_html, localized_label, localized_saved_state, localized_state},
    state::DesktopState,
    types::UiLocale,
};

impl DesktopState {
    #[must_use]
    pub fn accessible_snapshot(&self) -> String {
        let project = self.project.project();
        let active_profile = project.active_profile();
        let locale = self.locale;
        let mut snapshot = String::new();
        self.append_accessible_overview(&mut snapshot, project, active_profile, locale);
        self.append_accessible_mixer(&mut snapshot);
        self.append_accessible_scenes(&mut snapshot, project, active_profile, locale);
        self.append_accessible_footer(&mut snapshot, locale);
        snapshot
    }

    fn append_accessible_overview(
        &self,
        snapshot: &mut String,
        project: &Project,
        active_profile: &Identifier,
        locale: UiLocale,
    ) {
        writeln!(
            snapshot,
            "OBS-RS {} ({})",
            localized_label(locale, "desktop_state"),
            locale.code()
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "project"),
            project.title()
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {active_profile}",
            localized_label(locale, "profile")
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "preview_scene"),
            self.preview_scene().unwrap_or("none")
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "program_scene"),
            self.program_scene().unwrap_or("none")
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "selected_source"),
            self.selected_source().unwrap_or("none")
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {:?}",
            localized_label(locale, "transition"),
            self.transition
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "recording"),
            localized_state(self.recording, locale)
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "streaming"),
            localized_state(self.streaming, locale)
        )
        .expect("String formatting cannot fail");
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "project_changes"),
            localized_saved_state(self.is_dirty(), locale)
        )
        .expect("String formatting cannot fail");
    }

    fn append_accessible_mixer(&self, snapshot: &mut String) {
        snapshot.push_str(localized_label(self.locale, "audio_mixer"));
        snapshot.push_str(":\n");
        for channel in self.mixer_channels() {
            writeln!(
                snapshot,
                "- {}: {} gain={} pan={} muted={} peak={} peak_hold={} clipped={}",
                channel.id(),
                channel.name(),
                channel.gain_milli(),
                channel.pan_milli(),
                channel.muted(),
                channel.peak_milli(),
                channel.peak_hold_milli(),
                channel.clipped()
            )
            .expect("String formatting cannot fail");
        }
    }

    fn append_accessible_scenes(
        &self,
        snapshot: &mut String,
        project: &Project,
        active_profile: &Identifier,
        locale: UiLocale,
    ) {
        snapshot.push_str(localized_label(locale, "scenes"));
        snapshot.push_str(":\n");
        if let Some(profile) = project
            .profiles()
            .find(|profile| profile.id() == active_profile)
        {
            for scene in profile.scenes() {
                let preview = if self.preview_scene() == Some(scene.id().as_str()) {
                    " [preview]"
                } else {
                    ""
                };
                let program = if self.program_scene() == Some(scene.id().as_str()) {
                    " [program]"
                } else {
                    ""
                };
                writeln!(
                    snapshot,
                    "- {}: {}{}{}",
                    scene.id(),
                    scene.name(),
                    preview,
                    program
                )
                .expect("String formatting cannot fail");
            }
        }
    }

    fn append_accessible_footer(&self, snapshot: &mut String, locale: UiLocale) {
        writeln!(
            snapshot,
            "{}: {}",
            localized_label(locale, "shortcuts"),
            self.shortcuts.len()
        )
        .expect("String formatting cannot fail");
        snapshot.push_str(localized_label(locale, "recent_notices"));
        snapshot.push_str(":\n");
        for notice in self.notices() {
            writeln!(snapshot, "- #{}: {}", notice.sequence(), notice.message())
                .expect("String formatting cannot fail");
        }
    }

    /// Renders the accessible local browser control page for the current state.
    ///
    /// Localized text is selected while the document is assembled, so the page
    /// is written once. The previous form built an English page and then ran a
    /// chain of whole-document `replace` calls over it, each of which allocated
    /// and copied the entire page.
    #[must_use]
    pub fn web_page(&self) -> String {
        let text = |key: &str| web_text(self.locale, key);
        let mut page = String::with_capacity(WEB_PAGE_ESTIMATE);

        page.push_str("<!doctype html>\n<html lang=\"");
        page.push_str(self.locale.code());
        page.push_str("\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>OBS-RS control room</title>\n<style>body{font-family:system-ui,sans-serif;line-height:1.45;max-width:60rem;margin:2rem auto;padding:0 1rem;background:#101419;color:#edf2f7}main{display:grid;gap:1rem}section{border:1px solid #52606d;border-radius:.5rem;padding:1rem;background:#1b222c}button,input{font:inherit;padding:.5rem;margin:.25rem;border-radius:.35rem;border:1px solid #829ab1}button{background:#2f80ed;color:white;cursor:pointer}button:focus,input:focus{outline:3px solid #f6c344;outline-offset:2px}pre{white-space:pre-wrap;overflow:auto}#status{min-height:1.5rem}</style>\n</head>\n<body>\n<main id=\"main\" aria-labelledby=\"title\">\n<h1 id=\"title\">OBS-RS control room</h1>\n<p>");
        page.push_str(text("intro"));
        page.push_str("</p>\n<section aria-labelledby=\"state-label\">\n<h2 id=\"state-label\">");
        page.push_str(text("current_state"));
        page.push_str("</h2>\n<pre id=\"snapshot\" tabindex=\"0\">");
        page.push_str(&escape_html(&self.accessible_snapshot()));
        page.push_str("</pre>\n</section>\n<section aria-labelledby=\"actions-label\">\n<h2 id=\"actions-label\">");
        page.push_str(text("actions"));
        page.push_str("</h2>\n<div role=\"group\" aria-label=\"");
        page.push_str(text("actions_group"));
        page.push_str("\">\n");

        for (command, key) in [
            ("swap", "swap"),
            ("record start", "record_start"),
            ("record stop", "record_stop"),
            ("stream start", "stream_start"),
            ("stream stop", "stream_stop"),
            ("transition cut", "transition_cut"),
            ("transition fade 500", "fade_50"),
            ("take cut", "take_cut"),
            ("take fade 500", "take_fade"),
            ("language en", "language_en"),
            ("language es", "language_es"),
        ] {
            page.push_str("<button type=\"button\" data-command=\"");
            page.push_str(command);
            page.push_str("\">");
            page.push_str(text(key));
            page.push_str("</button>\n");
        }

        page.push_str("</div>\n<form id=\"command-form\">\n<label for=\"command\">");
        page.push_str(text("validated_command"));
        page.push_str("</label>\n<input id=\"command\" name=\"command\" maxlength=\"256\" size=\"32\" autocomplete=\"off\">\n<button type=\"submit\">");
        page.push_str(text("apply"));
        page.push_str("</button>\n</form>\n<p id=\"status\" role=\"status\" aria-live=\"polite\"></p>\n</section>\n</main>\n<script>\nasync function applyCommand(command){const response=await fetch('/command',{method:'POST',headers:{'Content-Type':'text/plain'},body:command});const body=await response.text();if(response.ok){document.getElementById('snapshot').textContent=body;document.getElementById('status').textContent='");
        page.push_str(text("command_applied"));
        page.push_str("';}else{document.getElementById('status').textContent=body;}}\ndocument.querySelectorAll('[data-command]').forEach((button)=>button.addEventListener('click',()=>applyCommand(button.dataset.command)));\ndocument.getElementById('command-form').addEventListener('submit',(event)=>{event.preventDefault();const input=document.getElementById('command');applyCommand(input.value);input.value='';});\n</script>\n</body>\n</html>\n");

        page
    }
}

/// Typical rendered page length, used to reserve the buffer once.
const WEB_PAGE_ESTIMATE: usize = 4_096;

/// Returns one localized fragment of the browser control page.
///
/// The language-switch buttons intentionally name each language in its own
/// language, so they are the same in every locale.
fn web_text(locale: UiLocale, key: &str) -> &'static str {
    // Language-switch buttons name each language in its own language, so they
    // are handled before the locale split.
    match key {
        "language_en" => return "English",
        "language_es" => return "Español",
        _ => {}
    }

    match locale {
        UiLocale::Spanish => match key {
            "intro" => "Superficie de control local en Rust que usa el modelo de estado validado.",
            "current_state" => "Estado actual",
            "actions" => "Acciones",
            "actions_group" => "Acciones de salida y escena",
            "swap" => "Intercambiar vista previa/al aire",
            "record_start" => "Iniciar grabación",
            "record_stop" => "Detener grabación",
            "stream_start" => "Iniciar transmisión",
            "stream_stop" => "Detener transmisión",
            "transition_cut" => "Transición de corte",
            "fade_50" => "Fundido al 50%",
            "take_cut" => "Enviar vista previa (corte)",
            "take_fade" => "Enviar vista previa (fundido al 50%)",
            "validated_command" => "Comando validado",
            "apply" => "Aplicar",
            "command_applied" => "Comando aplicado",
            _ => "",
        },
        UiLocale::English => match key {
            "intro" => "Rust-native local control surface using the validated desktop state model.",
            "current_state" => "Current state",
            "actions" => "Actions",
            "actions_group" => "Output and scene actions",
            "swap" => "Swap preview/program",
            "record_start" => "Start recording",
            "record_stop" => "Stop recording",
            "stream_start" => "Start streaming",
            "stream_stop" => "Stop streaming",
            "transition_cut" => "Cut transition",
            "fade_50" => "50% fade",
            "take_cut" => "Take preview (cut)",
            "take_fade" => "Take preview (50% fade)",
            "validated_command" => "Validated command",
            "apply" => "Apply",
            "command_applied" => "Command applied",
            _ => "",
        },
    }
}
