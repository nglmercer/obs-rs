//! Toolkit-neutral desktop state and commands for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::{BTreeMap, VecDeque},
    fmt::{self, Write as _},
};

use obs_rs_audio::{AudioBuffer, AudioError, AudioFormat, AudioMixer, AudioSourceId};
use obs_rs_media::{FrameTransition, MediaError, Timestamp};
use obs_rs_project::{Project, ProjectCommand, ProjectError, ProjectFileStore, ProjectSession};
use obs_rs_util::Identifier;

/// Maximum number of notices retained for the UI and diagnostics panel.
pub const MAX_UI_NOTICES: usize = 256;
/// Maximum UTF-8 byte length of a shortcut key name.
pub const MAX_SHORTCUT_KEY_BYTES: usize = 32;
/// Maximum UTF-8 byte length accepted by the terminal command parser.
pub const MAX_CONSOLE_COMMAND_BYTES: usize = 256;
/// Maximum complete HTTP request accepted by the local browser frontend.
pub const MAX_WEB_REQUEST_BYTES: usize = 64 * 1024;

/// Which scene view a desktop surface is showing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneView {
    /// The editable/queued scene.
    Preview,
    /// The scene currently sent to program output.
    Program,
}

/// Supported labels for toolkit-neutral state surfaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLocale {
    /// English labels.
    English,
    /// Spanish labels.
    Spanish,
}

impl UiLocale {
    /// Returns the stable language code used by frontends and project settings.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Spanish => "es",
        }
    }

    /// Parses a supported language code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" | "english" => Some(Self::English),
            "es" | "spanish" => Some(Self::Spanish),
            _ => None,
        }
    }
}

/// One stateful channel shown in the desktop audio mixer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MixerChannel {
    id: String,
    name: String,
    gain_milli: u16,
    muted: bool,
    peak_milli: u16,
}

impl MixerChannel {
    /// Returns the stable UI channel ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the channel display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns linear gain in thousandths, where 1000 is unity.
    #[must_use]
    pub const fn gain_milli(&self) -> u16 {
        self.gain_milli
    }

    /// Returns whether the channel is muted.
    #[must_use]
    pub const fn muted(&self) -> bool {
        self.muted
    }

    /// Returns the latest bounded peak meter value in thousandths.
    #[must_use]
    pub const fn peak_milli(&self) -> u16 {
        self.peak_milli
    }
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
    /// Swap the selected preview and program scenes.
    SwapPreviewProgram,
    /// Apply one validated project mutation.
    Project(ProjectCommand),
    /// Select an active profile.
    SelectProfile { id: String },
    /// Select the scene shown in preview.
    SelectPreviewScene { id: String },
    /// Select the scene sent to program output.
    SelectProgramScene { id: String },
    /// Select a source item from the current preview scene.
    SelectSource { id: String },
    /// Select the labels used by accessible frontends.
    SetLocale { locale: UiLocale },
    /// Replace the current scene transition policy.
    SetTransition { transition: FrameTransition },
    /// Send the selected preview scene to program using one transition.
    TakePreview { transition: FrameTransition },
    /// Set one mixer channel's linear gain in thousandths.
    SetMixerGain { id: String, gain_milli: u16 },
    /// Toggle one mixer channel's mute state.
    ToggleMixerMute { id: String },
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

/// A command understood by the safe terminal frontend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsoleCommand {
    /// Print the current accessible snapshot.
    Show,
    /// Print command help.
    Help,
    /// Apply one desktop-state command.
    Apply(UiCommand),
    /// End the frontend session.
    Quit,
}

