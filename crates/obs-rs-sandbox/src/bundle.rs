use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use obs_rs_plugin_api::{PluginApiVersion, PluginManifest};
use obs_rs_util::Identifier;
use sha2::{Digest, Sha256};

use super::{SandboxError, MAX_SANDBOX_SOURCE_KINDS};

pub const PLUGIN_BUNDLE_MAGIC: &[u8; 8] = b"OBSRPB01";
pub const MAX_PLUGIN_BUNDLE_BYTES: usize = 64 * 1_024 * 1_024;
pub const MAX_PLUGIN_PAYLOADS: usize = 32;
pub const MAX_PLUGIN_PAYLOAD_PATH_BYTES: usize = 240;
const MAX_CANONICAL_MANIFEST_BYTES: usize = 64 * 1_024;
static NEXT_INSTALL_ID: AtomicU64 = AtomicU64::new(0);

/// Capability that an external plugin asks the host to grant.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PluginCapability {
    Camera,
    Screen,
    Microphone,
    Network,
    FileRead,
    FileWrite,
}

impl PluginCapability {
    const fn tag(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Screen => "screen",
            Self::Microphone => "microphone",
            Self::Network => "network",
            Self::FileRead => "file-read",
            Self::FileWrite => "file-write",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "camera" => Some(Self::Camera),
            "screen" => Some(Self::Screen),
            "microphone" => Some(Self::Microphone),
            "network" => Some(Self::Network),
            "file-read" => Some(Self::FileRead),
            "file-write" => Some(Self::FileWrite),
            _ => None,
        }
    }
}

/// Canonical metadata signed for one subprocess plugin bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginBundleManifest {
    plugin: PluginManifest,
    target: String,
    minimum_app_version: String,
    signing_key_id: Identifier,
    executable_path: PathBuf,
    source_kinds: Vec<Identifier>,
    capabilities: BTreeSet<PluginCapability>,
}

impl PluginBundleManifest {
    /// Creates bounded canonical bundle metadata.
    ///
    /// # Errors
    ///
    /// Rejects unsafe delimiters, invalid versions, empty source kinds, or an
    /// invalid target/key identifier before signing.
    pub fn new(
        plugin: PluginManifest,
        target: &str,
        minimum_app_version: &str,
        signing_key_id: &str,
        executable_path: impl Into<PathBuf>,
        source_kinds: impl IntoIterator<Item = Identifier>,
        capabilities: impl IntoIterator<Item = PluginCapability>,
    ) -> Result<Self, SandboxError> {
        let source_kinds = source_kinds.into_iter().collect::<Vec<_>>();
        if source_kinds.is_empty() || source_kinds.len() > MAX_SANDBOX_SOURCE_KINDS {
            return Err(bundle_error(
                "source kind count is outside the bundle limit",
            ));
        }
        if source_kinds.iter().collect::<BTreeSet<_>>().len() != source_kinds.len() {
            return Err(bundle_error("source kinds are duplicated"));
        }
        for value in [plugin.name(), plugin.version(), target, minimum_app_version] {
            if value.is_empty()
                || value
                    .bytes()
                    .any(|byte| matches!(byte, b'|' | b'\n' | b'\r' | b','))
            {
                return Err(bundle_error("manifest text contains an unsafe delimiter"));
            }
        }
        validate_version(plugin.version())?;
        validate_version(minimum_app_version)?;
        if target.len() > 128
            || !target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(bundle_error("target triple is invalid"));
        }
        let executable_path = executable_path.into();
        validate_payload_path(&executable_path)?;
        Ok(Self {
            plugin,
            target: target.to_owned(),
            minimum_app_version: minimum_app_version.to_owned(),
            signing_key_id: Identifier::new(signing_key_id)
                .map_err(|error| bundle_error(error.to_string()))?,
            executable_path,
            source_kinds,
            capabilities: capabilities.into_iter().collect(),
        })
    }

    #[must_use]
    pub const fn plugin(&self) -> &PluginManifest {
        &self.plugin
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub fn minimum_app_version(&self) -> &str {
        &self.minimum_app_version
    }

    #[must_use]
    pub const fn signing_key_id(&self) -> &Identifier {
        &self.signing_key_id
    }

    /// Returns the relative subprocess executable path.
    #[must_use]
    pub fn executable_path(&self) -> &Path {
        &self.executable_path
    }

    #[must_use]
    pub fn source_kinds(&self) -> &[Identifier] {
        &self.source_kinds
    }

    #[must_use]
    pub const fn capabilities(&self) -> &BTreeSet<PluginCapability> {
        &self.capabilities
    }
}

