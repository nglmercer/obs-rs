//! Toolkit-neutral desktop state and commands for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
};

use obs_rs_media::{FrameTransition, MediaError};
use obs_rs_project::{Project, ProjectCommand, ProjectError, ProjectSession};
use obs_rs_util::Identifier;

/// Maximum number of notices retained for the UI and diagnostics panel.
pub const MAX_UI_NOTICES: usize = 256;
/// Maximum UTF-8 byte length of a shortcut key name.
pub const MAX_SHORTCUT_KEY_BYTES: usize = 32;

/// Which scene view a desktop surface is showing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneView {
    /// The editable/queued scene.
    Preview,
    /// The scene currently sent to program output.
    Program,
}

/// A user action that can be bound to a keyboard shortcut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAction {
    /// Swap the selected preview and program scenes.
    SwapPreviewProgram,
    /// Begin a recording output.
    StartRecording,
    /// Stop a recording output.
    StopRecording,
    /// Begin a streaming output.
    StartStreaming,
    /// Stop a streaming output.
    StopStreaming,
}

/// A validated, sortable keyboard shortcut description.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Shortcut {
    modifiers: u8,
    key: String,
}

impl Shortcut {
    /// Creates a shortcut from a modifier bitset and key name.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::InvalidShortcut`] for an empty or oversized key.
    pub fn new(modifiers: u8, key: &str) -> Result<Self, UiError> {
        let key = key.trim();
        if key.is_empty() || key.len() > MAX_SHORTCUT_KEY_BYTES {
            return Err(UiError::InvalidShortcut);
        }
        Ok(Self {
            modifiers,
            key: key.to_owned(),
        })
    }

    /// Returns the modifier bitset chosen by the frontend.
    #[must_use]
    pub const fn modifiers(&self) -> u8 {
        self.modifiers
    }

    /// Returns the normalized key name.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// A bounded user-visible diagnostic notice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiNotice {
    sequence: u64,
    message: String,
}

impl UiNotice {
    /// Returns the monotonically increasing notice sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the user-facing notice text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Commands consumed by the toolkit-specific frontend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiCommand {
    /// Apply one validated project mutation.
    Project(ProjectCommand),
    /// Select an active profile.
    SelectProfile { id: String },
    /// Select the scene shown in preview.
    SelectPreviewScene { id: String },
    /// Select the scene sent to program output.
    SelectProgramScene { id: String },
    /// Replace the current scene transition policy.
    SetTransition { transition: FrameTransition },
    /// Begin recording.
    StartRecording,
    /// Stop recording.
    StopRecording,
    /// Begin streaming.
    StartStreaming,
    /// Stop streaming.
    StopStreaming,
    /// Bind an action to a shortcut.
    BindShortcut {
        shortcut: Shortcut,
        action: UiAction,
    },
    /// Remove a shortcut binding.
    UnbindShortcut { shortcut: Shortcut },
    /// Execute a previously bound action.
    TriggerShortcut { shortcut: Shortcut },
}

/// Errors from the toolkit-neutral application state.
#[derive(Debug, Eq, PartialEq)]
pub enum UiError {
    /// Project command validation failed.
    Project(ProjectError),
    /// A requested profile or scene is not in the current project.
    UnknownSelection { kind: &'static str, id: String },
    /// A shortcut key is empty or too long.
    InvalidShortcut,
    /// A shortcut is not currently bound.
    UnknownShortcut(Shortcut),
    /// A shortcut is already bound and must be explicitly replaced.
    DuplicateShortcut(Shortcut),
    /// Recording is already active.
    RecordingAlreadyActive,
    /// Recording is not active.
    RecordingNotActive,
    /// Streaming is already active.
    StreamingAlreadyActive,
    /// Streaming is not active.
    StreamingNotActive,
    /// The scene transition is invalid.
    Media(MediaError),
    /// The notice sequence counter overflowed.
    NoticeSequenceExhausted,
}

impl fmt::Display for UiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(error) => error.fmt(formatter),
            Self::UnknownSelection { kind, id } => {
                write!(formatter, "unknown {kind} selection {id}")
            }
            Self::InvalidShortcut => formatter.write_str("shortcut key is empty or too long"),
            Self::UnknownShortcut(shortcut) => {
                write!(formatter, "shortcut {} is not bound", shortcut.key())
            }
            Self::DuplicateShortcut(shortcut) => {
                write!(formatter, "shortcut {} is already bound", shortcut.key())
            }
            Self::RecordingAlreadyActive => formatter.write_str("recording is already active"),
            Self::RecordingNotActive => formatter.write_str("recording is not active"),
            Self::StreamingAlreadyActive => formatter.write_str("streaming is already active"),
            Self::StreamingNotActive => formatter.write_str("streaming is not active"),
            Self::Media(error) => error.fmt(formatter),
            Self::NoticeSequenceExhausted => formatter.write_str("UI notice sequence is exhausted"),
        }
    }
}