/// Errors raised while parsing terminal frontend commands.
#[derive(Debug, Eq, PartialEq)]
pub enum ConsoleCommandError {
    /// The command contained no non-whitespace text.
    Empty,
    /// The command exceeded [`MAX_CONSOLE_COMMAND_BYTES`].
    TooLong,
    /// The first word is not a supported command.
    UnknownCommand(String),
    /// A command did not contain a required argument.
    MissingArgument(&'static str),
    /// A command contained an invalid subcommand or extra argument.
    InvalidArgument {
        command: &'static str,
        value: String,
    },
    /// A fade transition was outside the valid range.
    InvalidTransition(MediaError),
}

impl fmt::Display for ConsoleCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("console command is empty"),
            Self::TooLong => formatter.write_str("console command is too long"),
            Self::UnknownCommand(command) => write!(formatter, "unknown console command {command}"),
            Self::MissingArgument(argument) => {
                write!(formatter, "missing console argument {argument}")
            }
            Self::InvalidArgument { command, value } => {
                write!(formatter, "invalid argument for {command}: {value}")
            }
            Self::InvalidTransition(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ConsoleCommandError {}

/// A route understood by the local browser presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebRoute {
    /// Serve the accessible control page.
    Home,
    /// Return the current labeled state snapshot as plain text.
    Snapshot,
    /// Parse and dispatch one line-oriented desktop command.
    Command(String),
}

/// Errors raised while parsing a bounded local HTTP request.
#[derive(Debug, Eq, PartialEq)]
pub enum WebRequestError {
    /// The request exceeded [`MAX_WEB_REQUEST_BYTES`].
    TooLarge,
    /// The request was not valid UTF-8.
    InvalidUtf8,
    /// The request line or required headers were malformed.
    Malformed,
    /// The HTTP method is not supported by the local frontend.
    UnsupportedMethod(String),
    /// The path is not a supported local frontend route.
    InvalidPath(String),
    /// The request body exceeded [`MAX_CONSOLE_COMMAND_BYTES`].
    BodyTooLong,
    /// The declared body size did not match the received bytes.
    ContentLengthMismatch { expected: usize, actual: usize },
}

impl fmt::Display for WebRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("web request is too large"),
            Self::InvalidUtf8 => formatter.write_str("web request is not valid UTF-8"),
            Self::Malformed => formatter.write_str("web request is malformed"),
            Self::UnsupportedMethod(method) => {
                write!(formatter, "web method {method} is not supported")
            }
            Self::InvalidPath(path) => write!(formatter, "web path {path} is not supported"),
            Self::BodyTooLong => formatter.write_str("web command body is too long"),
            Self::ContentLengthMismatch { expected, actual } => write!(
                formatter,
                "web content length declares {expected} bytes but received {actual}"
            ),
        }
    }
}

impl std::error::Error for WebRequestError {}

/// Parses one line for the terminal frontend without mutating desktop state.
///
/// # Errors
///
/// Returns [`ConsoleCommandError`] when the line is empty, oversized, unknown,
/// missing an argument, or contains an invalid transition/output action.
pub fn parse_console_command(line: &str) -> Result<ConsoleCommand, ConsoleCommandError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(ConsoleCommandError::Empty);
    }
    if line.len() > MAX_CONSOLE_COMMAND_BYTES {
        return Err(ConsoleCommandError::TooLong);
    }

    let mut words = line.split_whitespace();
    let command = words.next().ok_or(ConsoleCommandError::Empty)?;
    match command {
        "help" => ensure_no_extra("help", words).map(|()| ConsoleCommand::Help),
        "show" => ensure_no_extra("show", words).map(|()| ConsoleCommand::Show),
        "snapshot" => ensure_no_extra("snapshot", words).map(|()| ConsoleCommand::Show),
        "quit" => ensure_no_extra("quit", words).map(|()| ConsoleCommand::Quit),
        "exit" => ensure_no_extra("exit", words).map(|()| ConsoleCommand::Quit),
        "swap" => ensure_no_extra("swap", words)
            .map(|()| ConsoleCommand::Apply(UiCommand::SwapPreviewProgram)),
        "preview" => {
            let id = required_word(&mut words, "preview scene")?;
            ensure_no_extra("preview", words)?;
            Ok(ConsoleCommand::Apply(UiCommand::SelectPreviewScene {
                id: id.to_owned(),
            }))
        }
        "program" => {
            let id = required_word(&mut words, "program scene")?;
            ensure_no_extra("program", words)?;
            Ok(ConsoleCommand::Apply(UiCommand::SelectProgramScene {
                id: id.to_owned(),
            }))
        }
        "profile" => {
            let id = required_word(&mut words, "profile")?;
            ensure_no_extra("profile", words)?;
            Ok(ConsoleCommand::Apply(UiCommand::SelectProfile {
                id: id.to_owned(),
            }))
        }
        "language" => {
            let code = required_word(&mut words, "language")?;
            ensure_no_extra("language", words)?;
            let locale =
                UiLocale::from_code(code).ok_or_else(|| ConsoleCommandError::InvalidArgument {
                    command: "language",
                    value: code.to_owned(),
                })?;
            Ok(ConsoleCommand::Apply(UiCommand::SetLocale { locale }))
        }
        "record" => parse_output_command("record", words, true),
        "stream" => parse_output_command("stream", words, false),
        "mixer" => parse_mixer_command("mixer", words),
        "transition" => parse_transition_command("transition", words),
        "take" => parse_take_command("take", words),
        _ => Err(ConsoleCommandError::UnknownCommand(command.to_owned())),
    }
}

