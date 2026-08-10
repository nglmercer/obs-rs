use obs_rs_media::FrameTransition;
use obs_rs_project::ProjectCommand;

use super::{error::UiError, MAX_SHORTCUT_KEY_BYTES};

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
    /// Returns the locales exposed by the current UI build.
    #[must_use]
    pub const fn supported() -> &'static [Self] {
        &[Self::English, Self::Spanish]
    }

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
        let language = code.trim().split(['-', '_']).next()?.to_ascii_lowercase();
        match language.as_str() {
            "en" | "english" => Some(Self::English),
            "es" | "spanish" => Some(Self::Spanish),
            _ => None,
        }
    }
}

/// One stateful channel shown in the desktop audio mixer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MixerChannel {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) gain_milli: u16,
    pub(crate) muted: bool,
    pub(crate) peak_milli: u16,
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
    pub(crate) sequence: u64,
    pub(crate) message: String,
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
    /// Rebuild the audio mixer at a new sample rate and channel count.
    SetAudioFormat { sample_rate: u32, channels: u16 },
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