impl std::error::Error for UiError {}

impl From<ProjectError> for UiError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

/// Rust-owned state shared by preview, program, and desktop control surfaces.
pub struct DesktopState {
    project: ProjectSession,
    preview_scene: Option<Identifier>,
    program_scene: Option<Identifier>,
    transition: FrameTransition,
    recording: bool,
    streaming: bool,
    shortcuts: BTreeMap<Shortcut, UiAction>,
    notices: VecDeque<UiNotice>,
    next_notice_sequence: u64,
}

impl DesktopState {
    /// Creates a clean desktop state and selects the first scene for both views.
    #[must_use]
    pub fn new(project: Project) -> Self {
        let session = ProjectSession::new(project);
        let first_scene = first_scene_id(session.project());
        Self {
            project: session,
            preview_scene: first_scene.clone(),
            program_scene: first_scene,
            transition: FrameTransition::Cut,
            recording: false,
            streaming: false,
            shortcuts: BTreeMap::new(),
            notices: VecDeque::new(),
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
            UiCommand::Project(command) => {
                self.project.dispatch(command)?;
                "project updated"
            }
            UiCommand::SelectProfile { id } => {
                self.project
                    .dispatch(ProjectCommand::SetActiveProfile { id })?;
                self.preview_scene = first_scene_id(self.project.project());
                self.program_scene = self.preview_scene.clone();
                "profile selected"
            }
            UiCommand::SelectPreviewScene { id } => {
                self.ensure_scene(&id)?;
                self.preview_scene = Some(identifier(&id, "scene")?);
                "preview scene selected"
            }
            UiCommand::SelectProgramScene { id } => {
                self.ensure_scene(&id)?;
                self.program_scene = Some(identifier(&id, "scene")?);
                "program scene selected"
            }
            UiCommand::SetTransition { transition } => {
                if let FrameTransition::CrossFade { progress_milli } = transition {
                    FrameTransition::cross_fade(progress_milli).map_err(UiError::Media)?;
                }
                self.transition = transition;
                "transition updated"
            }
            UiCommand::StartRecording => {
                if self.recording {
                    return Err(UiError::RecordingAlreadyActive);
                }
                self.recording = true;
                "recording started"
            }
            UiCommand::StopRecording => {
                if !self.recording {
                    return Err(UiError::RecordingNotActive);
                }
                self.recording = false;
                "recording stopped"
            }
            UiCommand::StartStreaming => {
                if self.streaming {
                    return Err(UiError::StreamingAlreadyActive);
                }
                self.streaming = true;
                "streaming started"
            }
            UiCommand::StopStreaming => {
                if !self.streaming {
                    return Err(UiError::StreamingNotActive);
                }
                self.streaming = false;
                "streaming stopped"
            }
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

    /// Returns a bounded snapshot of notices from oldest to newest.
    pub fn notices(&self) -> impl Iterator<Item = &UiNotice> {
        self.notices.iter()
    }

    /// Returns the action bound to a shortcut.
    #[must_use]
    pub fn shortcut_action(&self, shortcut: &Shortcut) -> Option<UiAction> {
        self.shortcuts.get(shortcut).copied()
    }

    fn ensure_scene(&self, id: &str) -> Result<(), UiError> {
        let profile = self
            .project
            .project()
            .profiles()
            .find(|profile| profile.id() == self.project.project().active_profile())
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "profile",
                id: self.project.project().active_profile().to_string(),
            })?;
        if profile.scenes().any(|scene| scene.id().as_str() == id) {
            Ok(())
        } else {
            Err(UiError::UnknownSelection {
                kind: "scene",
                id: id.to_owned(),
            })
        }
    }

