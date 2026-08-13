use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_paths(label: &str) -> (PathBuf, PathBuf) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir();
    (
        root.join(format!(
            "obs-rs-{label}-{}-{timestamp}-{id}.diag",
            std::process::id()
        )),
        root.join(format!(
            "obs-rs-{label}-{}-{timestamp}-{id}.part",
            std::process::id()
        )),
    )
}

#[test]
fn bundles_are_sorted_bounded_and_round_trip() {
    let mut bundle = DiagnosticBundle::new();
    bundle
        .insert_text("z-runtime", "rendered=3")
        .expect("section");
    bundle
        .insert_bytes("a-project", &[1, 2, 3])
        .expect("section");
    let encoded = bundle.encode().expect("encode");
    let decoded = DiagnosticBundle::decode(&encoded).expect("decode");

    assert_eq!(decoded, bundle);
    assert_eq!(decoded.section_count(), 2);
    assert_eq!(decoded.section("a-project"), Some(&[1, 2, 3][..]));
    assert_eq!(
        decoded.sections().map(|(name, _)| name).collect::<Vec<_>>(),
        vec!["a-project", "z-runtime"]
    );
    assert_eq!(encoded.len(), bundle.encoded_len());
}

#[test]
fn invalid_sections_do_not_mutate_the_bundle() {
    let mut bundle = DiagnosticBundle::new();
    bundle.insert_text("valid", "one").expect("section");
    assert_eq!(
        bundle.insert_text("valid", "two"),
        Err(DiagnosticError::DuplicateSection("valid".to_owned()))
    );
    assert_eq!(
        bundle.insert_text("bad name", "three"),
        Err(DiagnosticError::InvalidSectionName)
    );
    assert_eq!(bundle.section_count(), 1);
    assert_eq!(bundle.section("valid"), Some(b"one".as_slice()));
}

#[test]
fn decoder_rejects_truncation_and_trailing_bytes() {
    let mut bundle = DiagnosticBundle::new();
    bundle.insert_text("runtime", "ok").expect("section");
    let encoded = bundle.encode().expect("encode");
    assert_eq!(
        DiagnosticBundle::decode(&encoded[..encoded.len() - 1]),
        Err(DiagnosticError::Truncated)
    );
    let mut trailing = encoded;
    trailing.push(0);
    assert_eq!(
        DiagnosticBundle::decode(&trailing),
        Err(DiagnosticError::TrailingBytes)
    );
}

#[test]
fn atomic_writer_commits_and_abort_removes_temporary_state() {
    let (final_path, temp_path) = test_paths("commit");
    let mut bundle = DiagnosticBundle::new();
    bundle.insert_text("runtime", "ok").expect("section");
    let mut writer = AtomicDiagnosticFileWriter::new(&final_path, &temp_path).expect("writer");
    let committed = writer.finalize(&bundle).expect("commit");
    assert_eq!(writer.state(), DiagnosticFileState::Finalized);
    assert_eq!(writer.committed_bytes(), Some(committed));
    assert_eq!(
        DiagnosticBundle::decode(&fs::read(&final_path).expect("read")),
        Ok(bundle)
    );
    assert!(!temp_path.exists());
    fs::remove_file(final_path).expect("cleanup final");

    let (final_path, temp_path) = test_paths("abort");
    let mut writer = AtomicDiagnosticFileWriter::new(&final_path, &temp_path).expect("writer");
    fs::write(&temp_path, b"uncommitted").expect("temporary fixture");
    writer.abort().expect("abort");
    assert_eq!(writer.state(), DiagnosticFileState::Aborted);
    assert!(!temp_path.exists());
    assert!(!final_path.exists());
}

#[test]
fn diagnostics_redact_plugin_update_streaming_and_portal_secrets() {
    let input = concat!(
        "renderer=cpu\n",
        "plugin_path=/home/user/private/plugin\n",
        "update_credentials=bearer-token\n",
        "srt_passphrase=hunter2\n",
        "webrtc_signaling=wss://user:secret@example.invalid\n",
        "whip_bearer_token=whip-secret\n",
        "restore_token=portal-secret\n",
        "custom_password:secret\n",
    );
    let redacted = redact_diagnostics_text(input);
    assert!(redacted.contains("renderer=cpu"));
    for secret in [
        "/home/user/private/plugin",
        "bearer-token",
        "hunter2",
        "user:secret",
        "portal-secret",
        "whip-secret",
        "custom_password:secret",
    ] {
        assert!(!redacted.contains(secret));
    }
    assert_eq!(format!("{:?}", Redacted::new("secret")), REDACTED);
}
