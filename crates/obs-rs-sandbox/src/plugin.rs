use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use obs_rs_plugin_api::{Plugin, PluginManifest, SourceFactory};

use super::{
    discovery::discover_sandbox_manifest, error::SandboxError, manifest::SandboxedPluginManifest,
    process_source::ProcessSourceFactory, validation::validate_command,
};

/// A compile-time-safe host for source factories backed by a child process.
pub struct SandboxedPlugin {
    manifest: PluginManifest,
    command: PathBuf,
    arguments: Vec<String>,
    factories: Vec<Arc<dyn SourceFactory>>,
}

impl SandboxedPlugin {
    /// Discovers and configures a subprocess plugin from its manifest probe.
    ///
    /// # Errors
    ///
    /// Propagates manifest-probe or direct-launch policy errors.
    pub fn from_process(
        command: impl AsRef<Path>,
        arguments: Vec<String>,
    ) -> Result<Self, SandboxError> {
        let manifest = discover_sandbox_manifest(command.as_ref(), &arguments)?;
        Self::new(&manifest, command.as_ref(), arguments)
    }

    /// Creates a subprocess plugin without invoking the command.
    ///
    /// The executable is launched directly for each source instance. The child
    /// must write consecutive `OBSFRM01` packets to stdout; stdout is bounded by
    /// the capture decoder and stderr is discarded so an extension cannot corrupt
    /// the media stream with log text.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError`] when the command or argument policy is invalid.
    pub fn new(
        manifest: &SandboxedPluginManifest,
        command: impl Into<PathBuf>,
        arguments: Vec<String>,
    ) -> Result<Self, SandboxError> {
        let command = command.into();
        validate_command(&command, &arguments)?;
        let factories = manifest
            .source_kinds()
            .iter()
            .cloned()
            .map(|kind| {
                Arc::new(ProcessSourceFactory {
                    kind,
                    command: command.clone(),
                    arguments: arguments.clone(),
                }) as Arc<dyn SourceFactory>
            })
            .collect();
        Ok(Self {
            manifest: manifest.manifest().clone(),
            command,
            arguments,
            factories,
        })
    }

    /// Returns the executable selected for this plugin.
    #[must_use]
    pub fn command(&self) -> &Path {
        &self.command
    }

    /// Returns the fixed argument vector passed to every source process.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

impl Plugin for SandboxedPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source_factories(&self) -> Vec<Arc<dyn SourceFactory>> {
        self.factories.clone()
    }
}
