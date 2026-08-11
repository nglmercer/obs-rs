//! Checks that the sample settings file shipped at the repository root still
//! loads and is written in the canonical form this build produces.

use obs_rs_config::Config;

#[test]
fn repository_sample_settings_parse_and_are_canonical() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../obs-rs-settings.toml");
    let document = std::fs::read_to_string(path).expect("sample settings are present");

    let config = Config::parse(&document).expect("sample settings parse");

    assert_eq!(
        config.serialize(),
        document,
        "the sample settings file is not in canonical serialized form"
    );
}
