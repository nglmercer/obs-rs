use super::*;

#[test]
fn replay_buffer_encodes_only_while_active_and_saves_atomically() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    assert!(!engine.is_replay_buffer_active());
    assert_eq!(engine.replay_buffer_packet_count(), 0);

    let final_path =
        std::env::temp_dir().join(format!("obs-rs-replay-engine-{}.obsr", std::process::id()));
    engine
        .start_replay_buffer(4 * 1024 * 1024, Duration::from_secs(30))
        .expect("start replay buffer");
    assert!(engine.is_replay_buffer_active());
    assert_eq!(engine.replay_lifecycle(), OutputLifecycle::Running);
    assert_eq!(engine.replay_save_status(), &ReplaySaveStatus::Idle);
    for _ in 0..3 {
        engine.tick(None, Some("program")).expect("replay tick");
    }
    assert!(engine.replay_buffer_packet_count() >= 3);
    let bytes = engine
        .save_replay_buffer(&final_path)
        .expect("save replay buffer");
    assert!(bytes > 16);
    assert_eq!(
        engine.replay_save_status(),
        &ReplaySaveStatus::Saved { bytes }
    );
    let packets =
        obs_rs_output::MemoryMuxer::decode(&std::fs::read(&final_path).expect("read replay file"))
            .expect("decode replay file");
    assert!(!packets.is_empty());
    assert!(engine.is_replay_buffer_active());

    engine.stop_replay_buffer();
    assert!(!engine.is_replay_buffer_active());
    assert_eq!(engine.replay_lifecycle(), OutputLifecycle::Idle);
    assert_eq!(engine.replay_save_status(), &ReplaySaveStatus::Idle);
    assert!(matches!(
        engine.save_replay_buffer(&final_path),
        Err(EngineError::InvalidConfiguration(reason)) if reason.contains("not running")
    ));
    assert!(matches!(
        engine.replay_save_status(),
        ReplaySaveStatus::Failed { reason } if reason.contains("not running")
    ));
    std::fs::remove_file(final_path).expect("remove replay file");
}

#[test]
fn recording_contains_both_media_kinds_in_timestamp_order() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.obsr"));
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine.start_recording(&path).expect("recording");
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    let bytes = engine.finish_recording().expect("finalize");
    let persisted = std::fs::read(&path).expect("read recording");
    assert_eq!(persisted.len(), bytes);
    let packets = obs_rs_output::MemoryMuxer::decode(&persisted).expect("decode recording");
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == obs_rs_output::PacketKind::Video));
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == obs_rs_output::PacketKind::Audio));
    assert!(packets
        .windows(2)
        .all(|packets| packets[0].timestamp() <= packets[1].timestamp()));
    std::fs::remove_file(path).expect("remove recording");
}

#[test]
fn segmented_recording_publishes_numbered_packet_files() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("obs-rs-engine-segmented-{token}.obsr"));
    let policy =
        SegmentedRecordingPolicy::new(2_000_000, Duration::from_nanos(1), 4).expect("split policy");
    let stale = base.with_file_name(format!("obs-rs-engine-segmented-{token}-0002.obsr.part"));
    std::fs::write(&stale, [1, 2, 3]).expect("write stale segment artifact");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .start_segmented_recording(&base, policy)
        .expect("segmented recording");
    assert!(!stale.exists(), "startup removes stale segment artifact");
    for _ in 0..3 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    let bytes = engine.finish_recording().expect("finalize split recording");

    let paths: Vec<_> = (1..=3)
        .map(|index| {
            base.with_file_name(format!("obs-rs-engine-segmented-{token}-{index:04}.obsr"))
        })
        .collect();
    assert!(paths.iter().all(|path| path.is_file()));
    assert!(!base.exists(), "the base path is only a naming anchor");
    assert!(!base
        .with_file_name(format!("obs-rs-engine-segmented-{token}-0004.obsr"))
        .exists());
    let persisted_bytes: usize = paths
        .iter()
        .map(|path| {
            usize::try_from(std::fs::metadata(path).expect("segment metadata").len())
                .expect("segment size fits usize")
        })
        .sum();
    assert_eq!(persisted_bytes, bytes);
    for path in &paths {
        let packets =
            obs_rs_output::MemoryMuxer::decode(&std::fs::read(path).expect("read segment"))
                .expect("decode segment");
        assert!(packets.iter().any(|packet| {
            packet.kind() == obs_rs_output::PacketKind::Video && packet.is_keyframe()
        }));
        std::fs::remove_file(path).expect("remove segment");
    }
}

