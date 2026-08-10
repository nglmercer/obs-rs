use obs_rs_plugin_api::{PluginApiVersion, PluginManifest};
use obs_rs_util::Identifier;

use super::{
    error::SandboxError,
    protocol::{MAX_SANDBOX_MANIFEST_BYTES, SANDBOX_MANIFEST_MAGIC},
    validation::{invalid_manifest, validate_source_kinds},
};

/// A validated manifest for a subprocess plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxedPluginManifest {
    manifest: PluginManifest,
    source_kinds: Vec<Identifier>,
}

impl SandboxedPluginManifest {
    /// Creates a manifest from an existing plugin manifest and source kinds.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::InvalidManifest`] when no source kinds are
    /// supplied, a kind is duplicated, or the kind limit is exceeded.
    pub fn new(
        manifest: PluginManifest,
        source_kinds: impl IntoIterator<Item = Identifier>,
    ) -> Result<Self, SandboxError> {
        let source_kinds = source_kinds.into_iter().collect::<Vec<_>>();
        validate_source_kinds(&source_kinds)?;
        Ok(Self {
            manifest,
            source_kinds,
        })
    }

    /// Parses the bounded manifest format:
    ///
    /// ```text
    /// OBSRPLUGIN1
    /// plugin_id|display name|version|major|minor|source_kind,other_kind
    /// ```
    ///
    /// Names and versions must not contain `|`; source kinds use the shared
    /// identifier alphabet. The parser rejects trailing records so a caller can
    /// safely treat one document as one manifest.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError`] when the document is oversized, malformed, or
    /// incompatible with the current plugin API.
    pub fn parse(document: &str) -> Result<Self, SandboxError> {
        if document.len() > MAX_SANDBOX_MANIFEST_BYTES {
            return Err(SandboxError::ManifestTooLarge);
        }
        let mut lines = document.lines();
        if lines.next() != Some(SANDBOX_MANIFEST_MAGIC) {
            return Err(invalid_manifest("invalid sandbox manifest header"));
        }
        let record = lines
            .next()
            .ok_or_else(|| invalid_manifest("missing plugin record"))?;
        if lines.next().is_some() {
            return Err(invalid_manifest("trailing manifest records"));
        }
        let fields = record.split('|').collect::<Vec<_>>();
        if fields.len() != 6 || fields.iter().any(|field| field.is_empty()) {
            return Err(invalid_manifest("plugin record needs six non-empty fields"));
        }
        let major = fields[3]
            .parse::<u16>()
            .map_err(|_| invalid_manifest("API major version is invalid"))?;
        let minor = fields[4]
            .parse::<u16>()
            .map_err(|_| invalid_manifest("API minor version is invalid"))?;
        let manifest = PluginManifest::with_api_version(
            fields[0],
            fields[1],
            fields[2],
            PluginApiVersion::new(major, minor),
        )?;
        let source_kinds = fields[5]
            .split(',')
            .map(Identifier::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid_manifest(format!("source kind is invalid: {error}")))?;
        Self::new(manifest, source_kinds)
    }

    /// Returns the validated plugin metadata.
    #[must_use]
    pub const fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Returns source kinds in manifest order.
    #[must_use]
    pub fn source_kinds(&self) -> &[Identifier] {
        &self.source_kinds
    }

    /// Serializes the manifest deterministically.
    #[must_use]
    pub fn serialize(&self) -> String {
        let api = self.manifest.api_version();
        format!(
            "{SANDBOX_MANIFEST_MAGIC}\n{}|{}|{}|{}|{}|{}\n",
            self.manifest.id(),
            self.manifest.name(),
            self.manifest.version(),
            api.major(),
            api.minor(),
            self.source_kinds
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}
