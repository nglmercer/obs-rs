use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use obs_rs_audio::{AudioBuffer, AudioFormat, AudioMixer, AudioSourceId};
use obs_rs_media::{FrameTransition, Timestamp};
use obs_rs_project::{Project, ProjectCommand, ProjectFileStore, ProjectSession, SceneItemSpec};
use obs_rs_util::Identifier;

use super::{
    error::UiError,
    helpers::{default_mixer, first_scene_id, first_source_id, identifier, project_has_scene},
    types::{
        MixerChannel, ProjectSceneSelection, Shortcut, UiAction, UiCommand, UiLocale, UiNotice,
    },
    MAX_SHORTCUT_BINDINGS, MAX_UI_NOTICES,
};

mod selection;
use super::helpers::profile_scene_item_at_target;

/// The last Preview/Program choices for one in-memory profile.
#[derive(Clone, Debug, Default)]
struct SceneSelection {
    preview: Option<Identifier>,
    program: Option<Identifier>,
}

/// The scene choices associated with one loaded project document.
#[derive(Clone, Debug, Default)]
struct ProjectSceneSelectionState {
    selections: BTreeMap<Identifier, SceneSelection>,
}

pub(crate) struct ActiveTransition {
    pub(crate) source_scene: Identifier,
    pub(crate) destination_scene: Identifier,
    pub(crate) transition: FrameTransition,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
}

/// The selection cache is session state, not project data. Keep it bounded so
/// repeatedly opening profiles cannot grow the UI state without limit.
const MAX_PROFILE_SCENE_SELECTIONS: usize = 64;
/// Keep collection-switch keys bounded even when a frontend supplies a bad path.
const MAX_PROJECT_SELECTION_KEY_BYTES: usize = 4_096;
/// A studio session can visit many collections, but never needs an unbounded
/// history of their last visible scenes.
const MAX_PROJECT_SCENE_SELECTIONS: usize = 32;
/// Keep per-profile history bounded inside each remembered collection.
const MAX_PROJECT_PROFILE_SCENE_SELECTIONS: usize = 64;
/// Bound the flattened frontend snapshot list independently of its nested
/// document/profile dimensions.
const MAX_PROJECT_SELECTION_RECORDS: usize = 256;

