#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_output_gstreamer::ProductionPipelinePlan;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = ProductionPipelinePlan::validate_serialized(text);
    }
});
