use std::fmt;

use obs_rs_media::MediaError;
use obs_rs_plugin_api::{PluginApiVersion, SourceError};
use obs_rs_util::{Identifier, IdentifierError};

use crate::SourceId;

pub(crate) fn identifier(input: &str, kind: &'static str) -> Result<Identifier, RuntimeError> {
    Identifier::new(input).map_err(|error| RuntimeError::InvalidIdentifier { kind, error })
}

/// Errors raised by runtime lifecycle and rendering operations.
#[derive(Debug, Eq, PartialEq)]
pub enum RuntimeError {
    /// A name or kind failed identifier validation.
    InvalidIdentifier {
        /// The logical value being validated.
        kind: &'static str,
        /// The validation failure.
        error: IdentifierError,
    },
    /// A user-facing source or scene name is empty.
    InvalidName {
        /// The logical value being named.
        kind: &'static str,
    },
    /// The plugin is already registered.
    DuplicatePlugin(Identifier),
    /// A source kind is already owned by another factory.
    DuplicateSourceKind(Identifier),
    /// A scene name is already in use.
    DuplicateScene(Identifier),
    /// No factory is registered for a source kind.
    UnknownSourceKind(Identifier),
    /// No source exists for an ID.
    UnknownSource(SourceId),
    /// No scene exists for a name.
    UnknownScene(Identifier),
    /// A source is already present in a scene.
    SourceAlreadyAttached(SourceId),
    /// A source was not present in a scene.
    SourceNotAttached(SourceId),
    /// A scene-item index was outside the ordered item list.
    SceneItemOutOfBounds { index: usize },
    /// A source cannot be destroyed while a scene references it.
    SourceInUse(SourceId),
    /// Source IDs are exhausted.
    IdExhausted,
    /// A plugin requires an API version this runtime cannot provide.
    UnsupportedPluginApi {
        /// Runtime API version.
        expected: PluginApiVersion,
        /// Plugin API version.
        actual: PluginApiVersion,
    },
    /// A runtime-owned resource reached its configured safety limit.
    ResourceLimitExceeded {
        /// Human-readable resource class.
        resource: &'static str,
        /// Configured maximum.
        limit: usize,
    },
    /// A source rejected creation, update, or rendering.
    Source(SourceError),
    /// A media invariant failed during composition.
    Media(MediaError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { kind, error } => {
                write!(formatter, "invalid {kind} identifier: {error}")
            }
            Self::InvalidName { kind } => write!(formatter, "{kind} name is empty"),
            Self::DuplicatePlugin(id) => write!(formatter, "plugin {id} is already registered"),
            Self::DuplicateSourceKind(kind) => {
                write!(formatter, "source kind {kind} is already registered")
            }
            Self::DuplicateScene(name) => write!(formatter, "scene {name} already exists"),
            Self::UnknownSourceKind(kind) => {
                write!(formatter, "source kind {kind} is not registered")
            }
            Self::UnknownSource(source) => {
                write!(formatter, "source {} does not exist", source.value())
            }
            Self::UnknownScene(scene) => write!(formatter, "scene {scene} does not exist"),
            Self::SourceAlreadyAttached(source) => {
                write!(formatter, "source {} is already attached", source.value())
            }
            Self::SourceNotAttached(source) => {
                write!(formatter, "source {} is not attached", source.value())
            }
            Self::SceneItemOutOfBounds { index } => {
                write!(formatter, "scene item index {index} is out of bounds")
            }
            Self::SourceInUse(source) => {
                write!(
                    formatter,
                    "source {} is still used by a scene",
                    source.value()
                )
            }
            Self::IdExhausted => formatter.write_str("source ID space is exhausted"),
            Self::UnsupportedPluginApi { expected, actual } => write!(
                formatter,
                "plugin API {actual:?} is incompatible with runtime API {expected:?}"
            ),
            Self::ResourceLimitExceeded { resource, limit } => {
                write!(formatter, "runtime {resource} limit of {limit} was reached")
            }
            Self::Source(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RuntimeError {}