/// One path-safe executable or resource payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginPayload {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl PluginPayload {
    /// Creates a bounded relative payload.
    ///
    /// # Errors
    ///
    /// Rejects absolute paths, traversal, empty files, and oversized paths.
    pub fn new(path: impl Into<PathBuf>, bytes: Vec<u8>) -> Result<Self, SandboxError> {
        let path = path.into();
        validate_payload_path(&path)?;
        if bytes.is_empty() {
            return Err(bundle_error("plugin payload is empty"));
        }
        Ok(Self { path, bytes })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Canonical signed subprocess bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedPluginBundle {
    manifest: PluginBundleManifest,
    payloads: Vec<PluginPayload>,
    signature: [u8; 64],
}

impl SignedPluginBundle {
    /// Builds and signs a bundle after validating all payload limits.
    ///
    /// # Errors
    ///
    /// Rejects duplicate paths and bundles exceeding the aggregate size bound.
    pub fn sign(
        manifest: PluginBundleManifest,
        payloads: impl IntoIterator<Item = PluginPayload>,
        key: &SigningKey,
    ) -> Result<Self, SandboxError> {
        let payloads = payloads.into_iter().collect::<Vec<_>>();
        validate_payloads(&payloads)?;
        if !payloads
            .iter()
            .any(|payload| payload.path == manifest.executable_path)
        {
            return Err(bundle_error("signed executable is missing from payloads"));
        }
        let canonical = canonical_manifest(&manifest, &payloads)?;
        let signature = key.sign(&canonical).to_bytes();
        Ok(Self {
            manifest,
            payloads,
            signature,
        })
    }

    #[must_use]
    pub const fn manifest(&self) -> &PluginBundleManifest {
        &self.manifest
    }

    #[must_use]
    pub fn payloads(&self) -> &[PluginPayload] {
        &self.payloads
    }

    /// Verifies trust, compatibility, hashes, paths, size, and permissions.
    ///
    /// # Errors
    ///
    /// Returns a typed policy error before a payload can be installed/launched.
    pub fn verify(
        &self,
        trust: &PluginTrustStore,
        policy: &PluginVerificationPolicy,
    ) -> Result<VerifiedPluginBundle, SandboxError> {
        validate_payloads(&self.payloads)?;
        verify_compatibility(&self.manifest, policy)?;
        let key = trust
            .keys
            .get(&self.manifest.signing_key_id)
            .ok_or(SandboxError::UnknownSigningKey)?;
        let canonical = canonical_manifest(&self.manifest, &self.payloads)?;
        key.verify(&canonical, &Signature::from_bytes(&self.signature))
            .map_err(|_| SandboxError::InvalidBundleSignature)?;
        Ok(VerifiedPluginBundle(self.clone()))
    }

    /// Encodes the bounded binary bundle used for installation and fuzzing.
    ///
    /// # Errors
    ///
    /// Returns a bundle validation error if internal bounds are violated.
    pub fn encode(&self) -> Result<Vec<u8>, SandboxError> {
        validate_payloads(&self.payloads)?;
        let manifest = canonical_manifest(&self.manifest, &self.payloads)?;
        let mut output = Vec::new();
        output.extend_from_slice(PLUGIN_BUNDLE_MAGIC);
        output.extend_from_slice(&u32_len(manifest.len())?.to_le_bytes());
        output.extend_from_slice(&u16_len(self.payloads.len())?.to_le_bytes());
        output.extend_from_slice(&self.signature);
        output.extend_from_slice(&manifest);
        for payload in &self.payloads {
            let path = payload.path.to_string_lossy();
            output.extend_from_slice(&u16_len(path.len())?.to_le_bytes());
            output.extend_from_slice(&u64_len(payload.bytes.len())?.to_le_bytes());
            output.extend_from_slice(path.as_bytes());
            output.extend_from_slice(&payload.bytes);
        }
        if output.len() > MAX_PLUGIN_BUNDLE_BYTES {
            return Err(SandboxError::BundleTooLarge {
                bytes: output.len(),
            });
        }
        Ok(output)
    }

    /// Decodes a bundle while enforcing limits before allocation.
    ///
    /// # Errors
    ///
    /// Rejects malformed, oversized, truncated, or hash-inconsistent bundles.
    pub fn decode(input: &[u8]) -> Result<Self, SandboxError> {
        if input.len() > MAX_PLUGIN_BUNDLE_BYTES {
            return Err(SandboxError::BundleTooLarge { bytes: input.len() });
        }
        let mut cursor = BundleCursor::new(input);
        if cursor.take(PLUGIN_BUNDLE_MAGIC.len())? != PLUGIN_BUNDLE_MAGIC {
            return Err(bundle_error("invalid plugin bundle header"));
        }
        let manifest_len = usize::try_from(cursor.u32()?)
            .map_err(|_| bundle_error("manifest length is invalid"))?;
        if manifest_len > MAX_CANONICAL_MANIFEST_BYTES {
            return Err(bundle_error("canonical manifest is too large"));
        }
        let payload_count = usize::from(cursor.u16()?);
        if payload_count == 0 || payload_count > MAX_PLUGIN_PAYLOADS {
            return Err(bundle_error("payload count is outside the bundle limit"));
        }
        let signature: [u8; 64] = cursor
            .take(64)?
            .try_into()
            .map_err(|_| bundle_error("signature is truncated"))?;
        let canonical = cursor.take(manifest_len)?;
        let (manifest, metadata) = parse_canonical_manifest(canonical)?;
        if metadata.len() != payload_count {
            return Err(bundle_error("payload metadata count does not match bundle"));
        }
        let mut payloads = Vec::with_capacity(payload_count);
        for (expected_path, expected_len, expected_hash) in metadata {
            let path_len = usize::from(cursor.u16()?);
            if path_len == 0 || path_len > MAX_PLUGIN_PAYLOAD_PATH_BYTES {
                return Err(bundle_error("payload path length is invalid"));
            }
            let payload_len = usize::try_from(cursor.u64()?)
                .map_err(|_| bundle_error("payload length is invalid"))?;
            if payload_len > MAX_PLUGIN_BUNDLE_BYTES {
                return Err(SandboxError::BundleTooLarge { bytes: payload_len });
            }
            let path = std::str::from_utf8(cursor.take(path_len)?)
                .map_err(|_| bundle_error("payload path is not UTF-8"))?;
            if path != expected_path || payload_len != expected_len {
                return Err(SandboxError::PayloadHashMismatch);
            }
            let bytes = cursor.take(payload_len)?.to_vec();
            if sha256(&bytes) != expected_hash {
                return Err(SandboxError::PayloadHashMismatch);
            }
            payloads.push(PluginPayload::new(path, bytes)?);
        }
        if cursor.remaining() != 0 {
            return Err(bundle_error("plugin bundle has trailing bytes"));
        }
        Ok(Self {
            manifest,
            payloads,
            signature,
        })
    }
}

/// A bundle that passed every launch/install policy gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPluginBundle(SignedPluginBundle);

impl VerifiedPluginBundle {
    #[must_use]
    pub const fn bundle(&self) -> &SignedPluginBundle {
        &self.0
    }
}

/// Creates an installable unsigned bundle only in development builds.
///
/// Target, version, API, path, size, hash metadata, and permission checks still
/// apply. This symbol is absent from release builds, so production code cannot
/// accidentally enable unsigned plugins through a runtime flag.
///
/// # Errors
///
/// Rejects invalid payloads, missing executables, incompatible targets or
/// versions, and capabilities denied by `policy`.
#[cfg(debug_assertions)]
pub fn verify_unsigned_development(
    manifest: PluginBundleManifest,
    payloads: impl IntoIterator<Item = PluginPayload>,
    policy: &PluginVerificationPolicy,
) -> Result<VerifiedPluginBundle, SandboxError> {
    let payloads = payloads.into_iter().collect::<Vec<_>>();
    validate_payloads(&payloads)?;
    if !payloads
        .iter()
        .any(|payload| payload.path == manifest.executable_path)
    {
        return Err(bundle_error("unsigned executable is missing from payloads"));
    }
    verify_compatibility(&manifest, policy)?;
    Ok(VerifiedPluginBundle(SignedPluginBundle {
        manifest,
        payloads,
        signature: [0; 64],
    }))
}

/// Paths produced by an atomic verified installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledPlugin {
    directory: PathBuf,
    command: PathBuf,
}