/// Parses one bounded HTTP/1.x request for the local browser frontend.
///
/// Only `GET /`, `GET /snapshot`, and `POST /command` are accepted. Chunked
/// requests, arbitrary paths, and bodies larger than the terminal command limit are
/// rejected so the browser surface shares the same validated command model.
///
/// # Errors
///
/// Returns [`WebRequestError`] when the request is malformed, oversized, or uses an
/// unsupported route or method.
pub fn parse_web_request(request: &[u8]) -> Result<WebRoute, WebRequestError> {
    if request.len() > MAX_WEB_REQUEST_BYTES {
        return Err(WebRequestError::TooLarge);
    }
    let request = std::str::from_utf8(request).map_err(|_| WebRequestError::InvalidUtf8)?;
    let (head, body) = request
        .split_once("\r\n\r\n")
        .ok_or(WebRequestError::Malformed)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(WebRequestError::Malformed)?;
    let mut request_words = request_line.split_whitespace();
    let method = request_words.next().ok_or(WebRequestError::Malformed)?;
    let path = request_words.next().ok_or(WebRequestError::Malformed)?;
    let version = request_words.next().ok_or(WebRequestError::Malformed)?;
    if request_words.next().is_some() || (version != "HTTP/1.0" && version != "HTTP/1.1") {
        return Err(WebRequestError::Malformed);
    }

    let mut content_length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(WebRequestError::Malformed)?;
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(WebRequestError::Malformed);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| WebRequestError::Malformed)?,
            );
        }
    }
    let actual_length = body.len();
    let expected_length = content_length.unwrap_or(0);
    if expected_length != actual_length {
        return Err(WebRequestError::ContentLengthMismatch {
            expected: expected_length,
            actual: actual_length,
        });
    }

    match method {
        "GET" => {
            if !body.is_empty() {
                return Err(WebRequestError::Malformed);
            }
            match path {
                "/" => Ok(WebRoute::Home),
                "/snapshot" => Ok(WebRoute::Snapshot),
                _ => Err(WebRequestError::InvalidPath(path.to_owned())),
            }
        }
        "POST" => {
            if path != "/command" {
                return Err(WebRequestError::InvalidPath(path.to_owned()));
            }
            if body.len() > MAX_CONSOLE_COMMAND_BYTES {
                return Err(WebRequestError::BodyTooLong);
            }
            if body.trim().is_empty() {
                return Err(WebRequestError::Malformed);
            }
            Ok(WebRoute::Command(body.to_owned()))
        }
        _ => Err(WebRequestError::UnsupportedMethod(method.to_owned())),
    }
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
    /// An audio mixer operation failed.
    Audio(AudioError),
    /// A mixer channel ID is not present.
    UnknownMixerChannel(String),
    /// A mixer gain is outside the supported 0..=2000 range.
    InvalidMixerGain(u16),
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
            Self::Audio(error) => error.fmt(formatter),
            Self::UnknownMixerChannel(id) => write!(formatter, "mixer channel {id} does not exist"),
            Self::InvalidMixerGain(gain_milli) => {
                write!(formatter, "mixer gain {gain_milli} is outside 0..=2000")
            }
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

