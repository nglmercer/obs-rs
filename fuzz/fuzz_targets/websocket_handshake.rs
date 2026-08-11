#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_output::validate_websocket_handshake;

fuzz_target!(|data: &[u8]| {
    let _ = validate_websocket_handshake(data, "dGhlIHNhbXBsZSBub25jZQ==");
});
