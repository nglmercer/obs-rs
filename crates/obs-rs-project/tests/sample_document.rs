//! Checks that the sample project shipped at the repository root still loads
//! and is written in the canonical form this build produces.

use obs_rs_project::Project;

#[test]
fn repository_sample_project_parses_and_is_canonical() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../obs-rs-project.json");
    let document = std::fs::read_to_string(path).expect("sample project is present");

    let project = Project::parse(&document).expect("sample project parses");

    assert_eq!(
        project.serialize(),
        document,
        "the sample project is not in canonical serialized form"
    );
}