impl From<AudioError> for UiError {
    fn from(error: AudioError) -> Self {
        Self::Audio(error)
    }
}

/// Rust-owned state shared by preview, program, and desktop control surfaces.
pub struct DesktopState {
    project: ProjectSession,
    preview_scene: Option<Identifier>,
    program_scene: Option<Identifier>,
    selected_source: Option<Identifier>,
    locale: UiLocale,
    transition: FrameTransition,
    recording: bool,
    streaming: bool,
    audio_mixer: AudioMixer,
    mixer_sources: BTreeMap<String, AudioSourceId>,
    mixer_channels: BTreeMap<String, MixerChannel>,
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
        self.project = ProjectSession::new(project);
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

    /// Renders a deterministic labeled text surface for terminals and assistive
    /// frontends.
    ///
    /// The view is deliberately toolkit-neutral: a desktop adapter can expose the
    /// same labels through a GUI accessibility tree, while a terminal frontend can
    /// print this snapshot without duplicating project or lifecycle logic.
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

    fn sync_selections_after_project_update(&mut self) {
        let (preview_valid, program_valid, selected_valid) = {
            let project = self.project.project();
            let preview_valid = self
                .preview_scene
                .as_ref()
                .is_some_and(|scene| project_has_scene(project, scene));
            let program_valid = self
                .program_scene
                .as_ref()
                .is_some_and(|scene| project_has_scene(project, scene));
            let selected_valid = match (&self.preview_scene, &self.selected_source) {
                (Some(scene), Some(source)) => project_has_source(project, scene, source),
                _ => false,
            };
            (preview_valid, program_valid, selected_valid)
        };
        if !preview_valid {
            self.preview_scene = first_scene_id(self.project.project());
        }
        if !program_valid {
            self.program_scene = first_scene_id(self.project.project());
        }
        if !selected_valid {
            self.selected_source = self
                .preview_scene
                .as_ref()
                .and_then(|scene| first_source_id(self.project.project(), scene));
        }
    }

    fn ensure_source(&self, id: &str) -> Result<(), UiError> {
        let preview_scene = self
            .preview_scene()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: "none".to_owned(),
            })?;
        let profile = self
            .project
            .project()
            .profiles()
            .find(|profile| profile.id() == self.project.project().active_profile())
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "profile",
                id: self.project.project().active_profile().to_string(),
            })?;
        let scene = profile
            .scenes()
            .find(|scene| scene.id().as_str() == preview_scene)
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: preview_scene.to_owned(),
            })?;
        if scene
            .sources()
            .iter()
            .any(|source| source.id().as_str() == id)
        {
            Ok(())
        } else {
            Err(UiError::UnknownSelection {
                kind: "source",
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

    fn set_mixer_gain(&mut self, id: &str, gain_milli: u16) -> Result<(), UiError> {
        if gain_milli > 2_000 {
            return Err(UiError::InvalidMixerGain(gain_milli));
        }
        let source = *self
            .mixer_sources
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        self.audio_mixer
            .set_gain(source, f32::from(gain_milli) / 1_000.0)?;
        let channel = self
            .mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        channel.gain_milli = gain_milli;
        Ok(())
    }

    fn set_recording(&mut self, active: bool) -> Result<&'static str, UiError> {
        if active && self.recording {
            return Err(UiError::RecordingAlreadyActive);
        }
        if !active && !self.recording {
            return Err(UiError::RecordingNotActive);
        }
        self.recording = active;
        Ok(if active {
            "recording started"
        } else {
            "recording stopped"
        })
    }

    fn set_streaming(&mut self, active: bool) -> Result<&'static str, UiError> {
        if active && self.streaming {
            return Err(UiError::StreamingAlreadyActive);
        }
        if !active && !self.streaming {
            return Err(UiError::StreamingNotActive);
        }
        self.streaming = active;
        Ok(if active {
            "streaming started"
        } else {
            "streaming stopped"
        })
    }

    fn set_transition(&mut self, transition: FrameTransition) -> Result<(), UiError> {
        if let FrameTransition::CrossFade { progress_milli } = transition {
            FrameTransition::cross_fade(progress_milli).map_err(UiError::Media)?;
        }
        self.transition = transition;
        Ok(())
    }

    fn take_preview(&mut self, transition: FrameTransition) -> Result<&'static str, UiError> {
        let preview = self
            .preview_scene
            .clone()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "preview scene",
                id: "none".to_owned(),
            })?;
        self.set_transition(transition)?;
        self.program_scene = Some(preview);
        Ok("preview sent to program")
    }

    fn toggle_mixer_mute(&mut self, id: &str) -> Result<&'static str, UiError> {
        let source = *self
            .mixer_sources
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        let muted = !self
            .mixer_channels
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?
            .muted;
        self.audio_mixer.set_muted(source, muted)?;
        self.mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?
            .muted = muted;
        Ok(if muted {
            "mixer channel muted"
        } else {
            "mixer channel unmuted"
        })
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

