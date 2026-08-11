use std::collections::{BTreeMap, VecDeque};

use obs_rs_audio::{AudioBuffer, AudioFormat, AudioMixer, AudioSourceId};
use obs_rs_media::{FrameTransition, Timestamp};
use obs_rs_project::{Project, ProjectCommand, ProjectFileStore, ProjectSession};
use obs_rs_util::Identifier;

use super::{
    error::UiError,
    helpers::{default_mixer, first_scene_id, first_source_id, identifier},
    types::{MixerChannel, Shortcut, UiAction, UiCommand, UiLocale, UiNotice},
    MAX_UI_NOTICES,
};

pub struct DesktopState {
    pub(crate) project: ProjectSession,
    pub(crate) preview_scene: Option<Identifier>,
    pub(crate) program_scene: Option<Identifier>,
    pub(crate) selected_source: Option<Identifier>,
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
        let selected_source = first_scene
            .as_ref()
            .and_then(|scene| first_source_id(session.project(), scene));
        let (audio_mixer, mixer_sources, mixer_channels) = default_mixer();
        Self {
            project: session,
            preview_scene: first_scene.clone(),
            program_scene: first_scene,
            selected_source,
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
    pub fn dispatch(&mut self, command: UiCommand) -> Result<(), UiError> {
        let message = match command {
            UiCommand::SwapPreviewProgram => {
                std::mem::swap(&mut self.preview_scene, &mut self.program_scene);
                "preview and program scenes swapped"
            }
            UiCommand::Project(command) => {
                self.project.dispatch(command)?;
                self.sync_selections_after_project_update();
                "project updated"
            }
            UiCommand::SelectProfile { id } => {
                self.project
                    .dispatch(ProjectCommand::SetActiveProfile { id })?;
                self.preview_scene = first_scene_id(self.project.project());
                self.program_scene = self.preview_scene.clone();
                self.selected_source = self
                    .preview_scene
                    .as_ref()
                    .and_then(|scene| first_source_id(self.project.project(), scene));
                "profile selected"
            }
            UiCommand::SelectPreviewScene { id } => {
                self.ensure_scene(&id)?;
                self.preview_scene = Some(identifier(&id, "scene")?);
                self.selected_source = self
                    .preview_scene
                    .as_ref()
                    .and_then(|scene| first_source_id(self.project.project(), scene));
                "preview scene selected"
            }
            UiCommand::SelectProgramScene { id } => {
                self.ensure_scene(&id)?;
                self.program_scene = Some(identifier(&id, "scene")?);
                "program scene selected"
            }
            UiCommand::SelectSource { id } => {
                self.ensure_source(&id)?;
                self.selected_source = Some(identifier(&id, "source")?);
                "source selected"
            }
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

    fn replace_project(&mut self, project: Project) {
        self.project.replace(project);
        let first_scene = first_scene_id(self.project.project());
        self.preview_scene.clone_from(&first_scene);
        self.program_scene.clone_from(&first_scene);
        self.selected_source = self
            .preview_scene
            .as_ref()
            .and_then(|scene| first_source_id(self.project.project(), scene));
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
        self.selected_source.as_ref().map(Identifier::as_str)
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
        for channel in self.mixer_channels.values_mut() {
            channel.peak_milli = 0;
        }
        for (id, _) in inputs {
            let source = self
                .mixer_sources
                .get(*id)
                .ok_or_else(|| UiError::UnknownMixerChannel((*id).to_owned()))?;
            let peak = self.audio_mixer.source_peak_milli(*source)?;
            if let Some(channel) = self.mixer_channels.get_mut(*id) {
                channel.peak_milli = peak;
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
