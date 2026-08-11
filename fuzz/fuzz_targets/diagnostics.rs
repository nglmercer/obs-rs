#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_diagnostics::DiagnosticBundle;

fuzz_target!(|data: &[u8]| {
    let _ = DiagnosticBundle::decode(data);
});
