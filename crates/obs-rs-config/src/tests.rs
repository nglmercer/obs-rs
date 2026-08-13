use super::*;

#[test]
fn parses_comments_and_serializes_in_key_order() {
    let config = Config::parse("# comment\nzeta = \"2\"\nalpha = \"1\"\n").expect("valid config");

    assert_eq!(config.len(), 2);
    assert_eq!(config.get("alpha"), Some("1"));
    assert_eq!(config.serialize(), "alpha = 1\nzeta = 2\n");
    assert_eq!(
        config.iter().collect::<Vec<_>>(),
        vec![("alpha", "1"), ("zeta", "2")]
    );
}

#[test]
fn tolerates_whitespace_around_the_separator() {
    // The previous line format split on `=` before trimming, so a padded key
    // was rejected as an invalid identifier.
    let config = Config::parse("  alpha   =   \"1\"  \n").expect("valid config");

    assert_eq!(config.get("alpha"), Some("1"));
}

#[test]
fn parses_values_that_contain_equals_and_hashes() {
    let config = Config::parse("url = \"https://example.test?a=b#frag\"\n").expect("valid config");

    assert_eq!(config.get("url"), Some("https://example.test?a=b#frag"));
}

#[test]
fn strips_trailing_comments_outside_strings() {
    let config =
        Config::parse("alpha = \"one\" # trailing\nbeta = 2 # also\n").expect("valid config");

    assert_eq!(config.get("alpha"), Some("one"));
    assert_eq!(config.get("beta"), Some("2"));
}

#[test]
fn preserves_significant_whitespace_inside_values() {
    // Whitespace inside the quotes is data; a value can no longer drift by
    // picking up or shedding padding across a round trip.
    let mut config = Config::new();
    config.set("alpha", "  padded  ").expect("valid entry");

    let document = config.serialize();
    assert_eq!(document, "alpha = \"  padded  \"\n");
    assert_eq!(
        Config::parse(&document).expect("valid config").get("alpha"),
        Some("  padded  ")
    );
}

#[test]
fn round_trips_values_needing_escapes() {
    let mut config = Config::new();
    config
        .set("alpha", "line\nbreak\t\"quoted\" \\ back\u{1}")
        .expect("valid entry");

    let document = config.serialize();
    assert_eq!(
        document,
        "alpha = \"line\\nbreak\\t\\\"quoted\\\" \\\\ back\\u0001\"\n"
    );
    assert_eq!(
        Config::parse(&document).expect("valid config"),
        config,
        "serialize is a fixed point after one round trip"
    );
}

#[test]
fn writes_canonical_integers_and_booleans_bare() {
    let mut config = Config::new();
    config.set("count", "48000").expect("valid entry");
    config.set("enabled", "true").expect("valid entry");
    config.set("offset", "-7").expect("valid entry");
    config.set("zero", "0").expect("valid entry");
    // Leading zeros are not valid bare TOML integers, so this stays quoted.
    config.set("padded", "007").expect("valid entry");
    // Wider than i64, so it is data that merely looks numeric.
    config
        .set("huge", "99999999999999999999")
        .expect("valid entry");

    let document = config.serialize();
    assert_eq!(
        document,
        concat!(
            "count = 48000\n",
            "enabled = true\n",
            "huge = \"99999999999999999999\"\n",
            "offset = -7\n",
            "padded = \"007\"\n",
            "zero = 0\n",
        )
    );
    assert_eq!(Config::parse(&document).expect("valid config"), config);
}

#[test]
fn reads_literal_strings() {
    let config = Config::parse("path = 'C:\\raw\\path'\n").expect("valid config");

    assert_eq!(config.get("path"), Some("C:\\raw\\path"));
}

#[test]
fn rejects_malformed_and_duplicate_entries() {
    assert_eq!(
        Config::parse("missing_separator"),
        Err(ConfigError::InvalidLine { line: 1 })
    );
    assert_eq!(
        Config::parse("alpha = 1\nalpha = 2\n"),
        Err(ConfigError::DuplicateKey { line: 2 })
    );
    assert_eq!(
        Config::parse("alpha = \"unterminated\n"),
        Err(ConfigError::InvalidValue { line: 1 })
    );
    assert_eq!(
        Config::parse("alpha = \"one\" junk\n"),
        Err(ConfigError::InvalidValue { line: 1 })
    );
}

#[test]
fn rejects_toml_constructs_outside_the_supported_subset() {
    assert_eq!(
        Config::parse("[section]\n"),
        Err(ConfigError::UnsupportedConstruct { line: 1 })
    );
    assert_eq!(
        Config::parse("alpha = [1, 2]\n"),
        Err(ConfigError::UnsupportedConstruct { line: 1 })
    );
    assert_eq!(
        Config::parse("alpha = 1.5\n"),
        Err(ConfigError::UnsupportedConstruct { line: 1 })
    );
}

#[test]
fn rejects_invalid_keys_and_values() {
    assert!(matches!(
        Config::parse("1alpha = \"value\""),
        Err(ConfigError::InvalidKey { line: 1, .. })
    ));
    assert_eq!(
        Config::parse("alpha = \"bad\0value\""),
        Err(ConfigError::InvalidValue { line: 1 })
    );

    let long_value = "x".repeat(MAX_VALUE_BYTES + 1);
    assert_eq!(
        Config::parse(&format!("alpha = \"{long_value}\"")),
        Err(ConfigError::ValueTooLong { line: 1 })
    );
}

#[test]
fn rejects_documents_over_the_size_limit() {
    let oversized = "x".repeat(MAX_CONFIG_BYTES + 1);

    assert_eq!(Config::parse(&oversized), Err(ConfigError::InputTooLarge));
}

#[test]
fn programmatic_updates_replace_and_remove_values() {
    let mut config = Config::new();
    config.set("alpha", "one").expect("valid entry");
    config.set("alpha", "two").expect("replacement is valid");

    assert_eq!(config.len(), 1);
    assert_eq!(config.remove("alpha"), Some("two".to_owned()));
    assert!(config.is_empty());
}
