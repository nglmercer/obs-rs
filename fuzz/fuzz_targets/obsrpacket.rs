#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_output::MemoryMuxer;

fuzz_target!(|data: &[u8]| {
    let _ = MemoryMuxer::decode(data);
});
