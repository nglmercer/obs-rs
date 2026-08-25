//! Rust-native extension contracts for the OBS-RS engine.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{fmt, sync::Arc};

use obs_rs_config::Config;
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_util::Identifier;

/// Current major version of the in-process Rust plugin contract.
pub const PLUGIN_API_MAJOR: u16 = 1;
/// Current minor version of the in-process Rust plugin contract.
pub const PLUGIN_API_MINOR: u16 = 0;
/// Maximum number of dock descriptors one plugin may contribute.
pub const MAX_PLUGIN_DOCKS: usize = 16;
/// Maximum UTF-8 byte length accepted for a plugin dock title.
pub const MAX_DOCK_TITLE_BYTES: usize = 256;

/// Version of the Rust plugin contract implemented by a plugin.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PluginApiVersion {
    major: u16,
    minor: u16,
}

impl PluginApiVersion {
    /// Creates an explicit API version.
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Returns the supported contract version for this workspace.
    #[must_use]
    pub const fn current() -> Self {
        Self::new(PLUGIN_API_MAJOR, PLUGIN_API_MINOR)
    }

    /// Returns the major version.
    #[must_use]
    pub const fn major(self) -> u16 {
        self.major
    }

    /// Returns the minor version.
    #[must_use]
    pub const fn minor(self) -> u16 {
        self.minor
    }
}

/// Metadata identifying a plugin build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginManifest {
    id: Identifier,
    name: String,
    version: String,
    api_version: PluginApiVersion,
}

impl PluginManifest {
    /// Creates metadata after validating the plugin identifier.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InvalidManifest`] for an empty name or version, or
    /// [`PluginError::InvalidIdentifier`] for an invalid plugin ID.
    pub fn new(id: &str, name: &str, version: &str) -> Result<Self, PluginError> {
        Self::with_api_version(id, name, version, PluginApiVersion::current())
    }

    /// Creates metadata with an explicit Rust plugin API version.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InvalidManifest`] for an empty name or version, or
    /// [`PluginError::InvalidIdentifier`] for an invalid plugin ID.
    pub fn with_api_version(
        id: &str,
        name: &str,
        version: &str,
        api_version: PluginApiVersion,
    ) -> Result<Self, PluginError> {
        if name.trim().is_empty() || version.trim().is_empty() {
            return Err(PluginError::InvalidManifest {
                field: "name or version",
            });
        }

        Ok(Self {
            id: Identifier::new(id).map_err(PluginError::InvalidIdentifier)?,
            name: name.to_owned(),
            version: version.to_owned(),
            api_version,
        })
    }

    /// Returns the stable plugin identifier.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the plugin version string.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the Rust contract version implemented by this plugin.
    #[must_use]
    pub const fn api_version(&self) -> PluginApiVersion {
        self.api_version
    }
}

/// A validated declaration for one plugin-provided studio dock.
///
/// The descriptor is deliberately metadata-only. It gives the runtime and
/// later UI host a stable, plugin-scoped identity without allowing a plugin to
/// inject toolkit objects or an unbounded title into the desktop process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockDescriptor {
    id: Identifier,
    title: String,
}

impl DockDescriptor {
    /// Creates a dock descriptor after validating its identity and title.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InvalidIdentifier`] for an invalid dock ID or
    /// [`PluginError::InvalidDockDescriptor`] for an empty, oversized, or
    /// control-character-containing title.
    pub fn new(id: &str, title: &str) -> Result<Self, PluginError> {
        if title.trim().is_empty()
            || title.len() > MAX_DOCK_TITLE_BYTES
            || title.chars().any(char::is_control)
        {
            return Err(PluginError::InvalidDockDescriptor { field: "title" });
        }
        Ok(Self {
            id: Identifier::new(id).map_err(PluginError::InvalidIdentifier)?,
            title: title.to_owned(),
        })
    }

    /// Returns the plugin-local stable dock identifier.
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    /// Returns the user-facing title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// A request for one source video frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoRequest {
    timestamp: Timestamp,
    format: VideoFormat,
}

impl VideoRequest {
    /// Creates a frame request.
    #[must_use]
    pub const fn new(timestamp: Timestamp, format: VideoFormat) -> Self {
        Self { timestamp, format }
    }

    /// Returns the requested timestamp.
    #[must_use]
    pub const fn timestamp(self) -> Timestamp {
        self.timestamp
    }

    /// Returns the requested output format.
    #[must_use]
    pub const fn format(self) -> VideoFormat {
        self.format
    }
}

/// Errors produced while constructing or rendering a source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceError {
    /// A setting is missing or cannot be parsed.
    InvalidSetting {
        /// The setting key that failed validation.
        key: String,
        /// Human-readable reason for the failure.
        reason: String,
    },
    /// The source cannot produce the requested format.
    UnsupportedFormat {
        /// The source's configured format.
        configured: VideoFormat,
        /// The requested format.
        requested: VideoFormat,
    },
    /// The source is temporarily unavailable.
    Unavailable(String),
}