impl InstalledPlugin {
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn command(&self) -> &Path {
        &self.command
    }
}

/// Installs only a previously verified bundle using temp-directory plus rename.
///
/// Existing versions are never overwritten. Payload paths are checked again
/// before any write, and a failed staging attempt is removed.
///
/// # Errors
///
/// Returns a structural or I/O error without publishing a partial installation.
pub fn install_verified_plugin(
    bundle: &VerifiedPluginBundle,
    root: impl AsRef<Path>,
) -> Result<InstalledPlugin, SandboxError> {
    let root = root.as_ref();
    if root.as_os_str().is_empty() {
        return Err(bundle_error("plugin installation root is empty"));
    }
    fs::create_dir_all(root).map_err(|error| bundle_io("create installation root", &error))?;
    let manifest = bundle.0.manifest();
    let directory = root
        .join(manifest.plugin().id().as_str())
        .join(manifest.plugin().version());
    if directory.exists() {
        return Err(bundle_error("plugin version is already installed"));
    }
    let parent = directory
        .parent()
        .ok_or_else(|| bundle_error("plugin installation path has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| bundle_io("create plugin directory", &error))?;
    let install_id = NEXT_INSTALL_ID.fetch_add(1, AtomicOrdering::Relaxed);
    let temp = parent.join(format!(
        ".{}-{}-{install_id}.part",
        manifest.plugin().version(),
        std::process::id()
    ));
    if temp.exists() {
        return Err(bundle_error("temporary plugin installation already exists"));
    }
    let result = (|| {
        fs::create_dir(&temp).map_err(|error| bundle_io("create temporary plugin", &error))?;
        for payload in bundle.0.payloads() {
            validate_payload_path(payload.path())?;
            let path = temp.join(payload.path());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| bundle_io("create payload directory", &error))?;
            }
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|error| bundle_io("create plugin payload", &error))?;
            file.write_all(payload.bytes())
                .map_err(|error| bundle_io("write plugin payload", &error))?;
            file.sync_all()
                .map_err(|error| bundle_io("sync plugin payload", &error))?;
            if payload.path() == manifest.executable_path() {
                make_executable(&path)?;
            }
        }
        fs::rename(&temp, &directory)
            .map_err(|error| bundle_io("publish plugin installation", &error))?;
        Ok::<(), SandboxError>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temp);
        return Err(error);
    }
    Ok(InstalledPlugin {
        command: directory.join(manifest.executable_path()),
        directory,
    })
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), SandboxError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(path)
        .map_err(|error| bundle_io("read executable permissions", &error))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| bundle_io("set executable permissions", &error))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), SandboxError> {
    Ok(())
}

