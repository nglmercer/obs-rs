use obs_rs_config::ConfigError;
use obs_rs_media::MediaError;
use obs_rs_util::{Identifier, IdentifierError};
use std::fmt;
/// Errors raised by project state and persistence operations.
#[derive(Debug, Eq, PartialEq)]
pub enum ProjectError {
    /// The serialized document exceeds [`crate::MAX_PROJECT_BYTES`].
    DocumentTooLarge,
    /// A project, profile, scene, or source name is empty.
    InvalidName { kind: &'static str },
    /// An identifier failed validation.
    InvalidIdentifier {
        /// Logical identifier kind.
        kind: &'static str,
        /// Underlying validation failure.
        error: IdentifierError,
    },
    /// A serialized line is malformed.
    InvalidDocument { line: usize, reason: String },
    /// Final and temporary persistence paths are invalid.
    InvalidPaths { reason: String },
    /// A project file operation failed.
    Io {
        /// Logical filesystem operation.
        operation: &'static str,
        /// Underlying operating-system message.
        message: String,
    },
    /// A source setting document is invalid.
    Config(ConfigError),
    /// A video or transform value is invalid.
    Media(MediaError),
    /// A profile ID is already present.
    DuplicateProfile(Identifier),
    /// A scene ID is already present.
    DuplicateScene(Identifier),
    /// A source ID is already present in the profile registry.
    DuplicateSource(Identifier),
    /// A scene-item ID is already present in a scene.
    DuplicateSceneItem(Identifier),
    /// A filter ID is already present on a source.
    DuplicateFilter(Identifier),
    /// A profile ID is not present.
    UnknownProfile(Identifier),
    /// A scene ID is not present.
    UnknownScene(Identifier),
    /// A scene graph would recurse forever through nested scene references.
    CircularSceneReference(Identifier),
    /// A nested scene transform cannot be represented by the current flattening boundary.
    UnsupportedNestedSceneTransform(Identifier),
    /// A source ID is not present.
    UnknownSource(Identifier),
    /// A scene-item ID is not present.
    UnknownSceneItem(Identifier),
    /// A filter ID is not present on a source.
    UnknownFilter(Identifier),
    /// A legacy source move destination is outside the scene order.
    InvalidSourceOrder { index: usize },
    /// A filter move destination is outside the filter order.
    InvalidFilterOrder { index: usize },
    /// A scene-item move destination is outside the scene order.
    InvalidSceneItemOrder { index: usize },
    /// A source cannot be removed while a scene item references it.
    SourceInUse(Identifier),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DocumentTooLarge => formatter.write_str("project document is too large"),
            Self::InvalidName { kind } => write!(formatter, "{kind} name is empty"),
            Self::InvalidIdentifier { kind, error } => {
                write!(formatter, "invalid {kind} identifier: {error}")
            }
            Self::InvalidDocument { line, reason } => {
                write!(formatter, "invalid project document line {line}: {reason}")
            }
            Self::InvalidPaths { reason } => write!(formatter, "invalid project paths: {reason}"),
            Self::Io { operation, message } => write!(formatter, "{operation} failed: {message}"),
            Self::Config(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
            Self::DuplicateProfile(id) => write!(formatter, "profile {id} already exists"),
            Self::DuplicateScene(id) => write!(formatter, "scene {id} already exists"),
            Self::DuplicateSource(id) => write!(formatter, "source {id} already exists"),
            Self::DuplicateSceneItem(id) => write!(formatter, "scene item {id} already exists"),
            Self::DuplicateFilter(id) => write!(formatter, "filter {id} already exists"),
            Self::UnknownProfile(id) => write!(formatter, "profile {id} does not exist"),
            Self::UnknownScene(id) => write!(formatter, "scene {id} does not exist"),
            Self::CircularSceneReference(id) => {
                write!(formatter, "scene graph contains a cycle at {id}")
            }
            Self::UnsupportedNestedSceneTransform(id) => {
                write!(
                    formatter,
                    "nested scene item {id} has an unsupported transform"
                )
            }
            Self::UnknownSource(id) => write!(formatter, "source {id} does not exist"),
            Self::UnknownSceneItem(id) => write!(formatter, "scene item {id} does not exist"),
            Self::UnknownFilter(id) => write!(formatter, "filter {id} does not exist"),
            Self::InvalidSourceOrder { index } => {
                write!(formatter, "source order index {index} is out of range")
            }
            Self::InvalidFilterOrder { index } => {
                write!(formatter, "filter order index {index} is out of range")
            }
            Self::InvalidSceneItemOrder { index } => {
                write!(formatter, "scene item order index {index} is out of range")
            }
            Self::SourceInUse(id) => write!(formatter, "source {id} is still used by a scene"),
        }
    }
}

impl std::error::Error for ProjectError {}