fn escape_html(text: &str) -> String {
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

fn localized_label(locale: UiLocale, key: &str) -> &'static str {
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

fn localized_state(active: bool, locale: UiLocale) -> &'static str {
    match (active, locale) {
        (true, UiLocale::English) => "active",
        (false, UiLocale::English) => "stopped",
        (true, UiLocale::Spanish) => "activa",
        (false, UiLocale::Spanish) => "detenida",
    }
}

fn localized_saved_state(dirty: bool, locale: UiLocale) -> &'static str {
    match (dirty, locale) {
        (true, UiLocale::English) => "unsaved",
        (false, UiLocale::English) => "saved",
        (true, UiLocale::Spanish) => "sin guardar",
        (false, UiLocale::Spanish) => "guardado",
    }
}

fn first_scene_id(project: &Project) -> Option<Identifier> {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().next())
        .map(|scene| scene.id().clone())
}

fn project_has_scene(project: &Project, scene_id: &Identifier) -> bool {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .is_some_and(|profile| profile.scenes().any(|scene| scene.id() == scene_id))
}

fn project_has_source(project: &Project, scene_id: &Identifier, source_id: &Identifier) -> bool {
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

fn first_source_id(project: &Project, scene_id: &Identifier) -> Option<Identifier> {
    project
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| profile.scenes().find(|scene| scene.id() == scene_id))
        .and_then(|scene| scene.sources().first())
        .map(|source| source.id().clone())
}

fn default_mixer() -> (
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

fn required_word<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    argument: &'static str,
) -> Result<&'a str, ConsoleCommandError> {
    words
        .next()
        .ok_or(ConsoleCommandError::MissingArgument(argument))
}