    fn dispatch_action(&mut self, action: UiAction) -> Result<(), UiError> {
        match action {
            UiAction::SwapPreviewProgram => {
                std::mem::swap(&mut self.preview_scene, &mut self.program_scene);
                Ok(())
            }
            UiAction::StartRecording => self.dispatch(UiCommand::StartRecording),
            UiAction::StopRecording => self.dispatch(UiCommand::StopRecording),
            UiAction::StartStreaming => self.dispatch(UiCommand::StartStreaming),
            UiAction::StopStreaming => self.dispatch(UiCommand::StopStreaming),
        }
    }

    fn notice(&mut self, message: &str) -> Result<(), UiError> {
        let sequence = self.next_notice_sequence;
        self.next_notice_sequence = self
            .next_notice_sequence
            .checked_add(1)
            .ok_or(UiError::NoticeSequenceExhausted)?;
        if self.notices.len() == MAX_UI_NOTICES {
            let _ = self.notices.pop_front();
        }
        self.notices.push_back(UiNotice {
            sequence,
            message: message.to_owned(),
        });
        Ok(())
    }
}

fn first_scene_id(project: &Project) -> Option<Identifier> {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().next())
        .map(|scene| scene.id().clone())
}

fn identifier(input: &str, kind: &'static str) -> Result<Identifier, UiError> {
    Identifier::new(input).map_err(|_| UiError::UnknownSelection {
        kind,
        id: input.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_config::Config;
    use obs_rs_media::{FrameRate, VideoFormat};
    use obs_rs_project::{Profile, SceneSpec, SourceSpec};

    fn project() -> Project {
        let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
        let mut project = Project::new("UI fixture").expect("project");
        let mut profile = Profile::new("live", "Live", format).expect("profile");
        profile
            .add_scene(SceneSpec::new("preview", "Preview").expect("scene"))
            .expect("scene");
        profile
            .add_scene(SceneSpec::new("program", "Program").expect("scene"))
            .expect("scene");
        let mut source_scene = SceneSpec::new("source_scene", "Source").expect("scene");
        source_scene
            .add_source(
                SourceSpec::new("source", "color_source", "Color", Config::new()).expect("source"),
            )
            .expect("source");
        profile.add_scene(source_scene).expect("scene");
        project.add_profile(profile).expect("profile");
        project
    }

    #[test]
    fn desktop_state_selects_scenes_and_tracks_outputs() {
        let mut state = DesktopState::new(project());
        assert_eq!(state.preview_scene(), Some("preview"));
        assert_eq!(state.program_scene(), Some("preview"));
        state
            .dispatch(UiCommand::SelectProgramScene {
                id: "program".to_owned(),
            })
            .expect("program selection");
        state
            .dispatch(UiCommand::StartRecording)
            .expect("recording start");
        assert!(state.recording());
        assert!(!state.is_dirty());
        assert_eq!(state.notices().count(), 2);
    }

    #[test]
    fn shortcuts_trigger_actions_and_reject_duplicates() {
        let mut state = DesktopState::new(project());
        let shortcut = Shortcut::new(1, "F9").expect("shortcut");
        state
            .dispatch(UiCommand::BindShortcut {
                shortcut: shortcut.clone(),
                action: UiAction::StartStreaming,
            })
            .expect("bind");
        assert_eq!(
            state.shortcut_action(&shortcut),
            Some(UiAction::StartStreaming)
        );
        assert_eq!(
            state.dispatch(UiCommand::BindShortcut {
                shortcut: shortcut.clone(),
                action: UiAction::StopStreaming,
            }),
            Err(UiError::DuplicateShortcut(shortcut.clone()))
        );
        state
            .dispatch(UiCommand::TriggerShortcut { shortcut })
            .expect("trigger");
        assert!(state.streaming());
    }

    #[test]
    fn project_commands_keep_dirty_state_and_transitions_validate() {
        let mut state = DesktopState::new(project());
        state
            .dispatch(UiCommand::Project(ProjectCommand::SetActiveProfile {
                id: "live".to_owned(),
            }))
            .expect("project command");
        assert!(state.is_dirty());
        state
            .dispatch(UiCommand::SetTransition {
                transition: FrameTransition::cross_fade(500).expect("transition"),
            })
            .expect("transition");
        assert_eq!(
            state.transition(),
            FrameTransition::CrossFade {
                progress_milli: 500
            }
        );
        assert_eq!(
            state.dispatch(UiCommand::SetTransition {
                transition: FrameTransition::CrossFade {
                    progress_milli: 1_001
                },
            }),
            Err(UiError::Media(MediaError::InvalidTransition {
                progress_milli: 1_001
            }))
        );
    }
}
