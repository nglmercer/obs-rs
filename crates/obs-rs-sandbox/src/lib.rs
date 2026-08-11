//! Bounded subprocess extensions for OBS-RS.
//!
//! The host launches extension processes directly and accepts only validated
//! manifests and bounded OBSFRM01 frame packets.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod bundle;
mod discovery;
mod error;
mod frame_reader;
mod manifest;
mod plugin;
mod process_source;
mod protocol;
mod settings;
mod validation;

#[cfg(test)]
mod tests;

pub use bundle::{
    PluginBundleManifest, PluginCapability, PluginPayload, PluginTrustStore,
    PluginVerificationPolicy, SignedPluginBundle, VerifiedPluginBundle, MAX_PLUGIN_BUNDLE_BYTES,
    MAX_PLUGIN_PAYLOADS, MAX_PLUGIN_PAYLOAD_PATH_BYTES, PLUGIN_BUNDLE_MAGIC,
};
pub use discovery::discover_sandbox_manifest;
pub use error::SandboxError;
pub use manifest::SandboxedPluginManifest;
pub use plugin::SandboxedPlugin;
pub use protocol::{
    MAX_SANDBOX_ARGUMENTS, MAX_SANDBOX_ARGUMENT_BYTES, MAX_SANDBOX_MANIFEST_BYTES,
    MAX_SANDBOX_QUEUED_FRAMES, MAX_SANDBOX_SOURCE_KINDS, SANDBOX_FRAME_DELIVERY_TIMEOUT,
    SANDBOX_MANIFEST_ARGUMENT, SANDBOX_MANIFEST_MAGIC,
};