fn ensure_no_extra<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
) -> Result<(), ConsoleCommandError> {
    if let Some(value) = words.next() {
        return Err(ConsoleCommandError::InvalidArgument {
            command,
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn parse_output_command<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
    recording: bool,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    let action = required_word(&mut words, "start or stop")?;
    ensure_no_extra(command, words)?;
    let start = match action {
        "start" => true,
        "stop" => false,
        value => {
            return Err(ConsoleCommandError::InvalidArgument {
                command,
                value: value.to_owned(),
            });
        }
    };
    let command = if recording {
        if start {
            UiCommand::StartRecording
        } else {
            UiCommand::StopRecording
        }
    } else if start {
        UiCommand::StartStreaming
    } else {
        UiCommand::StopStreaming
    };
    Ok(ConsoleCommand::Apply(command))
}

fn parse_transition_command<'a>(
    command: &'static str,
    words: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    Ok(ConsoleCommand::Apply(UiCommand::SetTransition {
        transition: parse_transition_value(command, words)?,
    }))
}

fn parse_take_command<'a>(
    command: &'static str,
    words: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    Ok(ConsoleCommand::Apply(UiCommand::TakePreview {
        transition: parse_transition_value(command, words)?,
    }))
}

fn parse_transition_value<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
) -> Result<FrameTransition, ConsoleCommandError> {
    let kind = required_word(&mut words, "cut or fade")?;
    Ok(match kind {
        "cut" => {
            ensure_no_extra(command, words)?;
            FrameTransition::Cut
        }
        "fade" => {
            let progress = required_word(&mut words, "fade progress in 0..1000")?;
            ensure_no_extra(command, words)?;
            let progress =
                progress
                    .parse::<u16>()
                    .map_err(|_| ConsoleCommandError::InvalidArgument {
                        command,
                        value: progress.to_owned(),
                    })?;
            FrameTransition::cross_fade(progress).map_err(ConsoleCommandError::InvalidTransition)?
        }
        value => {
            return Err(ConsoleCommandError::InvalidArgument {
                command,
                value: value.to_owned(),
            });
        }
    })
}

