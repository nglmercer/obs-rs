use std::collections::{BTreeMap, HashSet, VecDeque};

use obs_rs_audio::{AudioBuffer, AudioFormat, AudioMixer, AudioSourceId};
use obs_rs_media::{FrameTransition, Timestamp};
use obs_rs_project::{
    Project, ProjectCommand, ProjectFileStore, ProjectSession, SceneItemDuplicateMode,
    SceneItemSpec, SceneSpec,
};
use obs_rs_util::Identifier;

use super::{
    error::UiError,
    helpers::{default_mixer, first_scene_id, first_source_id, identifier, project_has_scene},
    types::{MixerChannel, Shortcut, UiAction, UiCommand, UiLocale, UiNotice},
    MAX_UI_NOTICES,
};

/// The last Preview/Program choices for one in-memory profile.
#[derive(Clone, Debug, Default)]
struct SceneSelection {
    preview: Option<Identifier>,
    program: Option<Identifier>,
}

/// The selection cache is session state, not project data. Keep it bounded so
/// repeatedly opening profiles cannot grow the UI state without limit.
const MAX_PROFILE_SCENE_SELECTIONS: usize = 64;

pub struct DesktopState {
    pub(crate) project: ProjectSession,
    pub(crate) preview_scene: Option<Identifier>,
    pub(crate) program_scene: Option<Identifier>,
    profile_scene_selections: BTreeMap<Identifier, SceneSelection>,
    /// Ordered transient canvas selection. The last item is the active item
    /// used by property dialogs and single-row source actions.
    pub(crate) selected_sources: Vec<Identifier>,
    /// Transient scene-item clipboard. It is intentionally outside the
    /// persistent project and is cleared when a different project is loaded.
    pub(crate) clipboard: Option<SceneItemSpec>,
    pub(crate) locale: UiLocale,
    pub(crate) transition: FrameTransition,
    pub(crate) recording: bool,
    pub(crate) streaming: bool,
    pub(crate) audio_mixer: AudioMixer,
    pub(crate) mixer_sources: BTreeMap<String, AudioSourceId>,
    pub(crate) mixer_channels: BTreeMap<String, MixerChannel>,
    pub(crate) shortcuts: BTreeMap<Shortcut, UiAction>,
    pub(crate) notices: VecDeque<UiNotice>,
    pub(crate) next_notice_sequence: u64,
}

impl DesktopState {
    /// Creates a clean desktop state and selects the first scene for both views.
    #[must_use]
    pub fn new(project: Project) -> Self {
        let session = ProjectSession::new(project);
        let first_scene = first_scene_id(session.project());
        let selected_sources = first_scene
            .as_ref()
            .and_then(|scene| first_source_id(session.project(), scene));
        let (audio_mixer, mixer_sources, mixer_channels) = default_mixer();
        Self {
            project: session,
            preview_scene: first_scene.clone(),
            program_scene: first_scene,
            profile_scene_selections: BTreeMap::new(),
            selected_sources: selected_sources.into_iter().collect(),
            clipboard: None,
            locale: UiLocale::English,
            transition: FrameTransition::Cut,
            recording: false,
            streaming: false,
            audio_mixer,
            mixer_sources,
            mixer_channels,
            shortcuts: BTreeMap::new(),
            // Bounded by `MAX_UI_NOTICES`, so the ring is sized once and never
            // grows or reallocates afterwards.
            notices: VecDeque::with_capacity(MAX_UI_NOTICES),
            next_notice_sequence: 1,
        }
    }