#[test]
fn recording_rejects_extensions_that_do_not_select_a_known_container() {
    let path = std::env::temp_dir().join("obs-rs-unknown-recording.bin");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let error = engine
        .start_recording(path)
        .expect_err("unknown extension must be rejected");
    assert!(error
        .to_string()
        .contains(".mkv, .mp4, .mov, .flv, or .obsr"));
    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn matroska_recording_uses_raw_production_media_and_publishes_atomically() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::MatroskaH264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mkv"));
    let temp_path = path.with_extension("mkv.part");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine.start_recording(&path).expect("Matroska recording");
    assert!(!path.exists(), "the final path stays hidden until EOS");
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine.finish_recording().expect("finalize Matroska");
    let persisted = std::fs::read(&path).expect("read Matroska");
    assert_eq!(persisted.len(), bytes);
    assert!(persisted.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]));
    assert!(!temp_path.exists());
    std::fs::remove_file(path).expect("remove recording");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn mp4_recording_uses_raw_production_media_and_publishes_atomically() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::Mp4H264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mp4"));
    let temp_path = path.with_extension("mp4.part");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine.start_recording(&path).expect("MP4 recording");
    assert!(!path.exists(), "the final path stays hidden until EOS");
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine.finish_recording().expect("finalize MP4");
    let persisted = std::fs::read(&path).expect("read MP4");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
    assert!(!temp_path.exists());
    std::fs::remove_file(path).expect("remove recording");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn remux_recording_publishes_mp4_after_consuming_hidden_matroska_source() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::MatroskaH264Aac)
        || !capabilities.supports_remux()
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-auto-remux-{token}.mp4"));
    let source_path = path.with_extension("mkv.part");
    let remux_temp_path = path.with_extension("mp4.part");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .start_remux_recording(&path)
        .expect("automatic remux recording");
    assert!(
        !path.exists(),
        "the final path stays hidden until remux EOS"
    );
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine.finish_recording().expect("finalize automatic remux");
    let persisted = std::fs::read(&path).expect("read remuxed MP4");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
    assert!(!source_path.exists());
    assert!(!remux_temp_path.exists());
    std::fs::remove_file(path).expect("remove recording");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn explicit_fragmented_mp4_profile_reaches_the_engine_recording_boundary() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::FragmentedMp4H264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mp4"));
    let temp_path = path.with_extension("mp4.part");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .start_recording_profile(&path, OutputProfile::fragmented_mp4_h264_aac())
        .expect("fragmented MP4 recording");
    assert!(!path.exists(), "the final path stays hidden until EOS");
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine.finish_recording().expect("finalize fragmented MP4");
    let persisted = std::fs::read(&path).expect("read fragmented MP4");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
    assert!(persisted.windows(4).any(|chunk| chunk == b"moof"));
    assert!(!temp_path.exists());
    std::fs::remove_file(path).expect("remove recording");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn segmented_mp4_recording_reaches_the_engine_native_boundary() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::Mp4H264Aac)
        || !capabilities.supports_segmented_recording()
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base_path = std::env::temp_dir().join(format!("obs-rs-engine-segmented-{token}.mp4"));
    let policy = SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_millis(500), 3)
        .expect("segment policy");
    let available = capabilities.capabilities();
    let video_encoder = available
        .video_encoders()
        .iter()
        .find(|encoder| encoder.codec() == obs_rs_output::VideoCodec::H264);
    let audio_encoder = available
        .audio_encoders()
        .iter()
        .find(|encoder| encoder.codec() == obs_rs_output::AudioCodec::Aac);
    let (Some(video_encoder), Some(audio_encoder)) = (video_encoder, audio_encoder) else {
        return;
    };
    let video_config = VideoEncoderConfig {
        implementation: obs_rs_output::EncoderImplementation::new(video_encoder.id()),
        bitrate_kbps: 2_000,
        ..VideoEncoderConfig::default()
    };
    let audio_config = AudioEncoderConfig {
        implementation: obs_rs_output::EncoderImplementation::new(audio_encoder.id()),
        bitrate_kbps: 160,
        ..AudioEncoderConfig::default()
    };
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .start_segmented_recording_configured(&base_path, policy, video_config, audio_config)
        .expect("segmented MP4 recording");
    for _ in 0..90 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine
        .finish_recording()
        .expect("finalize segmented MP4 recording");
    assert!(bytes > 0);

    let mut published = 0_usize;
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("base stem");
    for index in 1..=policy.max_segments() {
        let path = base_path.with_file_name(format!("{stem}-{index:05}.mp4"));
        if path.exists() {
            published = published.saturating_add(1);
            assert!(std::fs::metadata(&path).expect("segment metadata").len() > 0);
            std::fs::remove_file(path).expect("remove segment");
        }
        let temp = base_path.with_file_name(format!("{stem}-{index:05}.mp4.part"));
        assert!(!temp.exists(), "temporary segment must be cleaned");
    }
    assert!(published > 0, "engine must publish at least one segment");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn mov_recording_uses_raw_production_media_and_publishes_atomically() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::MovH264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.mov"));
    let temp_path = path.with_extension("mov.part");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine.start_recording(&path).expect("MOV recording");
    assert!(!path.exists(), "the final path stays hidden until EOS");
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine.finish_recording().expect("finalize MOV");
    let persisted = std::fs::read(&path).expect("read MOV");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
    assert!(!temp_path.exists());
    std::fs::remove_file(path).expect("remove recording");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn flv_recording_uses_raw_production_media_and_publishes_atomically() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::FlvH264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-{token}.flv"));
    let temp_path = path.with_extension("flv.part");
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine.start_recording(&path).expect("FLV recording");
    assert!(!path.exists(), "the final path stays hidden until EOS");
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("media tick");
    }
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let bytes = engine.finish_recording().expect("finalize FLV");
    let persisted = std::fs::read(&path).expect("read FLV");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(..3), Some(&b"FLV"[..]));
    assert!(!temp_path.exists());
    std::fs::remove_file(path).expect("remove recording");
}