fn parse_mixer_command<'a>(
    command: &'static str,
    mut words: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleCommandError> {
    let id = required_word(&mut words, "mixer channel")?;
    let action = required_word(&mut words, "gain or mute")?;
    let command = match action {
        "mute" => {
            ensure_no_extra(command, words)?;
            UiCommand::ToggleMixerMute { id: id.to_owned() }
        }
        "gain" => {
            let value = required_word(&mut words, "mixer gain 0..2000")?;
            ensure_no_extra(command, words)?;
            let gain_milli =
                value
                    .parse::<u16>()
                    .map_err(|_| ConsoleCommandError::InvalidArgument {
                        command,
                        value: value.to_owned(),
                    })?;
            if gain_milli > 2_000 {
                return Err(ConsoleCommandError::InvalidArgument {
                    command,
                    value: value.to_owned(),
                });
            }
            UiCommand::SetMixerGain {
                id: id.to_owned(),
                gain_milli,
            }
        }
        value => {
            return Err(ConsoleCommandError::InvalidArgument {
                command,
                value: value.to_owned(),
            });
        }
    };
    Ok(ConsoleCommand::Apply(command))
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
    use obs_rs_audio::AudioBuffer;
    use obs_rs_config::Config;
    use obs_rs_media::{FrameRate, Timestamp, VideoFormat};
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
    fn desktop_state_selects_source_items_in_preview_scene() {
        let mut state = DesktopState::new(project());
        assert_eq!(state.selected_source(), None);
        state
            .dispatch(UiCommand::SelectPreviewScene {
                id: "source_scene".to_owned(),
            })
            .expect("source scene selection");
        assert_eq!(state.selected_source(), Some("source"));
        state
            .dispatch(UiCommand::SelectSource {
                id: "source".to_owned(),
            })
            .expect("source selection");
        assert_eq!(state.selected_source(), Some("source"));
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
        state
            .dispatch(UiCommand::TakePreview {
                transition: FrameTransition::Cut,
            })
            .expect("take preview");
        assert_eq!(state.program_scene(), state.preview_scene());
    }

    #[test]
    fn mixer_commands_update_real_audio_controls() {
        let mut state = DesktopState::new(project());
        state
            .dispatch(UiCommand::SetMixerGain {
                id: "desktop".to_owned(),
                gain_milli: 1_500,
            })
            .expect("mixer gain");
        state
            .dispatch(UiCommand::ToggleMixerMute {
                id: "desktop".to_owned(),
            })
            .expect("mixer mute");

        let desktop = state
            .mixer_channels()
            .find(|channel| channel.id() == "desktop")
            .expect("desktop mixer channel");
        assert_eq!(desktop.gain_milli(), 1_500);
        assert!(desktop.muted());
        assert_eq!(desktop.peak_milli(), 0);
        assert_eq!(
            state.dispatch(UiCommand::SetMixerGain {
                id: "desktop".to_owned(),
                gain_milli: 2_001,
            }),
            Err(UiError::InvalidMixerGain(2_001))
        );
    }

    #[test]
    fn mixer_updates_visible_peak_meters_from_real_audio() {
        let mut state = DesktopState::new(project());
        let format = AudioFormat::new(48_000, 2).expect("audio format");
        let input = AudioBuffer::new(format, Timestamp::ZERO, vec![0.75; 8]).expect("audio input");
        let output = state
            .mix_audio(Timestamp::ZERO, 4, &[("desktop", &input)])
            .expect("audio mix");
        assert_eq!(output.samples(), &[0.75; 8]);
        assert_eq!(
            state
                .mixer_channels()
                .find(|channel| channel.id() == "desktop")
                .expect("desktop channel")
                .peak_milli(),
            750
        );
    }

    #[test]
    fn desktop_state_persists_project_editor_changes() {
        let final_path = std::env::temp_dir().join(format!(
            "obs-rs-ui-persistence-{}.project",
            std::process::id()
        ));
        let temp_path = final_path.with_file_name("obs-rs-ui-persistence.project.tmp");
        let store = ProjectFileStore::new(&final_path, &temp_path).expect("project store");
        let mut state = DesktopState::new(project());
        state
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: SceneSpec::new("studio", "Studio").expect("scene"),
            }))
            .expect("add scene");
        assert!(state.is_dirty());
        let document = state.project_document();

        let bytes = state.save_project(&store).expect("save project");
        assert_eq!(bytes, document.len());
        assert!(!state.is_dirty());

        let mut loaded = DesktopState::new(project());
        loaded.load_project(&store).expect("load project");
        assert_eq!(loaded.project_document(), document);
        assert!(!loaded.is_dirty());
        assert_eq!(loaded.preview_scene(), Some("preview"));
        assert!(!temp_path.exists());

        std::fs::remove_file(final_path).expect("remove project fixture");
    }

    #[test]
    fn console_parser_covers_state_and_output_commands() {
        assert_eq!(
            parse_console_command("preview program"),
            Ok(ConsoleCommand::Apply(UiCommand::SelectPreviewScene {
                id: "program".to_owned(),
            }))
        );
        assert_eq!(
            parse_console_command("record start"),
            Ok(ConsoleCommand::Apply(UiCommand::StartRecording))
        );
        assert_eq!(
            parse_console_command("transition fade 500"),
            Ok(ConsoleCommand::Apply(UiCommand::SetTransition {
                transition: FrameTransition::CrossFade {
                    progress_milli: 500,
                },
            }))
        );
        assert_eq!(
            parse_console_command("take fade 500"),
            Ok(ConsoleCommand::Apply(UiCommand::TakePreview {
                transition: FrameTransition::CrossFade {
                    progress_milli: 500,
                },
            }))
        );
        assert_eq!(
            parse_console_command("mixer desktop gain 1500"),
            Ok(ConsoleCommand::Apply(UiCommand::SetMixerGain {
                id: "desktop".to_owned(),
                gain_milli: 1_500,
            }))
        );
        assert_eq!(
            parse_console_command("mixer mic mute"),
            Ok(ConsoleCommand::Apply(UiCommand::ToggleMixerMute {
                id: "mic".to_owned(),
            }))
        );
        assert_eq!(
            parse_console_command("not-a-command"),
            Err(ConsoleCommandError::UnknownCommand(
                "not-a-command".to_owned()
            ))
        );
        assert_eq!(
            parse_console_command("transition fade 1001"),
            Err(ConsoleCommandError::InvalidTransition(
                MediaError::InvalidTransition {
                    progress_milli: 1_001,
                },
            ))
        );
    }

    #[test]
    fn console_commands_drive_desktop_state_without_duplicate_logic() {
        let mut state = DesktopState::new(project());
        for line in ["program program", "swap", "record start", "stream start"] {
            let command = parse_console_command(line).expect("console command");
            if let ConsoleCommand::Apply(command) = command {
                state.dispatch(command).expect("state command");
            }
        }

        assert_eq!(state.preview_scene(), Some("program"));
        assert_eq!(state.program_scene(), Some("preview"));
        assert!(state.recording());
        assert!(state.streaming());
    }

    #[test]
    fn web_request_parser_routes_bounded_browser_commands() {
        assert_eq!(
            parse_web_request(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            Ok(WebRoute::Home)
        );
        assert_eq!(
            parse_web_request(b"GET /snapshot HTTP/1.1\r\n\r\n"),
            Ok(WebRoute::Snapshot)
        );
        let body = "transition fade 500";
        let request = format!(
            "POST /command HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        assert_eq!(
            parse_web_request(request.as_bytes()),
            Ok(WebRoute::Command(body.to_owned()))
        );
        assert_eq!(
            parse_web_request(b"POST /command HTTP/1.1\r\nContent-Length: 2\r\n\r\nswap"),
            Err(WebRequestError::ContentLengthMismatch {
                expected: 2,
                actual: 4
            })
        );
        assert_eq!(
            parse_console_command("language es"),
            Ok(ConsoleCommand::Apply(UiCommand::SetLocale {
                locale: UiLocale::Spanish
            }))
        );
        assert_eq!(
            parse_web_request(b"DELETE / HTTP/1.1\r\n\r\n"),
            Err(WebRequestError::UnsupportedMethod("DELETE".to_owned()))
        );
    }

    #[test]
    fn localized_snapshot_uses_the_selected_language() {
        let mut state = DesktopState::new(project());
        state
            .dispatch(UiCommand::SetLocale {
                locale: UiLocale::Spanish,
            })
            .expect("locale selection");
        let snapshot = state.accessible_snapshot();
        assert!(snapshot.contains("Proyecto:"));
        assert!(snapshot.contains("Mezclador de audio:"));
        assert!(snapshot.contains("(es)"));
    }

    #[test]
    fn web_page_is_accessible_and_escapes_snapshot_text() {
        let state = DesktopState::new(project());
        let page = state.web_page();
        assert!(page.contains("<main id=\"main\""));
        assert!(page.contains("aria-live=\"polite\""));
        assert!(page.contains("data-command=\"swap\""));
        assert!(page.contains("data-command=\"take fade 500\""));
        assert!(page.contains("OBS-RS desktop state"));
        let mut spanish = DesktopState::new(project());
        spanish
            .dispatch(UiCommand::SetLocale {
                locale: UiLocale::Spanish,
            })
            .expect("locale selection");
        let spanish_page = spanish.web_page();
        assert!(spanish_page.contains("<html lang=\"es\">"));
        assert!(spanish_page.contains("Estado actual"));
        assert_eq!(escape_html("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
        assert_eq!(
            parse_web_request(&vec![b'x'; MAX_WEB_REQUEST_BYTES + 1]),
            Err(WebRequestError::TooLarge)
        );
    }

    #[test]
    fn accessible_snapshot_contains_labeled_state_and_scene_markers() {
        let mut state = DesktopState::new(project());
        state
            .dispatch(UiCommand::SelectProgramScene {
                id: "program".to_owned(),
            })
            .expect("program selection");
        state
            .dispatch(UiCommand::StartRecording)
            .expect("recording start");
        let snapshot = state.accessible_snapshot();

        assert!(snapshot.contains("OBS-RS desktop state"));
        assert!(snapshot.contains("Preview scene: preview"));
        assert!(snapshot.contains("Program scene: program"));
        assert!(snapshot.contains("Recording: active"));
        assert!(snapshot.contains("- preview: Preview [preview]"));
        assert!(snapshot.contains("- program: Program [program]"));
        assert!(snapshot.contains("Recent notices:"));
    }
}