/// Rotatable set of trusted official/development verification keys.
#[derive(Clone, Debug, Default)]
pub struct PluginTrustStore {
    keys: BTreeMap<Identifier, VerifyingKey>,
}

impl PluginTrustStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or rotates one key ID. Keeping the previous ID enables overlap.
    ///
    /// # Errors
    ///
    /// Rejects an invalid key identifier.
    pub fn trust(&mut self, id: &str, key: VerifyingKey) -> Result<(), SandboxError> {
        let id = Identifier::new(id).map_err(|error| bundle_error(error.to_string()))?;
        self.keys.insert(id, key);
        Ok(())
    }

    pub fn revoke(&mut self, id: &str) {
        self.keys.remove(id);
    }
}

/// Host compatibility and permission policy used before install/launch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginVerificationPolicy {
    target: String,
    application_version: String,
    plugin_api: PluginApiVersion,
    allowed_capabilities: BTreeSet<PluginCapability>,
}

impl PluginVerificationPolicy {
    /// Creates an explicit host policy.
    ///
    /// # Errors
    ///
    /// Rejects invalid target or version strings.
    pub fn new(
        target: &str,
        application_version: &str,
        plugin_api: PluginApiVersion,
        allowed_capabilities: impl IntoIterator<Item = PluginCapability>,
    ) -> Result<Self, SandboxError> {
        validate_version(application_version)?;
        if target.is_empty() || target.len() > 128 {
            return Err(bundle_error("verification target is invalid"));
        }
        Ok(Self {
            target: target.to_owned(),
            application_version: application_version.to_owned(),
            plugin_api,
            allowed_capabilities: allowed_capabilities.into_iter().collect(),
        })
    }
}