#[test]
fn worker_accepts_frames_and_finalizes_on_its_own_thread() {
    let format = project()
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let worker = EngineWorker::spawn_with_capacity(session, 1).expect("worker");
    worker
        .sync_project(project())
        .expect("project sync while idle");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-worker-{token}.obsr"));
    worker.start_recording(&path).expect("recording");
    assert!(worker.try_push_frame(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255],)));
    let bytes = worker.finish_recording().expect("finalize");
    let packets =
        obs_rs_output::MemoryMuxer::decode(&std::fs::read(&path).expect("read recording"))
            .expect("decode recording");
    assert_eq!(packets.len(), 2);
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == obs_rs_output::PacketKind::Video));
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == obs_rs_output::PacketKind::Audio));
    assert!(bytes > 0);
    std::fs::remove_file(path).expect("remove recording");
}

#[test]
fn worker_accepts_segmented_recording_and_finalizes_numbered_files() {
    let format = project()
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let worker = EngineWorker::spawn_with_capacity(session, 4).expect("worker");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("obs-rs-engine-worker-segmented-{token}.obsr"));
    let policy =
        SegmentedRecordingPolicy::new(10_000, Duration::from_nanos(1), 3).expect("split policy");

    worker
        .start_segmented_recording(&base, policy)
        .expect("segmented recording");
    assert!(worker.try_push_frame(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255],)));
    assert!(worker.try_push_frame(VideoFrame::solid(
        format,
        Timestamp::from_millis(33),
        [4, 5, 6, 255],
    )));
    let bytes = worker.finish_recording().expect("finalize split recording");
    assert!(bytes > 0);

    for index in 1..=2 {
        let path = base.with_file_name(format!(
            "obs-rs-engine-worker-segmented-{token}-{index:04}.obsr"
        ));
        assert!(path.is_file(), "missing segment {index}");
        let packets =
            obs_rs_output::MemoryMuxer::decode(&std::fs::read(&path).expect("read segment"))
                .expect("decode segment");
        assert!(packets
            .iter()
            .any(|packet| packet.kind() == obs_rs_output::PacketKind::Video));
        std::fs::remove_file(path).expect("remove segment");
    }
}

