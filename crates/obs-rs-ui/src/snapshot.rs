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
                "- {}: {} gain={} muted={} peak={}",
                channel.id(),
                channel.name(),
                channel.gain_milli(),
                channel.muted(),
                channel.peak_milli()
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
    #[must_use]
    pub fn web_page(&self) -> String {
        let mut page = String::new();
        page.push_str(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>OBS-RS control room</title>\n<style>body{font-family:system-ui,sans-serif;line-height:1.45;max-width:60rem;margin:2rem auto;padding:0 1rem;background:#101419;color:#edf2f7}main{display:grid;gap:1rem}section{border:1px solid #52606d;border-radius:.5rem;padding:1rem;background:#1b222c}button,input{font:inherit;padding:.5rem;margin:.25rem;border-radius:.35rem;border:1px solid #829ab1}button{background:#2f80ed;color:white;cursor:pointer}button:focus,input:focus{outline:3px solid #f6c344;outline-offset:2px}pre{white-space:pre-wrap;overflow:auto}#status{min-height:1.5rem}</style>\n</head>\n<body>\n<main id=\"main\" aria-labelledby=\"title\">\n<h1 id=\"title\">OBS-RS control room</h1>\n<p>Rust-native local control surface using the validated desktop state model.</p>\n<section aria-labelledby=\"state-label\">\n<h2 id=\"state-label\">Current state</h2>\n<pre id=\"snapshot\" tabindex=\"0\">",
        );
        page.push_str(&escape_html(&self.accessible_snapshot()));
        page.push_str(
            "</pre>\n</section>\n<section aria-labelledby=\"actions-label\">\n<h2 id=\"actions-label\">Actions</h2>\n<div role=\"group\" aria-label=\"Output and scene actions\">\n<button type=\"button\" data-command=\"swap\">Swap preview/program</button>\n<button type=\"button\" data-command=\"record start\">Start recording</button>\n<button type=\"button\" data-command=\"record stop\">Stop recording</button>\n<button type=\"button\" data-command=\"stream start\">Start streaming</button>\n<button type=\"button\" data-command=\"stream stop\">Stop streaming</button>\n<button type=\"button\" data-command=\"transition cut\">Cut transition</button>\n<button type=\"button\" data-command=\"transition fade 500\">50% fade</button>\n</div>\n<form id=\"command-form\">\n<label for=\"command\">Validated command</label>\n<input id=\"command\" name=\"command\" maxlength=\"256\" size=\"32\" autocomplete=\"off\">\n<button type=\"submit\">Apply</button>\n</form>\n<p id=\"status\" role=\"status\" aria-live=\"polite\"></p>\n</section>\n</main>\n<script>\nasync function applyCommand(command){const response=await fetch('/command',{method:'POST',headers:{'Content-Type':'text/plain'},body:command});const body=await response.text();if(response.ok){document.getElementById('snapshot').textContent=body;document.getElementById('status').textContent='Command applied';}else{document.getElementById('status').textContent=body;}}\ndocument.querySelectorAll('[data-command]').forEach((button)=>button.addEventListener('click',()=>applyCommand(button.dataset.command)));\ndocument.getElementById('command-form').addEventListener('submit',(event)=>{event.preventDefault();const input=document.getElementById('command');applyCommand(input.value);input.value='';});\n</script>\n</body>\n</html>\n",
        );
        page = page.replace(
            "<html lang=\"en\">",
            &format!("<html lang=\"{}\">", self.locale.code()),
        );
        page = page.replace(
            "<button type=\"button\" data-command=\"transition fade 500\">50% fade</button>\n</div>",
            "<button type=\"button\" data-command=\"transition fade 500\">50% fade</button>\n<button type=\"button\" data-command=\"take cut\">Take preview (cut)</button>\n<button type=\"button\" data-command=\"take fade 500\">Take preview (50% fade)</button>\n<button type=\"button\" data-command=\"language en\">English</button>\n<button type=\"button\" data-command=\"language es\">Español</button>\n</div>",
        );
        if self.locale == UiLocale::Spanish {
            for (english, spanish) in [
                (
                    "Rust-native local control surface using the validated desktop state model.",
                    "Superficie de control local en Rust que usa el modelo de estado validado.",
                ),
                ("Current state", "Estado actual"),
                ("Actions", "Acciones"),
                ("Output and scene actions", "Acciones de salida y escena"),
                ("Swap preview/program", "Intercambiar vista previa/al aire"),
                ("Start recording", "Iniciar grabación"),
                ("Stop recording", "Detener grabación"),
                ("Start streaming", "Iniciar transmisión"),
                ("Stop streaming", "Detener transmisión"),
                ("Cut transition", "Transición de corte"),
                ("50% fade", "Fundido al 50%"),
                ("Take preview (cut)", "Enviar vista previa (corte)"),
                (
                    "Take preview (50% fade)",
                    "Enviar vista previa (fundido al 50%)",
                ),
                ("Validated command", "Comando validado"),
                ("Apply", "Aplicar"),
                ("Command applied", "Comando aplicado"),
            ] {
                page = page.replace(english, spanish);
            }
        }
        page
    }
}