fn verify_compatibility(
    manifest: &PluginBundleManifest,
    policy: &PluginVerificationPolicy,
) -> Result<(), SandboxError> {
    if manifest.target != policy.target {
        return Err(SandboxError::BundleTargetMismatch {
            expected: policy.target.clone(),
            actual: manifest.target.clone(),
        });
    }
    if compare_versions(&policy.application_version, &manifest.minimum_app_version)?
        == std::cmp::Ordering::Less
    {
        return Err(SandboxError::BundleVersionIncompatible {
            required: manifest.minimum_app_version.clone(),
            actual: policy.application_version.clone(),
        });
    }
    let api = manifest.plugin.api_version();
    if api.major() != policy.plugin_api.major() || api.minor() > policy.plugin_api.minor() {
        return Err(SandboxError::BundleApiIncompatible {
            required_major: api.major(),
            required_minor: api.minor(),
        });
    }
    if !manifest
        .capabilities
        .is_subset(&policy.allowed_capabilities)
    {
        return Err(SandboxError::BundlePermissionDenied);
    }
    Ok(())
}

fn validate_payloads(payloads: &[PluginPayload]) -> Result<(), SandboxError> {
    if payloads.is_empty() || payloads.len() > MAX_PLUGIN_PAYLOADS {
        return Err(bundle_error("payload count is outside the bundle limit"));
    }
    let mut paths = BTreeSet::new();
    let mut bytes = 0_usize;
    for payload in payloads {
        validate_payload_path(&payload.path)?;
        if !paths.insert(payload.path.clone()) {
            return Err(bundle_error("payload paths are duplicated"));
        }
        bytes = bytes
            .checked_add(payload.bytes.len())
            .ok_or(SandboxError::BundleTooLarge { bytes: usize::MAX })?;
    }
    if bytes > MAX_PLUGIN_BUNDLE_BYTES {
        return Err(SandboxError::BundleTooLarge { bytes });
    }
    Ok(())
}

fn validate_payload_path(path: &Path) -> Result<(), SandboxError> {
    let text = path.to_string_lossy();
    if text.is_empty()
        || text.len() > MAX_PLUGIN_PAYLOAD_PATH_BYTES
        || text
            .bytes()
            .any(|byte| matches!(byte, 0 | b':' | b'\n' | b'\r'))
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SandboxError::UnsafeBundlePath);
    }
    Ok(())
}

