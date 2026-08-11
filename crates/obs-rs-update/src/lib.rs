//! Signed, bounded application updates with atomic activation and rollback.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const UPDATE_MANIFEST_MAGIC: &str = "OBSRUPDATE1";
pub const MAX_UPDATE_ARTIFACT_BYTES: u64 = 2 * 1_024 * 1_024 * 1_024;
pub const MAX_UPDATE_MANIFEST_BYTES: usize = 16 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateError {
    InvalidManifest(String),
    ManifestTooLarge,
    UnknownSigningKey,
    InvalidSignature,
    TargetMismatch { expected: String, actual: String },
    VersionIncompatible { required: String, actual: String },
    ArtifactSizeMismatch { expected: u64, actual: u64 },
    ArtifactHashMismatch,
    AlreadyStaged,
    HealthCheckFailed(String),
    Io { operation: String, message: String },
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(reason) => write!(formatter, "invalid update manifest: {reason}"),
            Self::ManifestTooLarge => formatter.write_str("update manifest is too large"),
            Self::UnknownSigningKey => formatter.write_str("update signing key is not trusted"),
            Self::InvalidSignature => formatter.write_str("update signature is invalid"),
            Self::TargetMismatch { expected, actual } => {
                write!(
                    formatter,
                    "update target {actual} does not match {expected}"
                )
            }
            Self::VersionIncompatible { required, actual } => {
                write!(
                    formatter,
                    "update requires {required}; current version is {actual}"
                )
            }
            Self::ArtifactSizeMismatch { expected, actual } => {
                write!(
                    formatter,
                    "update size is {actual} bytes; expected {expected}"
                )
            }
            Self::ArtifactHashMismatch => formatter.write_str("update artifact hash is invalid"),
            Self::AlreadyStaged => formatter.write_str("update version is already staged"),
            Self::HealthCheckFailed(reason) => {
                write!(formatter, "update health check failed: {reason}")
            }
            Self::Io { operation, message } => write!(formatter, "update {operation}: {message}"),
        }
    }
}

impl std::error::Error for UpdateError {}

/// Signed update metadata. Debug output intentionally omits the artifact URL.
#[derive(Clone, Eq, PartialEq)]
pub struct SignedUpdateManifest {
    artifact_url: String,
    target: String,
    version: String,
    size: u64,
    sha256: [u8; 32],
    minimum_version: String,
    signing_key_id: String,
    signature: [u8; 64],
}

impl fmt::Debug for SignedUpdateManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedUpdateManifest")
            .field("artifact_url", &"[REDACTED]")
            .field("target", &self.target)
            .field("version", &self.version)
            .field("size", &self.size)
            .field("minimum_version", &self.minimum_version)
            .field("signing_key_id", &self.signing_key_id)
            .finish_non_exhaustive()
    }
}

impl SignedUpdateManifest {
    /// Creates and signs canonical update metadata.
    ///
    /// # Errors
    ///
    /// Rejects invalid URLs, targets, versions, key IDs, and artifact bounds.
    pub fn sign(
        artifact_url: &str,
        target: &str,
        version: &str,
        artifact: &[u8],
        minimum_version: &str,
        signing_key_id: &str,
        key: &SigningKey,
    ) -> Result<Self, UpdateError> {
        validate_fields(
            artifact_url,
            target,
            version,
            artifact.len() as u64,
            minimum_version,
            signing_key_id,
        )?;
        let mut manifest = Self {
            artifact_url: artifact_url.to_owned(),
            target: target.to_owned(),
            version: version.to_owned(),
            size: artifact.len() as u64,
            sha256: sha256(artifact),
            minimum_version: minimum_version.to_owned(),
            signing_key_id: signing_key_id.to_owned(),
            signature: [0; 64],
        };
        manifest.signature = key.sign(&manifest.canonical()).to_bytes();
        Ok(manifest)
    }