    /// Applies one UI command through the validated state machine.
    ///
    /// # Errors
    ///
    /// Returns [`UiError`] and leaves the affected state unchanged when validation
    /// or lifecycle checks fail.
    #[allow(
        clippy::too_many_lines,
        reason = "the command dispatcher keeps the UI state-machine boundary explicit"
    )]
    pub fn dispatch(&mut self, command: UiCommand) -> Result<(), UiError> {
        let message = match command {
            UiCommand::SwapPreviewProgram => {
                std::mem::swap(&mut self.preview_scene, &mut self.program_scene);
                "preview and program scenes swapped"
            }
            UiCommand::Project(command) => {
                let previous_profile = self.project.project().active_profile().clone();
                let previous_selection = self.scene_selection();
                self.project.dispatch(command)?;
                if self.project.project().active_profile() == &previous_profile {
                    self.sync_selections_after_project_update();
                } else {
                    self.remember_profile_selection(previous_profile, previous_selection);
                    self.restore_active_profile_selection();
                }
                "project updated"
            }
            UiCommand::Undo => self.step_history(true),
            UiCommand::Redo => self.step_history(false),
            UiCommand::SelectProfile { id } => {
                let previous_profile = self.project.project().active_profile().clone();
                let previous_selection = self.scene_selection();
                self.project
                    .dispatch(ProjectCommand::SetActiveProfile { id })?;
                self.remember_profile_selection(previous_profile, previous_selection);
                self.restore_active_profile_selection();
                "profile selected"
            }
            UiCommand::SelectPreviewScene { id } => {
                self.ensure_scene(&id)?;
                self.preview_scene = Some(identifier(&id, "scene")?);
                self.selected_sources = self
                    .preview_scene
                    .as_ref()
                    .and_then(|scene| first_source_id(self.project.project(), scene))
                    .into_iter()
                    .collect();
                "preview scene selected"
            }
            UiCommand::SelectProgramScene { id } => {
                self.ensure_scene(&id)?;
                self.program_scene = Some(identifier(&id, "scene")?);
                "program scene selected"
            }
            UiCommand::SelectSource { id } => {
                self.select_one_source(&id)?;
                "source selected"
            }
            UiCommand::ToggleSourceSelection { id } => {
                self.toggle_source_selection(&id)?;
                "source selection toggled"
            }
            UiCommand::SelectSources { ids, additive } => {
                self.select_sources(ids, additive)?;
                "source selection updated"
            }
            UiCommand::CopySource { id } => {
                let preview_scene =
                    self.preview_scene
                        .as_ref()
                        .ok_or_else(|| UiError::UnknownSelection {
                            kind: "scene",
                            id: "none".to_owned(),
                        })?;
                let item = self
                    .project
                    .project()
                    .active_profile_spec()
                    .and_then(|profile| profile.scene(preview_scene))
                    .and_then(|scene| scene_item_at_target(scene, &id))
                    .cloned()
                    .ok_or(UiError::UnknownSelection {
                        kind: "scene item",
                        id,
                    })?;
                self.clipboard = Some(item);
                "source item copied"
            }
            UiCommand::PasteSource { mode, target } => self.paste_source(mode, &target)?,
            UiCommand::SetLocale { locale } => {
                self.locale = locale;
                "language selected"
            }
            UiCommand::SetTransition { transition } => {
                self.set_transition(transition)?;
                "transition updated"
            }
            UiCommand::TakePreview { transition } => self.take_preview(transition)?,
            UiCommand::SetMixerGain { id, gain_milli } => {
                self.set_mixer_gain(&id, gain_milli)?;
                "mixer gain updated"
            }
            UiCommand::SetMixerPan { id, pan_milli } => {
                self.set_mixer_pan(&id, pan_milli)?;
                "mixer pan updated"
            }
            UiCommand::ToggleMixerMute { id } => self.toggle_mixer_mute(&id)?,
            UiCommand::SetAudioFormat {
                sample_rate,
                channels,
            } => {
                self.set_audio_format(sample_rate, channels)?;
                "audio format updated"
            }
            UiCommand::StartRecording => self.set_recording(true)?,
            UiCommand::StopRecording => self.set_recording(false)?,
            UiCommand::StartStreaming => self.set_streaming(true)?,
            UiCommand::StopStreaming => self.set_streaming(false)?,
            UiCommand::BindShortcut { shortcut, action } => {
                if self.shortcuts.contains_key(&shortcut) {
                    return Err(UiError::DuplicateShortcut(shortcut));
                }
                self.shortcuts.insert(shortcut, action);
                "shortcut bound"
            }
            UiCommand::UnbindShortcut { shortcut } => {
                if self.shortcuts.remove(&shortcut).is_none() {
                    return Err(UiError::UnknownShortcut(shortcut));
                }
                "shortcut unbound"
            }
            UiCommand::TriggerShortcut { shortcut } => {
                let action = *self
                    .shortcuts
                    .get(&shortcut)
                    .ok_or_else(|| UiError::UnknownShortcut(shortcut.clone()))?;
                self.dispatch_action(action)?;
                "shortcut triggered"
            }
        };
        self.notice(message)?;
        Ok(())
    }

    /// Saves the current project through the crash-safe project-file store.
    ///
    /// The session is marked clean only after the store has completed its
    /// temporary-file write, synchronization, and rename sequence.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::Project`] when persistence fails. The dirty flag remains
    /// set when the write does not complete successfully.
    pub fn save_project(&mut self, store: &ProjectFileStore) -> Result<usize, UiError> {
        let bytes = store.save(&mut self.project)?;
        self.notice("project saved")?;
        Ok(bytes)
    }

    /// Writes the current project document without changing its dirty state.
    ///
    /// This is the persistence boundary used by collection export. Exporting
    /// is a copy operation, so it must not make the active project appear
    /// saved or alter its undo history.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::Project`] when persistence fails.
    pub fn save_project_document(&self, store: &ProjectFileStore) -> Result<usize, UiError> {
        let document = self.project.document();
        Ok(store.save_document(&document)?)
    }

    /// Loads a project through the crash-safe project-file store.
    ///
    /// The current project is replaced only after the file has been read and
    /// parsed successfully. Preview and program selections reset to the first
    /// scene in the loaded active profile.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::Project`] when the file cannot be read or parsed.
    pub fn load_project(&mut self, store: &ProjectFileStore) -> Result<(), UiError> {
        let project = store.load()?;
        self.replace_project(project);
        self.notice("project loaded")?;
        Ok(())
    }

    /// Recovers a complete temporary project left by an interrupted save.
    ///
    /// Returns `Ok(false)` when no recovery file exists. A recovered project is
    /// intentionally still dirty so the user must explicitly save it.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::Project`] when the recovery artifact cannot be read or
    /// parsed.
    pub fn recover_project(&mut self, store: &ProjectFileStore) -> Result<bool, UiError> {
        let Some(project) = store.recover()? else {
            return Ok(false);
        };
        self.replace_project(project);
        self.project.mark_dirty();
        self.notice("project recovered from interrupted save")?;
        Ok(true)
    }

    /// Moves one step through the project history and reconciles selections.
    ///
    /// Reaching either end is reported as a notice rather than as an error: the
    /// user asked for something that is simply unavailable, not for something
    /// invalid.
    fn step_history(&mut self, backwards: bool) -> &'static str {
        let previous_profile = self.project.project().active_profile().clone();
        let previous_selection = self.scene_selection();
        let moved = if backwards {
            self.project.undo()
        } else {
            self.project.redo()
        };
        if !moved {
            return if backwards {
                "nothing to undo"
            } else {
                "nothing to redo"
            };
        }
        // A restored project may not contain the scene or item that was
        // selected, so the selections are reconciled the same way an ordinary
        // project command reconciles them.
        if self.project.project().active_profile() == &previous_profile {
            self.sync_selections_after_project_update();
        } else {
            self.remember_profile_selection(previous_profile, previous_selection);
            self.restore_active_profile_selection();
        }
        if backwards {
            "edit undone"
        } else {
            "edit redone"
        }
    }

    fn replace_project(&mut self, project: Project) {
        self.project.replace(project);
        self.clipboard = None;
        self.profile_scene_selections.clear();
        let first_scene = first_scene_id(self.project.project());
        self.preview_scene.clone_from(&first_scene);
        self.program_scene.clone_from(&first_scene);
        self.selected_sources = self
            .preview_scene
            .as_ref()
            .and_then(|scene| first_source_id(self.project.project(), scene))
            .into_iter()
            .collect();
    }

    /// Restores session-level preview/program scene choices without creating a
    /// project-history entry.
    ///
    /// The choices live in the desktop session rather than in the project
    /// document. A missing, malformed, or stale choice is ignored so loading
    /// a project always retains the first-scene fallback established by
    /// [`Self::replace_project`].
    pub fn restore_scene_selection(&mut self, preview: Option<&str>, program: Option<&str>) {
        let (preview, program) = {
            let project = self.project.project();
            let preview = preview
                .and_then(|id| Identifier::new(id).ok())
                .filter(|id| project_has_scene(project, id));
            let program = program
                .and_then(|id| Identifier::new(id).ok())
                .filter(|id| project_has_scene(project, id));
            (preview, program)
        };

        if let Some(preview) = preview {
            self.preview_scene = Some(preview);
            self.selected_sources = self
                .preview_scene
                .as_ref()
                .and_then(|scene| first_source_id(self.project.project(), scene))
                .into_iter()
                .collect();
        }
        if let Some(program) = program {
            self.program_scene = Some(program);
        }
    }

    fn scene_selection(&self) -> SceneSelection {
        SceneSelection {
            preview: self.preview_scene.clone(),
            program: self.program_scene.clone(),
        }
    }

    fn remember_profile_selection(&mut self, profile: Identifier, selection: SceneSelection) {
        if self.profile_scene_selections.len() == MAX_PROFILE_SCENE_SELECTIONS
            && !self.profile_scene_selections.contains_key(&profile)
        {
            if let Some(oldest) = self.profile_scene_selections.keys().next().cloned() {
                self.profile_scene_selections.remove(&oldest);
            }
        }
        self.profile_scene_selections.insert(profile, selection);
    }

    fn restore_active_profile_selection(&mut self) {
        let fallback = first_scene_id(self.project.project());
        self.preview_scene.clone_from(&fallback);
        self.program_scene = fallback;
        self.selected_sources = self
            .preview_scene
            .as_ref()
            .and_then(|scene| first_source_id(self.project.project(), scene))
            .into_iter()
            .collect();
        let selection = self
            .profile_scene_selections
            .get(self.project.project().active_profile())
            .cloned();
        if let Some(selection) = selection {
            self.restore_scene_selection(
                selection.preview.as_ref().map(Identifier::as_str),
                selection.program.as_ref().map(Identifier::as_str),
            );
        }
    }

    /// Returns the project session used by persistence and rendering adapters.
    #[must_use]
    pub const fn project_session(&self) -> &ProjectSession {
        &self.project
    }

    /// Serializes the current project without changing its dirty state.
    #[must_use]
    pub fn project_document(&self) -> String {
        self.project.document()
    }

    /// Returns whether project state has unsaved changes.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.project.is_dirty()
    }

    /// Returns whether an earlier project state is available to restore.
    ///
    /// Frontends use this to enable or grey out an Undo affordance rather than
    /// offering one that silently does nothing.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.project.can_undo()
    }

    /// Returns whether an undone project state is available to reapply.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.project.can_redo()
    }

    /// Returns the scene selected for preview.
    #[must_use]
    pub fn preview_scene(&self) -> Option<&str> {
        self.preview_scene.as_ref().map(Identifier::as_str)
    }

    /// Returns the scene selected for program output.
    #[must_use]
    pub fn program_scene(&self) -> Option<&str> {
        self.program_scene.as_ref().map(Identifier::as_str)
    }

    /// Returns the source selected in the current preview scene.
    #[must_use]
    pub fn selected_source(&self) -> Option<&str> {
        self.selected_sources.last().map(Identifier::as_str)
    }

    /// Returns the current ordered canvas selection.
    pub fn selected_sources(&self) -> impl Iterator<Item = &str> {
        self.selected_sources.iter().map(Identifier::as_str)
    }

    /// Returns whether a scene item belongs to the current canvas selection.
    #[must_use]
    pub fn is_source_selected(&self, id: &str) -> bool {
        self.selected_sources
            .iter()
            .any(|selected| selected.as_str() == id)
    }

    /// Returns whether a copied scene item is available to paste.
    #[must_use]
    pub const fn can_paste_source(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Returns the active presentation locale.
    #[must_use]
    pub const fn locale(&self) -> UiLocale {
        self.locale
    }

    /// Returns the current transition policy.
    #[must_use]
    pub const fn transition(&self) -> FrameTransition {
        self.transition
    }

    /// Returns whether recording is active.
    #[must_use]
    pub const fn recording(&self) -> bool {
        self.recording
    }

    /// Returns whether streaming is active.
    #[must_use]
    pub const fn streaming(&self) -> bool {
        self.streaming
    }

    /// Returns mixer channels in deterministic display order.
    pub fn mixer_channels(&self) -> impl Iterator<Item = &MixerChannel> {
        self.mixer_channels.values()
    }

    /// Publishes the peak a live capture backend measured for one channel.
    ///
    /// The desktop mixes audio inside its output worker rather than through
    /// [`DesktopState::mix_audio`], so the meter a user watches is fed from the
    /// backend's own measurement instead of being left at zero.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::UnknownMixerChannel`] for an unknown channel ID.
    pub fn set_channel_peak_milli(&mut self, id: &str, peak_milli: u16) -> Result<(), UiError> {
        self.mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?
            .peak_milli = peak_milli.min(1_000);
        Ok(())
    }

    /// Publishes the complete bounded meter state for one live channel.
    ///
    /// Capture and engine adapters call this with telemetry already measured
    /// on their real-time path; the UI only stores and presents the values.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::UnknownMixerChannel`] for an unknown channel ID.
    pub fn set_channel_meter(
        &mut self,
        id: &str,
        peak_milli: u16,
        peak_hold_milli: u16,
        clipped: bool,
    ) -> Result<(), UiError> {
        let channel = self
            .mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        channel.peak_milli = peak_milli.min(1_000);
        channel.peak_hold_milli = peak_hold_milli.min(1_000);
        channel.clipped = clipped;
        Ok(())
    }

    /// Renames a mixer channel so it can show the device it is capturing.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::UnknownMixerChannel`] for an unknown channel ID.
    pub fn set_channel_name(&mut self, id: &str, name: &str) -> Result<(), UiError> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        let channel = self
            .mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        if channel.name != name {
            name.clone_into(&mut channel.name);
        }
        Ok(())
    }

    /// Returns the mixer's current sample rate and channel count.
    #[must_use]
    pub const fn audio_format(&self) -> AudioFormat {
        self.audio_mixer.format()
    }

    /// Mixes one UI-labeled set of audio inputs and updates the visible peak
    /// meters from the real mixer result.
    ///
    /// The frontend supplies stable channel IDs rather than audio-internal IDs;
    /// unknown labels are rejected before the mixer is called.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::UnknownMixerChannel`] or an audio validation error.
    pub fn mix_audio(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
        inputs: &[(&str, &AudioBuffer)],
    ) -> Result<AudioBuffer, UiError> {
        let resolved = inputs
            .iter()
            .map(|(id, buffer)| {
                let source = *self
                    .mixer_sources
                    .get(*id)
                    .ok_or_else(|| UiError::UnknownMixerChannel((*id).to_owned()))?;
                Ok((source, *buffer))
            })
            .collect::<Result<Vec<_>, UiError>>()?;
        let output = self.audio_mixer.mix(timestamp, frames, &resolved)?;
        for (id, source) in &self.mixer_sources {
            let peak = self.audio_mixer.source_peak_milli(*source)?;
            let peak_hold = self.audio_mixer.source_peak_hold_milli(*source)?;
            let clipped = self.audio_mixer.source_clipped(*source)?;
            if let Some(channel) = self.mixer_channels.get_mut(id) {
                channel.peak_milli = peak.min(1_000);
                channel.peak_hold_milli = peak_hold.min(1_000);
                channel.clipped = clipped;
            }
        }
        Ok(output)
    }

    /// Returns a bounded snapshot of notices from oldest to newest.
    pub fn notices(&self) -> impl Iterator<Item = &UiNotice> {
        self.notices.iter()
    }

    /// Returns the action bound to a shortcut.
    #[must_use]
    pub fn shortcut_action(&self, shortcut: &Shortcut) -> Option<UiAction> {
        self.shortcuts.get(shortcut).copied()
    }
}

