#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_sandbox::{SandboxedPluginManifest, SignedPluginBundle};

fuzz_target!(|data: &[u8]| {
    let _ = SignedPluginBundle::decode(data);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = SandboxedPluginManifest::parse(text);
    }
});