    #[must_use]
    pub fn artifact_url(&self) -> &str {
        &self.artifact_url
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Serializes the canonical manifest plus detached signature.
    #[must_use]
    pub fn serialize(&self) -> String {
        format!(
            "{}signature={}\n",
            String::from_utf8_lossy(&self.canonical()),
            hex(&self.signature)
        )
    }

    /// Parses bounded signed metadata without verifying trust.
    ///
    /// # Errors
    ///
    /// Rejects malformed, oversized, or non-canonical input.
    pub fn parse(document: &str) -> Result<Self, UpdateError> {
        if document.len() > MAX_UPDATE_MANIFEST_BYTES {
            return Err(UpdateError::ManifestTooLarge);
        }
        let mut lines = document.lines();
        if lines.next() != Some(UPDATE_MANIFEST_MAGIC) {
            return Err(invalid("header is invalid"));
        }
        let artifact_url = field(&mut lines, "url")?;
        let target = field(&mut lines, "target")?;
        let version = field(&mut lines, "version")?;
        let size = field(&mut lines, "size")?
            .parse::<u64>()
            .map_err(|_| invalid("size is invalid"))?;
        let sha256 = parse_hex::<32>(&field(&mut lines, "sha256")?)?;
        let minimum_version = field(&mut lines, "minimum_version")?;
        let signing_key_id = field(&mut lines, "key")?;
        let signature = parse_hex::<64>(&field(&mut lines, "signature")?)?;
        if lines.next().is_some() {
            return Err(invalid("trailing records"));
        }
        validate_fields(
            &artifact_url,
            &target,
            &version,
            size,
            &minimum_version,
            &signing_key_id,
        )?;
        Ok(Self {
            artifact_url,
            target,
            version,
            size,
            sha256,
            minimum_version,
            signing_key_id,
            signature,
        })
    }

    fn canonical(&self) -> Vec<u8> {
        format!(
            "{UPDATE_MANIFEST_MAGIC}\nurl={}\ntarget={}\nversion={}\nsize={}\nsha256={}\nminimum_version={}\nkey={}\n",
            self.artifact_url,
            self.target,
            self.version,
            self.size,
            hex(&self.sha256),
            self.minimum_version,
            self.signing_key_id,
        )
        .into_bytes()
    }
}

#[derive(Clone, Debug, Default)]
pub struct UpdateTrustStore {
    keys: BTreeMap<String, VerifyingKey>,
}

impl UpdateTrustStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or rotates a trusted key ID.
    ///
    /// # Errors
    ///
    /// Rejects unsafe key IDs.
    pub fn trust(&mut self, id: &str, key: VerifyingKey) -> Result<(), UpdateError> {
        validate_token(id, "key ID")?;
        self.keys.insert(id.to_owned(), key);
        Ok(())
    }

