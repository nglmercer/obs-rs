#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_config::Config;
use obs_rs_project::Project;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = Config::parse(text);
        let _ = Project::parse(text);
    }
});
