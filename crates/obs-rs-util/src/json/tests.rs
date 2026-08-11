use super::*;

#[test]
fn parses_every_value_kind() {
    let document = r#"{
      "null": null,
      "yes": true,
      "no": false,
      "count": -12,
      "ratio": 1.5e-3,
      "text": "hello",
      "list": [1, "two", [3]],
      "nested": { "inner": {} }
    }"#;

    let value = Json::parse(document).expect("valid document");

    assert_eq!(value.get("null"), Some(&Json::Null));
    assert_eq!(value.get("yes").and_then(Json::as_bool), Some(true));
    assert_eq!(value.get("no").and_then(Json::as_bool), Some(false));
    assert_eq!(value.get("count").and_then(Json::as_number::<i32>), Some(-12));
    assert_eq!(value.get("ratio").and_then(Json::as_number::<f64>), Some(0.0015));
    assert_eq!(value.get("text").and_then(Json::as_str), Some("hello"));
    assert_eq!(value.get("list").and_then(Json::as_array).map(<[Json]>::len), Some(3));
    assert!(value.get("nested").and_then(|inner| inner.get("inner")).is_some());
}

#[test]
fn keeps_large_integers_exact() {
    // Routing through f64 would round this to 9007199254740993's neighbour.
    let value = Json::parse(r#"{"id": 9007199254740993}"#).expect("valid document");

    assert_eq!(
        value.get("id").and_then(Json::as_number::<i64>),
        Some(9_007_199_254_740_993)
    );
}

#[test]
fn decodes_string_escapes_including_surrogate_pairs() {
    let value = Json::parse(r#""a\/b\\c\"d\n\tA😀""#).expect("valid document");

    assert_eq!(value.as_str(), Some("a/b\\c\"d\n\tA\u{1F600}"));
}

#[test]
fn writes_deterministic_sorted_output() {
    let value = Json::object([
        ("zeta", Json::number(2)),
        ("alpha", Json::string("one")),
        ("nested", Json::Array(vec![Json::Bool(true), Json::Null])),
        ("empty_list", Json::Array(Vec::new())),
        ("empty_map", Json::Object(BTreeMap::new())),
    ]);

    assert_eq!(
        value.to_pretty_string(),
        concat!(
            "{\n",
            "  \"alpha\": \"one\",\n",
            "  \"empty_list\": [],\n",
            "  \"empty_map\": {},\n",
            "  \"nested\": [\n",
            "    true,\n",
            "    null\n",
            "  ],\n",
            "  \"zeta\": 2\n",
            "}\n",
        )
    );
}

#[test]
fn round_trips_through_parse_and_write() {
    let original = Json::object([
        ("text", Json::string("control\u{1}\ttab \"quoted\" \\ back\n")),
        ("unicode", Json::string("emoji \u{1F600} and ünïcode")),
        ("number", Json::number(i64::MIN)),
    ]);

    let document = original.to_pretty_string();
    let reparsed = Json::parse(&document).expect("written document re-parses");

    assert_eq!(reparsed, original);
    assert_eq!(reparsed.to_pretty_string(), document);
}

#[test]
fn rejects_malformed_documents() {
    for document in [
        "{",
        "{\"a\": }",
        "{\"a\" 1}",
        "{\"a\": 1,}",
        "[1, 2",
        "\"unterminated",
        "{\"a\": 01}",
        "nul",
        "{} trailing",
        "{\"a\": 1.}",
        "{\"a\": 1e}",
    ] {
        assert!(
            Json::parse(document).is_err(),
            "expected `{document}` to be rejected"
        );
    }
}

#[test]
fn rejects_duplicate_member_names() {
    let error = Json::parse(r#"{"a": 1, "a": 2}"#).expect_err("duplicate keys are rejected");

    assert!(error.message.contains("duplicate"), "{error}");
}

#[test]
fn rejects_raw_control_characters_in_strings() {
    assert!(Json::parse("\"line\nbreak\"").is_err());
}

#[test]
fn reports_the_line_a_failure_was_found_on() {
    let error = Json::parse("{\n  \"a\": 1,\n  \"b\": oops\n}").expect_err("invalid value");

    assert_eq!(error.line, 3, "{error}");
}

#[test]
fn rejects_documents_that_nest_past_the_depth_cap() {
    let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));

    let error = Json::parse(&deep).expect_err("depth is capped");
    assert!(error.message.contains("deeply"), "{error}");
}

#[test]
fn accepts_nesting_up_to_the_depth_cap() {
    let deep = format!("{}1{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));

    assert!(Json::parse(&deep).is_ok());
}