#[test]
fn worker_publishes_monitor_levels_while_outputs_are_idle() {
    let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let worker = EngineWorker::spawn_with_capacity(session, 1).expect("worker");

    assert!(worker.try_monitor_audio(Timestamp::ZERO));
    // A blocking command is a queue barrier: its reply arrives only after
    // the preceding monitor sample has been processed and published.
    worker
        .set_channel_gain_milli(EngineAudioChannel::Microphone, 1_000)
        .expect("queue barrier");

    let snapshot = worker.snapshot();
    assert!(snapshot.engine.stats.microphone_peak_milli > 0);
    assert_eq!(snapshot.engine.stats.video_frames, 0);
    assert!(!snapshot.engine.recording && !snapshot.engine.streaming);
}

#[test]
fn a_recording_reports_every_phase_it_passes_through() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-phase-{token}.obsr"));
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Idle);

    engine.start_recording(&path).expect("recording");
    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Running);
    engine.tick(None, Some("program")).expect("media tick");

    engine.finish_recording().expect("finalize");
    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Idle);
    assert!(engine.recording_lifecycle().is_stopped());
    std::fs::remove_file(path).expect("remove recording");
}

#[test]
fn a_recording_that_cannot_be_opened_reports_failed_rather_than_idle() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");

    // An empty path names no file, so the writer cannot be built at all.
    engine
        .start_recording("")
        .expect_err("a path that names no file is rejected");

    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
    assert!(
        engine.recording_lifecycle().is_stopped(),
        "a frontend must treat a failed start as not recording"
    );
    assert!(
        engine.snapshot().last_error.is_some(),
        "the failure has to leave an explanation behind"
    );
    assert!(!engine.is_recording());
}

#[test]
fn a_recording_that_cannot_be_committed_stays_failed_and_keeps_its_stream() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("obs-rs-engine-commit-{token}"));
    let path = root.join("recording.obsr");
    std::fs::create_dir_all(&path).expect("create conflicting final directory");
    // The temporary stream can be opened beside the requested path, but a
    // regular file cannot atomically replace the directory at commit time.
    engine
        .start_recording(&path)
        .expect("the temporary packet stream opens");
    engine.tick(None, Some("program")).expect("media tick");

    engine
        .finish_recording()
        .expect_err("committing into a missing directory fails");

    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
    assert!(
        engine.is_recording(),
        "a failed commit must not discard the captured packet stream"
    );
    engine.abort_recording();
    std::fs::remove_dir(&path).expect("remove conflicting final directory");
    std::fs::remove_dir(root).expect("remove commit fixture root");
}

#[test]
fn a_stream_that_cannot_connect_reports_failed_and_can_be_retried() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");

    // Port 0 is never a listening peer, so the connect is refused.
    engine
        .start_streaming("127.0.0.1:0")
        .expect_err("an unreachable peer is rejected");

    assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Failed);
    assert!(!engine.is_streaming());

    // Stopping clears the failure so the next attempt starts from idle
    // rather than inheriting the previous one's phase.
    engine
        .finish_streaming()
        .expect("stop a stream that failed");
    assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Idle);
}

#[cfg(not(feature = "production-gstreamer"))]
#[test]
fn reference_only_build_rejects_persisted_production_stream_targets_clearly() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let target = obs_rs_output::StreamTarget::Hls(obs_rs_output::HlsConfig::default());

    let error = engine
        .start_streaming_target_configured(
            &target,
            &obs_rs_output::VideoEncoderConfig::default(),
            &obs_rs_output::AudioEncoderConfig::default(),
        )
        .expect_err("reference builds cannot open an HLS target");

    assert!(error.to_string().contains("production-gstreamer feature"));
    assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Failed);
}