pub struct DesktopState {
    pub(crate) project: ProjectSession,
    pub(crate) preview_scene: Option<Identifier>,
    pub(crate) program_scene: Option<Identifier>,
    profile_scene_selections: BTreeMap<Identifier, SceneSelection>,
    project_selection_key: Option<String>,
    project_scene_selections: BTreeMap<String, ProjectSceneSelectionState>,
    /// Ordered transient scene-item selection. The last item is the active
    /// item used by property dialogs and single-row source actions. Nested
    /// group rows use their bounded outer-to-inner path.
    pub(crate) selected_sources: Vec<String>,
    /// Transient scene-item clipboard. It is intentionally outside the
    /// persistent project and is cleared when a different project is loaded.
    pub(crate) clipboard: Option<SceneItemSpec>,
    pub(crate) locale: UiLocale,
    pub(crate) transition: FrameTransition,
    pub(crate) active_transition: Option<ActiveTransition>,
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
            project_selection_key: None,
            project_scene_selections: BTreeMap::new(),
            selected_sources: selected_sources
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            clipboard: None,
            locale: UiLocale::English,
            transition: FrameTransition::Cut,
            active_transition: None,
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
                self.active_transition = None;
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
                self.active_transition = None;
                "project updated"
            }
            UiCommand::Undo => {
                let message = self.step_history(true);
                self.active_transition = None;
                message
            }
            UiCommand::Redo => {
                let message = self.step_history(false);
                self.active_transition = None;
                message
            }
            UiCommand::SelectProfile { id } => {
                let previous_profile = self.project.project().active_profile().clone();
                let previous_selection = self.scene_selection();
                self.project
                    .dispatch(ProjectCommand::SetActiveProfile { id })?;
                self.remember_profile_selection(previous_profile, previous_selection);
                self.restore_active_profile_selection();
                self.active_transition = None;
                "profile selected"
            }
            UiCommand::SelectPreviewScene { id } => {
                self.ensure_scene(&id)?;
                self.preview_scene = Some(identifier(&id, "scene")?);
                self.active_transition = None;
                self.selected_sources = self
                    .preview_scene
                    .as_ref()
                    .and_then(|scene| first_source_id(self.project.project(), scene))
                    .map(|id| id.to_string())
                    .into_iter()
                    .collect();
                "preview scene selected"
            }
            UiCommand::SelectProgramScene { id } => {
                self.ensure_scene(&id)?;
                self.program_scene = Some(identifier(&id, "scene")?);
                self.active_transition = None;
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
                    .and_then(|profile| {
                        profile_scene_item_at_target(profile, preview_scene.as_str(), &id)
                    })
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
            UiCommand::TakePreview {
                transition,
                duration_ms,
            } => self.take_preview(transition, duration_ms)?,
            UiCommand::SetPreviewSceneTransition { transition } => {
                let message = self.set_preview_scene_transition(transition)?;
                self.active_transition = None;
                message
            }
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
                if self.shortcuts.len() >= MAX_SHORTCUT_BINDINGS {
                    return Err(UiError::TooManyShortcuts);
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
        self.project_selection_key = None;
        self.project_scene_selections.clear();
        self.replace_project(project);
        self.notice("project loaded")?;
        Ok(())
    }

    /// Loads a project while retaining its session-scoped Preview/Program
    /// choices under `selection_key`.
    ///
    /// The key is normally the active project path. It is deliberately kept
    /// outside the project document so exchanging the file cannot change
    /// project history.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::Project`] when the file cannot be read or parsed.
    pub fn load_project_for_key(
        &mut self,
        store: &ProjectFileStore,
        selection_key: &str,
    ) -> Result<(), UiError> {
        let project = store.load()?;
        self.remember_project_selection();
        self.replace_project(project);
        self.project_selection_key = project_selection_key(selection_key);
        self.restore_project_selection();
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
        self.project_selection_key = None;
        self.project_scene_selections.clear();
        self.replace_project(project);
        self.project.mark_dirty();
        self.notice("project recovered from interrupted save")?;
        Ok(true)
    }

    /// Recovers a project and restores the last session selection for its key.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::Project`] when the recovery artifact cannot be read or
    /// parsed.
    pub fn recover_project_for_key(
        &mut self,
        store: &ProjectFileStore,
        selection_key: &str,
    ) -> Result<bool, UiError> {
        let Some(project) = store.recover()? else {
            return Ok(false);
        };
        self.remember_project_selection();
        self.replace_project(project);
        self.project_selection_key = project_selection_key(selection_key);
        self.restore_project_selection();
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
        self.active_transition = None;
        self.profile_scene_selections.clear();
        let first_scene = first_scene_id(self.project.project());
        self.preview_scene.clone_from(&first_scene);
        self.program_scene.clone_from(&first_scene);
        self.selected_sources = self
            .preview_scene
            .as_ref()
            .and_then(|scene| first_source_id(self.project.project(), scene))
            .map(|id| id.to_string())
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
        self.active_transition = None;
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
                .map(|id| id.to_string())
                .into_iter()
                .collect();
        }
        if let Some(program) = program {
            self.program_scene = Some(program);
        }
    }

    /// Sets the current document key without changing the loaded project or
    /// scene selection. A later keyed load uses this key to remember the
    /// current selection before replacing the project.
    pub fn set_project_selection_key(&mut self, selection_key: &str) {
        let selection_key = project_selection_key(selection_key);
        if self.project_selection_key == selection_key {
            return;
        }
        self.remember_project_selection();
        self.project_selection_key = selection_key;
    }

    /// Replaces the bounded document-selection cache from a frontend's
    /// persisted session settings.
    ///
    /// Invalid keys, profiles, and scene IDs are ignored. The active project
    /// is not changed; a keyed load or
    /// [`Self::restore_project_selection_for_current_key`] applies a valid
    /// matching record after the project is available.
    pub fn restore_project_selections(&mut self, selections: &[ProjectSceneSelection]) {
        self.project_scene_selections.clear();
        for selection in selections.iter().take(MAX_PROJECT_SELECTION_RECORDS) {
            let Some(key) = project_selection_key(selection.key()) else {
                continue;
            };
            let Ok(profile) = Identifier::new(selection.profile()) else {
                continue;
            };
            if self.project_scene_selections.len() == MAX_PROJECT_SCENE_SELECTIONS
                && !self.project_scene_selections.contains_key(&key)
            {
                if let Some(oldest) = self.project_scene_selections.keys().next().cloned() {
                    self.project_scene_selections.remove(&oldest);
                }
            }
            let document = self.project_scene_selections.entry(key).or_default();
            if document.selections.len() == MAX_PROJECT_PROFILE_SCENE_SELECTIONS
                && !document.selections.contains_key(&profile)
            {
                if let Some(oldest) = document.selections.keys().next().cloned() {
                    document.selections.remove(&oldest);
                }
            }
            document.selections.insert(
                profile,
                SceneSelection {
                    preview: selection.preview().and_then(|id| Identifier::new(id).ok()),
                    program: selection.program().and_then(|id| Identifier::new(id).ok()),
                },
            );
        }
    }

    /// Returns bounded document/profile-selection snapshots, including all
    /// visited profiles for the active document and its current choices even
    /// when it has not been switched away.
    #[must_use]
    pub fn project_scene_selections(&self) -> Vec<ProjectSceneSelection> {
        let mut selections = BTreeMap::new();
        let mut insert = |key: &str, profile: &Identifier, selection: &SceneSelection| {
            let profile_text = profile.to_string();
            let record_key = (key.to_owned(), profile_text.clone());
            if selections.len() == MAX_PROJECT_SELECTION_RECORDS
                && !selections.contains_key(&record_key)
            {
                return;
            }
            selections.insert(
                record_key,
                ProjectSceneSelection::new(
                    key,
                    profile_text,
                    selection.preview.as_ref().map(ToString::to_string),
                    selection.program.as_ref().map(ToString::to_string),
                ),
            );
        };
        if let Some(key) = self.project_selection_key.as_ref() {
            if let Some(saved) = self.project_scene_selections.get(key) {
                for (profile, selection) in &saved.selections {
                    insert(key, profile, selection);
                }
            }
            for (profile, selection) in &self.profile_scene_selections {
                insert(key, profile, selection);
            }
            insert(
                key,
                self.project.project().active_profile(),
                &self.scene_selection(),
            );
        }
        for (key, saved) in &self.project_scene_selections {
            if self.project_selection_key.as_deref() == Some(key.as_str()) {
                continue;
            }
            for (profile, selection) in &saved.selections {
                insert(key, profile, selection);
            }
        }
        selections.into_values().collect()
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
            .map(|id| id.to_string())
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

    fn remember_project_selection(&mut self) {
        let Some(key) = self.project_selection_key.as_ref() else {
            return;
        };
        let profile = self.project.project().active_profile().clone();
        let selection = self.scene_selection();
        if self.project_scene_selections.len() == MAX_PROJECT_SCENE_SELECTIONS
            && !self.project_scene_selections.contains_key(key)
        {
            if let Some(oldest) = self.project_scene_selections.keys().next().cloned() {
                self.project_scene_selections.remove(&oldest);
            }
        }
        let document = self
            .project_scene_selections
            .entry(key.clone())
            .or_default();
        if document.selections.len() == MAX_PROJECT_PROFILE_SCENE_SELECTIONS
            && !document.selections.contains_key(&profile)
        {
            if let Some(oldest) = document.selections.keys().next().cloned() {
                document.selections.remove(&oldest);
            }
        }
        document.selections.insert(profile, selection);
    }

    fn restore_project_selection(&mut self) {
        let Some(key) = self.project_selection_key.as_ref() else {
            return;
        };
        let Some(saved) = self.project_scene_selections.get(key).cloned() else {
            return;
        };
        self.profile_scene_selections.clear();
        for (profile, selection) in saved.selections {
            if self.project.project().profile(&profile).is_some() {
                self.profile_scene_selections.insert(profile, selection);
            }
        }
        self.restore_active_profile_selection();
    }

    /// Applies all persisted profile selections for the currently loaded
    /// document. Invalid or stale profiles/scenes keep their first-scene
    /// fallback and do not create project history entries.
    pub fn restore_project_selection_for_current_key(&mut self) {
        self.restore_project_selection();
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
        self.selected_sources.last().map(String::as_str)
    }

    /// Returns the current ordered canvas selection.
    pub fn selected_sources(&self) -> impl Iterator<Item = &str> {
        self.selected_sources.iter().map(String::as_str)
    }

    /// Returns whether a scene item belongs to the current canvas selection.
    #[must_use]
    pub fn is_source_selected(&self, id: &str) -> bool {
        self.selected_sources.iter().any(|selected| selected == id)
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

    /// Replaces the complete bounded shortcut table atomically.
    ///
    /// Settings Apply uses this rather than unbinding and rebinding one entry
    /// at a time, so a failed validation never leaves a half-updated runtime
    /// map. Duplicate and oversized input is rejected before the current map
    /// is touched.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::DuplicateShortcut`] for duplicate keys or
    /// [`UiError::TooManyShortcuts`] when the bounded table would overflow.
    pub fn replace_shortcuts(&mut self, bindings: &[(Shortcut, UiAction)]) -> Result<(), UiError> {
        if bindings.len() > MAX_SHORTCUT_BINDINGS {
            return Err(UiError::TooManyShortcuts);
        }
        let mut next = BTreeMap::new();
        for (shortcut, action) in bindings {
            if next.insert(shortcut.clone(), *action).is_some() {
                return Err(UiError::DuplicateShortcut(shortcut.clone()));
            }
        }
        self.shortcuts = next;
        Ok(())
    }
}

fn project_selection_key(selection_key: &str) -> Option<String> {
    let selection_key = selection_key.trim();
    if selection_key.is_empty() || selection_key.len() > MAX_PROJECT_SELECTION_KEY_BYTES {
        return None;
    }
    Some(selection_key.to_owned())
}