impl SourceError {
    /// Creates an invalid-setting error without repeating allocation details at
    /// call sites.
    #[must_use]
    pub fn invalid_setting(key: &str, reason: impl Into<String>) -> Self {
        Self::InvalidSetting {
            key: key.to_owned(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSetting { key, reason } => {
                write!(formatter, "invalid source setting {key}: {reason}")
            }
            Self::UnsupportedFormat {
                configured,
                requested,
            } => write!(
                formatter,
                "source format {configured:?} does not support request {requested:?}"
            ),
            Self::Unavailable(reason) => write!(formatter, "source unavailable: {reason}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// A live source instance owned by the runtime.
pub trait Source: Send {
    /// Returns the registered source kind.
    fn kind(&self) -> &Identifier;

    /// Returns the user-facing instance name.
    fn name(&self) -> &str;

    /// Applies new settings atomically from the source's perspective.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when the source cannot accept the settings.
    fn update(&mut self, settings: &Config) -> Result<(), SourceError>;

    /// Returns a backend-generated settings update, if one is ready.
    ///
    /// This is used for state that a live source learns from its backend after
    /// creation, such as a Wayland portal's replacement restore token. The
    /// default keeps existing sources source-compatible with the optional
    /// lifecycle event.
    fn take_settings_update(&mut self) -> Option<Config> {
        None
    }

    /// Produces one frame or reports that no frame is ready yet.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when the source cannot satisfy the request.
    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError>;
}

/// Factory for one registered source kind.
pub trait SourceFactory: Send + Sync {
    /// Returns the stable kind identifier.
    fn kind(&self) -> &Identifier;

    /// Validates settings and creates a source instance.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when the settings or source name are invalid.
    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError>;
}

/// A Rust plugin that contributes source factories to a runtime.
pub trait Plugin: Send + Sync {
    /// Returns immutable plugin metadata.
    fn manifest(&self) -> &PluginManifest;

    /// Returns the factories contributed by this plugin.
    fn source_factories(&self) -> &[Arc<dyn SourceFactory>];

    /// Returns bounded metadata for plugin-provided studio docks.
    ///
    /// The default keeps existing source-only plugins source-compatible. A
    /// runtime owns registration and later decides whether a UI host is
    /// available; the plugin never receives a Slint or native window handle.
    fn dock_descriptors(&self) -> &[DockDescriptor] {
        &[]
    }
}

/// Errors raised before a plugin can be registered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginError {
    /// The plugin identifier is invalid.
    InvalidIdentifier(obs_rs_util::IdentifierError),
    /// A required manifest field is empty.
    InvalidManifest {
        /// Name of the invalid logical field.
        field: &'static str,
    },
    /// A plugin dock descriptor contains an invalid field.
    InvalidDockDescriptor {
        /// Human-readable logical field name.
        field: &'static str,
    },
    /// The plugin's API version is not compatible with this runtime.
    UnsupportedApi {
        /// Runtime API version.
        expected: PluginApiVersion,
        /// Plugin API version.
        actual: PluginApiVersion,
    },
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier(error) => {
                write!(formatter, "invalid plugin identifier: {error}")
            }
            Self::InvalidManifest { field } => {
                write!(formatter, "invalid plugin manifest field: {field}")
            }
            Self::InvalidDockDescriptor { field } => {
                write!(formatter, "invalid plugin dock descriptor field: {field}")
            }
            Self::UnsupportedApi { expected, actual } => write!(
                formatter,
                "plugin API {actual:?} is incompatible with runtime API {expected:?}"
            ),
        }
    }
}

impl std::error::Error for PluginError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_plugin_manifest() {
        let manifest =
            PluginManifest::new("test_plugin", "Test plugin", "0.1.0").expect("valid manifest");

        assert_eq!(manifest.id().as_str(), "test_plugin");
        assert_eq!(manifest.name(), "Test plugin");
        assert_eq!(manifest.version(), "0.1.0");
        assert_eq!(manifest.api_version(), PluginApiVersion::current());
        let legacy = PluginManifest::with_api_version(
            "legacy_plugin",
            "Legacy plugin",
            "0.1.0",
            PluginApiVersion::new(0, 9),
        )
        .expect("explicit API version");
        assert_eq!(legacy.api_version(), PluginApiVersion::new(0, 9));
        assert!(matches!(
            PluginManifest::new("test_plugin", "", "0.1.0"),
            Err(PluginError::InvalidManifest { .. })
        ));
    }

    #[test]
    fn validates_bounded_plugin_dock_descriptors() {
        let dock = DockDescriptor::new("stats", "Plugin stats").expect("valid dock");
        assert_eq!(dock.id().as_str(), "stats");
        assert_eq!(dock.title(), "Plugin stats");
        assert!(matches!(
            DockDescriptor::new("stats", "\n"),
            Err(PluginError::InvalidDockDescriptor { field: "title" })
        ));
        assert!(matches!(
            DockDescriptor::new("stats", &"x".repeat(MAX_DOCK_TITLE_BYTES + 1)),
            Err(PluginError::InvalidDockDescriptor { field: "title" })
        ));
    }
}