#[cfg(feature = "production-gstreamer")]
#[test]
#[ignore = "requires a local native production sink; run on a reference output host"]
fn production_schemes_create_native_stream_outputs() {
    let video = project()
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let audio = EngineConfig::default().audio_format();
    for endpoint in [
        "rtmp://127.0.0.1:9/live/test",
        "rtmps://127.0.0.1:9/live/test",
        "srt://127.0.0.1:9",
        "rist://127.0.0.1:5000",
    ] {
        let mut stream = StreamOutput::connect(endpoint, 1_048_576, 1, video, audio, None)
            .expect("native production pipeline");
        assert!(matches!(stream, StreamOutput::Production(_)));
        assert_eq!(stream.video_requirement(), VideoInputRequirement::Raw);
        assert_eq!(stream.audio_requirement(), AudioInputRequirement::Raw);
        stream.close().expect("close live pipeline");
    }
}

#[cfg(feature = "production-gstreamer")]
#[test]
#[ignore = "requires a local native production sink; run on a reference output host"]
fn production_only_streams_skip_reference_encoders_and_receive_raw_media() {
    let frame = VideoFrame::solid(
        project()
            .active_profile_spec()
            .expect("profile")
            .video_format(),
        Timestamp::ZERO,
        [24, 96, 180, 255],
    );
    for endpoint in [
        "rtmp://127.0.0.1:9/live/test",
        "rtmps://127.0.0.1:9/live/test",
        "srt://127.0.0.1:9",
        "rist://127.0.0.1:5000",
    ] {
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        engine
            .start_streaming(endpoint)
            .expect("native production pipeline");

        engine
            .push_program_frame(&frame)
            .expect("raw media submission");

        assert_eq!(engine.reference_video_encode_calls, 0, "{endpoint}");
        assert_eq!(engine.reference_audio_encode_calls, 0, "{endpoint}");
        assert_eq!(engine.stats.video_encode_latency.samples(), 0, "{endpoint}");
        assert_eq!(engine.stats.audio_encode_latency.samples(), 0, "{endpoint}");
        assert_eq!(
            engine.stats.output_submit_latency.samples(),
            2,
            "{endpoint}"
        );
        let metrics = engine
            .snapshot()
            .production_stream_metrics
            .expect("production telemetry");
        assert_eq!(metrics.video_submitted, 1, "{endpoint}");
        assert_eq!(metrics.audio_submitted, 1, "{endpoint}");
        assert!(metrics.video_queue_bytes <= 1_048_576, "{endpoint}");
        assert!(metrics.audio_queue_bytes <= 1_048_576, "{endpoint}");
    }
}

#[test]
fn reference_recording_runs_reference_encoders_once_per_media_item() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-reference-only-{token}.obsr"));
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
    engine.start_recording(&path).expect("recording");

    engine
        .push_program_frame(&frame)
        .expect("packetized media submission");

    assert_eq!(engine.reference_video_encode_calls, 1);
    assert_eq!(engine.reference_audio_encode_calls, 1);
    assert_eq!(engine.stats.video_encode_latency.samples(), 1);
    assert_eq!(engine.stats.audio_encode_latency.samples(), 1);
    assert_eq!(engine.stats.output_submit_latency.samples(), 2);
    engine.finish_recording().expect("finalize recording");
    std::fs::remove_file(path).expect("remove recording");
}

