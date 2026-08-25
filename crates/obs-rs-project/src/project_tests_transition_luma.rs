use super::*;
use obs_rs_media::{LumaWipePattern, TransitionKind, TransitionSpec};

#[test]
fn luma_wipe_transition_round_trips_pattern_softness_and_inversion() {
    let mut project = project();
    let transition = TransitionSpec::luma_wipe(725, LumaWipePattern::LinearVertical, true, 85)
        .expect("luma wipe transition");
    project
        .apply(ProjectCommand::SetSceneTransitionOverride {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            transition: Some(transition),
        })
        .expect("set luma wipe transition");

    let encoded = project.serialize();
    assert!(encoded.contains(r#""kind": "luma_wipe""#), "{encoded}");
    assert!(encoded.contains(r#""pattern": "linear-v""#), "{encoded}");
    assert!(encoded.contains(r#""invert": true"#), "{encoded}");
    assert!(encoded.contains(r#""softness_milli": 85"#), "{encoded}");

    let decoded = Project::parse(&encoded).expect("luma wipe project parses");
    assert_eq!(decoded, project);
    assert_eq!(
        decoded
            .profile("live")
            .and_then(|profile| profile.scene("main"))
            .and_then(SceneSpec::transition_override)
            .map(TransitionSpec::kind),
        Some(TransitionKind::LumaWipe {
            pattern: LumaWipePattern::LinearVertical,
            invert: true,
            softness_milli: 85,
        })
    );
}

#[test]
fn luma_wipe_decode_rejects_unknown_pattern_and_out_of_range_softness() {
    let unknown_pattern = project().serialize().replace(
        r#""transition": null"#,
        r#""transition": {"kind": "luma_wipe", "duration_ms": 300, "pattern": "burst", "softness_milli": 30}"#,
    );
    assert!(Project::parse(&unknown_pattern).is_err());

    let invalid_softness = project().serialize().replace(
        r#""transition": null"#,
        r#""transition": {"kind": "luma_wipe", "duration_ms": 300, "pattern": "linear-h", "softness_milli": 1001}"#,
    );
    assert!(Project::parse(&invalid_softness).is_err());
}