impl DesktopState {
    fn paste_source(
        &mut self,
        mode: SceneItemDuplicateMode,
        target: &str,
    ) -> Result<&'static str, UiError> {
        let item = self
            .clipboard
            .clone()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "clipboard",
                id: "none".to_owned(),
            })?;
        let profile = self.project.project().active_profile().to_string();
        let scene = self
            .preview_scene
            .as_ref()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: "none".to_owned(),
            })?
            .to_string();
        let group_path = self.paste_group_path(&scene, target)?;
        let paste_at_root = group_path.is_empty();
        let before = self
            .project
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene.as_str()))
            .map(|scene| {
                scene
                    .items()
                    .iter()
                    .map(|item| item.id().clone())
                    .collect::<HashSet<_>>()
            })
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: scene.clone(),
            })?;
        if paste_at_root {
            self.project.dispatch(ProjectCommand::PasteSceneItem {
                profile,
                scene: scene.clone(),
                item,
                mode,
            })?;
        } else {
            self.project.dispatch(ProjectCommand::PasteGroupItem {
                profile,
                scene: scene.clone(),
                group_path,
                item,
                mode,
            })?;
        }
        if paste_at_root {
            // The root paste path has a stable top-level selection affordance;
            // nested rows are deliberately not part of the canvas selection
            // model yet, so leave that selection untouched.
            let pasted_id = self
                .project
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene.as_str()))
                .and_then(|scene| {
                    scene
                        .items()
                        .iter()
                        .find(|item| !before.contains(item.id()))
                })
                .map(|item| item.id().clone());
            let pasted_id = pasted_id.ok_or_else(|| UiError::UnknownSelection {
                kind: "pasted source",
                id: scene.clone(),
            })?;
            self.selected_sources.clear();
            self.selected_sources.push(pasted_id);
        }
        self.sync_selections_after_project_update();
        Ok(match mode {
            SceneItemDuplicateMode::Reference => "source reference pasted",
            SceneItemDuplicateMode::DuplicateSource => "source duplicate pasted",
        })
    }

    fn paste_group_path(&self, scene_id: &str, target: &str) -> Result<Vec<String>, UiError> {
        if target.is_empty() {
            return Ok(Vec::new());
        }
        let scene = self
            .project
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene_id))
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: scene_id.to_owned(),
            })?;
        let parts = target_parts(target).ok_or_else(|| UiError::UnknownSelection {
            kind: "scene item",
            id: target.to_owned(),
        })?;
        let item = scene_item_at_parts(scene, &parts).ok_or_else(|| UiError::UnknownSelection {
            kind: "scene item",
            id: target.to_owned(),
        })?;
        let end = if item.is_group() {
            parts.len()
        } else {
            parts.len().saturating_sub(1)
        };
        Ok(parts[..end].iter().map(|part| (*part).to_owned()).collect())
    }

    fn select_one_source(&mut self, id: &str) -> Result<(), UiError> {
        self.ensure_source(id)?;
        self.selected_sources.clear();
        self.selected_sources.push(identifier(id, "source")?);
        Ok(())
    }

    fn toggle_source_selection(&mut self, id: &str) -> Result<(), UiError> {
        self.ensure_source(id)?;
        if let Some(index) = self
            .selected_sources
            .iter()
            .position(|selected| selected.as_str() == id)
        {
            self.selected_sources.remove(index);
        } else if self.selected_sources.len() < crate::MAX_CANVAS_SELECTIONS {
            self.selected_sources.push(identifier(id, "source")?);
        }
        Ok(())
    }

    fn select_sources(&mut self, ids: Vec<String>, additive: bool) -> Result<(), UiError> {
        let mut validated = Vec::with_capacity(ids.len().min(crate::MAX_CANVAS_SELECTIONS));
        for id in ids.into_iter().take(crate::MAX_CANVAS_SELECTIONS) {
            self.ensure_source(&id)?;
            let id = identifier(&id, "source")?;
            if !validated.contains(&id) {
                validated.push(id);
            }
        }
        let mut next = if additive {
            self.selected_sources.clone()
        } else {
            Vec::with_capacity(validated.len())
        };
        for id in validated {
            if !next.contains(&id) && next.len() < crate::MAX_CANVAS_SELECTIONS {
                next.push(id);
            }
        }
        self.selected_sources = next;
        Ok(())
    }
}

const MAX_SCENE_ITEM_PATH_DEPTH: usize = 64;

fn target_parts(target: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::with_capacity(4);
    for part in target.split('/') {
        if part.is_empty() || parts.len() >= MAX_SCENE_ITEM_PATH_DEPTH {
            return None;
        }
        parts.push(part);
    }
    (!parts.is_empty()).then_some(parts)
}

fn scene_item_at_target<'a>(scene: &'a SceneSpec, target: &str) -> Option<&'a SceneItemSpec> {
    let parts = target_parts(target)?;
    scene_item_at_parts(scene, &parts)
}

fn scene_item_at_parts<'a>(scene: &'a SceneSpec, parts: &[&str]) -> Option<&'a SceneItemSpec> {
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