    pub fn revoke(&mut self, id: &str) {
        self.keys.remove(id);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedUpdate(SignedUpdateManifest);

impl VerifiedUpdate {
    #[must_use]
    pub const fn manifest(&self) -> &SignedUpdateManifest {
        &self.0
    }
}

/// Verifies signature, target, minimum version, size, and hash before staging.
///
/// # Errors
///
/// Returns a typed policy failure before any filesystem mutation.
pub fn verify_update(
    manifest: &SignedUpdateManifest,
    artifact: &[u8],
    trust: &UpdateTrustStore,
    target: &str,
    current_version: &str,
) -> Result<VerifiedUpdate, UpdateError> {
    if manifest.target != target {
        return Err(UpdateError::TargetMismatch {
            expected: target.to_owned(),
            actual: manifest.target.clone(),
        });
    }
    if compare_versions(current_version, &manifest.minimum_version)? == std::cmp::Ordering::Less {
        return Err(UpdateError::VersionIncompatible {
            required: manifest.minimum_version.clone(),
            actual: current_version.to_owned(),
        });
    }
    let actual = artifact.len() as u64;
    if actual != manifest.size {
        return Err(UpdateError::ArtifactSizeMismatch {
            expected: manifest.size,
            actual,
        });
    }
    if sha256(artifact) != manifest.sha256 {
        return Err(UpdateError::ArtifactHashMismatch);
    }
    let key = trust
        .keys
        .get(&manifest.signing_key_id)
        .ok_or(UpdateError::UnknownSigningKey)?;
    key.verify(
        &manifest.canonical(),
        &Signature::from_bytes(&manifest.signature),
    )
    .map_err(|_| UpdateError::InvalidSignature)?;
    Ok(VerifiedUpdate(manifest.clone()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedUpdate {
    version: String,
    artifact: PathBuf,
}

impl StagedUpdate {
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn artifact(&self) -> &Path {
        &self.artifact
    }
}

/// Filesystem manager using immutable release directories and atomic pointers.
#[derive(Clone, Debug)]
pub struct UpdateManager {
    root: PathBuf,
}

impl UpdateManager {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Stages a verified artifact atomically under its version.
    ///
    /// # Errors
    ///
    /// Refuses to overwrite an existing staged release.
    pub fn stage(
        &self,
        update: &VerifiedUpdate,
        artifact: &[u8],
    ) -> Result<StagedUpdate, UpdateError> {
        if artifact.len() as u64 != update.0.size || sha256(artifact) != update.0.sha256 {
            return Err(UpdateError::ArtifactHashMismatch);
        }
        let releases = self.root.join("releases");
        fs::create_dir_all(&releases).map_err(|error| io_error("create releases", &error))?;
        let final_dir = releases.join(&update.0.version);
        if final_dir.exists() {
            return Err(UpdateError::AlreadyStaged);
        }
        let temp_dir = releases.join(format!(".{}.part", update.0.version));
        if temp_dir.exists() {
            return Err(UpdateError::AlreadyStaged);
        }
        let result = (|| {
            fs::create_dir(&temp_dir).map_err(|error| io_error("create staged update", &error))?;
            let artifact_path = temp_dir.join("artifact");
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&artifact_path)
                .map_err(|error| io_error("create staged artifact", &error))?;
            file.write_all(artifact)
                .map_err(|error| io_error("write staged artifact", &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync staged artifact", &error))?;
            fs::rename(&temp_dir, &final_dir)
                .map_err(|error| io_error("publish staged update", &error))?;
            Ok::<(), UpdateError>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(error);
        }
        Ok(StagedUpdate {
            version: update.0.version.clone(),
            artifact: final_dir.join("artifact"),
        })
    }

    /// Atomically activates a staged release and rolls back its pointer when
    /// the supplied startup/health check fails.
    ///
    /// # Errors
    ///
    /// Returns the health failure after restoring the previous active version.
    pub fn activate_with_health_check(
        &self,
        staged: &StagedUpdate,
        health_check: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), UpdateError> {
        let previous = fs::read_to_string(self.root.join("active"))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let Some(previous) = &previous {
            write_atomic_text(&self.root.join("previous"), previous)?;
        }
        write_atomic_text(&self.root.join("active"), &staged.version)?;
        if let Err(reason) = health_check(&staged.artifact) {
            if let Some(previous) = previous {
                write_atomic_text(&self.root.join("active"), &previous)?;
            } else {
                let active = self.root.join("active");
                if let Err(error) = fs::remove_file(active) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        return Err(io_error("remove failed active pointer", &error));
                    }
                }
            }
            return Err(UpdateError::HealthCheckFailed(reason));
        }
        Ok(())
    }

    #[must_use]
    pub fn active_version(&self) -> Option<String> {
        fs::read_to_string(self.root.join("active"))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }
}

fn write_atomic_text(path: &Path, value: &str) -> Result<(), UpdateError> {
    let temp = path.with_extension("part");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| io_error("create pointer", &error))?;
    file.write_all(value.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| io_error("write pointer", &error))?;
    file.sync_all()
        .map_err(|error| io_error("sync pointer", &error))?;
    fs::rename(temp, path).map_err(|error| io_error("publish pointer", &error))
}

fn validate_fields(
    url: &str,
    target: &str,
    version: &str,
    size: u64,
    minimum_version: &str,
    key_id: &str,
) -> Result<(), UpdateError> {
    if !url.starts_with("https://")
        || url.len() > 2_048
        || url.bytes().any(|byte| matches!(byte, b'\n' | b'\r'))
    {
        return Err(invalid("artifact URL must be bounded HTTPS"));
    }
    validate_token(target, "target")?;
    validate_token(key_id, "key ID")?;
    validate_version(version)?;
    validate_version(minimum_version)?;
    if size == 0 || size > MAX_UPDATE_ARTIFACT_BYTES {
        return Err(invalid("artifact size is outside the update limit"));
    }
    Ok(())
}

fn validate_token(value: &str, field: &str) -> Result<(), UpdateError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid(format!("{field} is invalid")));
    }
    Ok(())
}

fn validate_version(value: &str) -> Result<(), UpdateError> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.parse::<u64>().is_err()) {
        return Err(invalid("version must be numeric major.minor.patch"));
    }
    Ok(())
}

fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, UpdateError> {
    validate_version(left)?;
    validate_version(right)?;
    let parse = |value: &str| {
        value
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(parse(left)
        .map_err(|_| invalid("version is invalid"))?
        .cmp(&parse(right).map_err(|_| invalid("version is invalid"))?))
}

fn field<'a>(lines: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<String, UpdateError> {
    lines
        .next()
        .and_then(|line| line.strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{name} record is missing")))
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

fn parse_hex<const N: usize>(value: &str) -> Result<[u8; N], UpdateError> {
    if value.len() != N * 2 {
        return Err(invalid("hex field length is invalid"));
    }
    let mut output = [0; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(pair).map_err(|_| invalid("hex field is invalid"))?;
        output[index] =
            u8::from_str_radix(pair, 16).map_err(|_| invalid("hex field is invalid"))?;
    }
    Ok(output)
}

fn invalid(reason: impl Into<String>) -> UpdateError {
    UpdateError::InvalidManifest(reason.into())
}

fn io_error(operation: &str, error: &std::io::Error) -> UpdateError {
    UpdateError::Io {
        operation: operation.to_owned(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> (Vec<u8>, SignedUpdateManifest, UpdateTrustStore) {
        let artifact = b"signed release artifact".to_vec();
        let key = SigningKey::from_bytes(&[9; 32]);
        let manifest = SignedUpdateManifest::sign(
            "https://updates.example.invalid/obs-rs-0.2.0.tar.zst",
            "x86_64-unknown-linux-gnu",
            "0.2.0",
            &artifact,
            "0.1.0",
            "official-2026",
            &key,
        )
        .expect("manifest");
        let mut trust = UpdateTrustStore::new();
        trust
            .trust("official-2026", key.verifying_key())
            .expect("trust");
        (artifact, manifest, trust)
    }

    #[test]
    fn manifest_round_trips_verifies_and_redacts_url() {
        let (artifact, manifest, trust) = fixture();
        let parsed = SignedUpdateManifest::parse(&manifest.serialize()).expect("parse");
        assert_eq!(parsed, manifest);
        assert!(!format!("{parsed:?}").contains("updates.example"));
        assert!(verify_update(
            &parsed,
            &artifact,
            &trust,
            "x86_64-unknown-linux-gnu",
            "0.1.0"
        )
        .is_ok());
        let mut tampered = artifact;
        tampered[0] ^= 1;
        assert_eq!(
            verify_update(
                &parsed,
                &tampered,
                &trust,
                "x86_64-unknown-linux-gnu",
                "0.1.0"
            ),
            Err(UpdateError::ArtifactHashMismatch)
        );
    }

    #[test]
    fn staging_is_atomic_and_failed_health_checks_roll_back() {
        let (artifact, manifest, trust) = fixture();
        let verified = verify_update(
            &manifest,
            &artifact,
            &trust,
            "x86_64-unknown-linux-gnu",
            "0.1.0",
        )
        .expect("verify");
        let root = std::env::temp_dir().join(format!(
            "obs-rs-update-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("root");
        write_atomic_text(&root.join("active"), "0.1.0").expect("old active");
        let manager = UpdateManager::new(&root);
        let staged = manager.stage(&verified, &artifact).expect("stage");
        assert_eq!(fs::read(staged.artifact()).expect("artifact"), artifact);
        assert_eq!(
            manager.activate_with_health_check(&staged, |_| Err("startup failed".to_owned())),
            Err(UpdateError::HealthCheckFailed("startup failed".to_owned()))
        );
        assert_eq!(manager.active_version().as_deref(), Some("0.1.0"));
        manager
            .activate_with_health_check(&staged, |_| Ok(()))
            .expect("activate");
        assert_eq!(manager.active_version().as_deref(), Some("0.2.0"));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