#[test]
fn reference_tcp_and_websocket_streams_keep_packetized_encoding() {
    let policy = ReconnectPolicy::new(1);
    let streams = [
        StreamOutput::Tcp(
            StreamSession::new(
                TcpPacketTransport::new("127.0.0.1:9"),
                1_048_576,
                PacketDropPolicy::DropNewest,
                policy,
            )
            .expect("TCP stream"),
        ),
        StreamOutput::WebSocket(
            StreamSession::new(
                WebSocketPacketTransport::new("ws://127.0.0.1:9/live"),
                1_048_576,
                PacketDropPolicy::DropNewest,
                policy,
            )
            .expect("WebSocket stream"),
        ),
    ];

    for stream in streams {
        assert_eq!(
            stream.video_requirement(),
            VideoInputRequirement::Packetized
        );
        assert_eq!(
            stream.audio_requirement(),
            AudioInputRequirement::Packetized
        );
        let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
        let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
        engine.streaming = Some(stream);

        engine
            .push_program_frame(&frame)
            .expect("packetized media submission");

        assert_eq!(engine.reference_video_encode_calls, 1);
        assert_eq!(engine.reference_audio_encode_calls, 1);
        assert!(engine.snapshot().stream_queued_bytes > 0);
    }
}

#[test]
fn failed_media_output_releases_recording_and_stream_handles() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-output-recovery-{token}.obsr"));
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine.start_recording(&path).expect("recording");

    let mut stream = StreamSession::new(
        TcpPacketTransport::new("127.0.0.1:9"),
        1_048_576,
        PacketDropPolicy::DropNewest,
        ReconnectPolicy::new(1),
    )
    .expect("stream");
    stream.close();
    engine.streaming = Some(StreamOutput::Tcp(stream));
    engine.streaming_lifecycle = OutputLifecycle::Running;

    let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
    let error = engine
        .push_program_frame(&frame)
        .expect_err("closed stream must reject media");
    assert!(matches!(error, EngineError::Output(_)));
    assert!(engine.handle_media_error(&error));

    assert!(!engine.is_recording());
    assert!(!engine.is_streaming());
    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);
    assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Failed);
    assert!(!path.exists(), "failed output must not publish a recording");
}

#[test]
fn terminal_stream_health_failure_releases_stream_handle() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let mut stream = StreamSession::new(
        TcpPacketTransport::new("127.0.0.1:9"),
        1_048_576,
        PacketDropPolicy::DropNewest,
        ReconnectPolicy::new(1),
    )
    .expect("stream");
    stream.close();
    engine.streaming = Some(StreamOutput::Tcp(stream));
    engine.streaming_lifecycle = OutputLifecycle::Running;

    assert!(engine.pump_outputs().is_err());
    assert!(!engine.is_streaming());
    assert_eq!(engine.streaming_lifecycle(), OutputLifecycle::Failed);
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn reference_recording_and_rtmp_encode_once_and_submit_raw_once() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-mixed-output-{token}.obsr"));
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let frame = VideoFrame::solid(engine.format(), Timestamp::ZERO, [24, 96, 180, 255]);
    engine.start_recording(&path).expect("recording");
    engine
        .start_streaming("rtmp://127.0.0.1:9/live/test")
        .expect("native production pipeline");

    engine
        .push_program_frame(&frame)
        .expect("mixed media submission");

    assert_eq!(engine.reference_video_encode_calls, 1);
    assert_eq!(engine.reference_audio_encode_calls, 1);
    let metrics = engine
        .snapshot()
        .production_stream_metrics
        .expect("production telemetry");
    assert_eq!(metrics.video_submitted, 1);
    assert_eq!(metrics.audio_submitted, 1);
    engine.finish_recording().expect("finalize recording");
    std::fs::remove_file(path).expect("remove recording");
}

#[test]
fn aborting_a_recording_clears_a_previous_failure() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .start_recording("")
        .expect_err("a path that names no file is rejected");
    assert_eq!(engine.recording_lifecycle(), OutputLifecycle::Failed);

    engine.abort_recording();

    assert_eq!(
        engine.recording_lifecycle(),
        OutputLifecycle::Idle,
        "an explicit stop must not leave the session permanently broken"
    );
}

#[test]
fn a_dead_worker_reports_both_outputs_as_failed() {
    let session = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let worker = EngineWorker::spawn_with_capacity(session, 1).expect("worker");

    let snapshot = worker.snapshot();

    assert!(snapshot.alive);
    assert_eq!(snapshot.engine.recording_lifecycle, OutputLifecycle::Idle);
    assert_eq!(snapshot.engine.streaming_lifecycle, OutputLifecycle::Idle);
}