fn canonical_manifest(
    manifest: &PluginBundleManifest,
    payloads: &[PluginPayload],
) -> Result<Vec<u8>, SandboxError> {
    let api = manifest.plugin.api_version();
    let sources = manifest
        .source_kinds
        .iter()
        .map(Identifier::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let capabilities = manifest
        .capabilities
        .iter()
        .map(|capability| capability.tag())
        .collect::<Vec<_>>()
        .join(",");
    let mut output = format!(
        "OBSRPBM1\n{}|{}|{}|{}|{}|{}|{}|{}|{}\n{sources}\n{capabilities}\n",
        manifest.plugin.id(),
        manifest.plugin.name(),
        manifest.plugin.version(),
        api.major(),
        api.minor(),
        manifest.target,
        manifest.minimum_app_version,
        manifest.signing_key_id,
        manifest.executable_path.to_string_lossy(),
    );
    for payload in payloads {
        use std::fmt::Write as _;
        writeln!(
            output,
            "{}:{}:{}",
            payload.path.to_string_lossy(),
            payload.bytes.len(),
            hex(&sha256(&payload.bytes))
        )
        .map_err(|_| bundle_error("canonical manifest formatting failed"))?;
    }
    if output.len() > MAX_CANONICAL_MANIFEST_BYTES {
        return Err(bundle_error("canonical manifest is too large"));
    }
    Ok(output.into_bytes())
}

type PayloadMetadata = (String, usize, [u8; 32]);

fn parse_canonical_manifest(
    canonical: &[u8],
) -> Result<(PluginBundleManifest, Vec<PayloadMetadata>), SandboxError> {
    let text = std::str::from_utf8(canonical)
        .map_err(|_| bundle_error("canonical manifest is not UTF-8"))?;
    let mut lines = text.lines();
    if lines.next() != Some("OBSRPBM1") {
        return Err(bundle_error("canonical manifest header is invalid"));
    }
    let fields = lines
        .next()
        .ok_or_else(|| bundle_error("canonical plugin record is missing"))?
        .split('|')
        .collect::<Vec<_>>();
    if fields.len() != 9 {
        return Err(bundle_error("canonical plugin record is invalid"));
    }
    let api_major = fields[3]
        .parse()
        .map_err(|_| bundle_error("plugin API major is invalid"))?;
    let api_minor = fields[4]
        .parse()
        .map_err(|_| bundle_error("plugin API minor is invalid"))?;
    let plugin = PluginManifest::with_api_version(
        fields[0],
        fields[1],
        fields[2],
        PluginApiVersion::new(api_major, api_minor),
    )?;
    let sources = lines
        .next()
        .ok_or_else(|| bundle_error("source record is missing"))?
        .split(',')
        .map(Identifier::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| bundle_error(error.to_string()))?;
    let capability_line = lines
        .next()
        .ok_or_else(|| bundle_error("capability record is missing"))?;
    let capabilities = if capability_line.is_empty() {
        Vec::new()
    } else {
        capability_line
            .split(',')
            .map(|value| {
                PluginCapability::parse(value)
                    .ok_or_else(|| bundle_error("plugin capability is invalid"))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let manifest = PluginBundleManifest::new(
        plugin,
        fields[5],
        fields[6],
        fields[7],
        fields[8],
        sources,
        capabilities,
    )?;
    let mut metadata = Vec::new();
    for line in lines {
        let (path, remainder) = line
            .split_once(':')
            .ok_or_else(|| bundle_error("payload metadata is invalid"))?;
        let (length, hash) = remainder
            .split_once(':')
            .ok_or_else(|| bundle_error("payload metadata is invalid"))?;
        let length = length
            .parse::<usize>()
            .map_err(|_| bundle_error("payload metadata length is invalid"))?;
        metadata.push((path.to_owned(), length, parse_hex_hash(hash)?));
    }
    Ok((manifest, metadata))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(*byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(*byte & 0x0f)]));
    }
    output
}

fn parse_hex_hash(value: &str) -> Result<[u8; 32], SandboxError> {
    if value.len() != 64 {
        return Err(bundle_error("payload hash length is invalid"));
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair =
            std::str::from_utf8(pair).map_err(|_| bundle_error("payload hash is invalid"))?;
        output[index] =
            u8::from_str_radix(pair, 16).map_err(|_| bundle_error("payload hash is invalid"))?;
    }
    Ok(output)
}

fn validate_version(version: &str) -> Result<(), SandboxError> {
    let components = version.split('.').collect::<Vec<_>>();
    if components.len() != 3
        || components
            .iter()
            .any(|component| component.is_empty() || component.parse::<u64>().is_err())
    {
        return Err(bundle_error(
            "application version must be numeric major.minor.patch",
        ));
    }
    Ok(())
}

fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, SandboxError> {
    validate_version(left)?;
    validate_version(right)?;
    let parse = |version: &str| {
        version
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(parse(left)
        .map_err(|_| bundle_error("application version is invalid"))?
        .cmp(&parse(right).map_err(|_| bundle_error("application version is invalid"))?))
}

fn u16_len(value: usize) -> Result<u16, SandboxError> {
    u16::try_from(value).map_err(|_| bundle_error("bundle field exceeds u16"))
}

fn u32_len(value: usize) -> Result<u32, SandboxError> {
    u32::try_from(value).map_err(|_| bundle_error("bundle field exceeds u32"))
}

fn u64_len(value: usize) -> Result<u64, SandboxError> {
    u64::try_from(value).map_err(|_| bundle_error("bundle field exceeds u64"))
}

fn bundle_error(reason: impl Into<String>) -> SandboxError {
    SandboxError::InvalidBundle {
        reason: reason.into(),
    }
}

fn bundle_io(operation: &str, error: &std::io::Error) -> SandboxError {
    SandboxError::BundleIo {
        operation: operation.to_owned(),
        message: error.to_string(),
    }
}

struct BundleCursor<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> BundleCursor<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], SandboxError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| bundle_error("plugin bundle is truncated"))?;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| bundle_error("plugin bundle is truncated"))?;
        self.offset = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, SandboxError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, SandboxError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, SandboxError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.offset)
    }
}
