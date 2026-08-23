use super::*;

#[test]
fn output_runtime_finalizes_an_atomic_av_recording() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-gui-output-{token}.obsr"));
    let mut output = OutputRuntime::new(format);
    output
        .start_recording(final_path.to_str().expect("UTF-8 temp path"))
        .expect("recording should open");
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [20, 30, 40, 255]);
    output.push_frame(&frame);
    let bytes = output
        .finish_recording()
        .expect("recording should finalize");
    assert!(bytes > 0);
    let persisted = std::fs::read(&final_path).expect("recording should be persisted");
    assert_eq!(persisted.len(), bytes);
    let packets = MemoryMuxer::decode(&persisted).expect("packet recording should decode");
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Video));
    assert!(packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Audio));
    std::fs::remove_file(final_path).expect("remove output fixture");
}

#[test]
fn output_runtime_routes_fragmented_mp4_settings_to_the_explicit_profile() {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-gui-fragmented-{token}.mp4"));
    let mut output = OutputRuntime::new(format);
    if !output
        .capabilities()
        .recording_formats()
        .contains(&OutputProfileKind::FragmentedMp4H264Aac)
    {
        return;
    }
    let settings = AppSettings {
        recording_format: crate::settings::RecordingFormat::FragmentedMp4,
        ..AppSettings::default()
    };
    output.configure_stream(&settings);
    output.configure_recording(&settings);
    assert_eq!(
        output.recording_profile_kind(),
        Some(OutputProfileKind::FragmentedMp4H264Aac)
    );
    output
        .start_recording(final_path.to_str().expect("UTF-8 temp path"))
        .expect("fragmented MP4 recording should open");
    for timestamp in [0, 33_333_333, 66_666_666, 99_999_999] {
        let frame = VideoFrame::solid(format, Timestamp::from_nanos(timestamp), [20, 30, 40, 255]);
        output.push_frame(&frame);
    }
    let bytes = output
        .finish_recording()
        .expect("fragmented MP4 recording should finalize");
    let persisted = std::fs::read(&final_path).expect("fragmented MP4 should be persisted");
    assert_eq!(persisted.len(), bytes);
    assert!(persisted.windows(4).any(|chunk| chunk == b"moof"));
    std::fs::remove_file(final_path).expect("remove fragmented output fixture");
}

#[test]
fn output_runtime_routes_matroska_auto_remux_to_the_mp4_final_path() {
    let format = VideoFormat::new(64, 64, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    if !output.capabilities().supports_remux() {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-gui-remux-{token}.mp4"));
    let settings = AppSettings {
        recording_auto_remux: true,
        ..AppSettings::default()
    };
    output.configure_stream(&settings);
    output.configure_recording(&settings);
    output
        .start_recording(final_path.to_str().expect("UTF-8 temp path"))
        .expect("automatic remux recording should open");
    for index in 0..90 {
        let timestamp = u64::try_from(index).expect("index") * 33_333_333;
        output.push_frame(&VideoFrame::solid(
            format,
            Timestamp::from_nanos(timestamp),
            [20, 30, 40, 255],
        ));
    }
    let bytes = output
        .finish_recording()
        .expect("automatic remux recording should finalize");
    assert!(bytes > 0);
    let persisted = std::fs::read(&final_path).expect("remuxed MP4 should be persisted");
    assert_eq!(persisted.len(), bytes);
    assert!(persisted.windows(4).any(|chunk| chunk == b"ftyp"));
    assert!(!final_path.with_extension("mkv.part").exists());
    assert!(!final_path.with_extension("mp4.part").exists());
    std::fs::remove_file(final_path).expect("remove remux fixture");
}

#[test]
fn output_runtime_requests_interrupted_remux_recovery_without_blocking_the_gui() {
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    if !output.remux_recovery_supported() {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-gui-recovery-{token}.mp4"));
    output
        .request_recover_interrupted_remux(final_path.to_str().expect("UTF-8 temp path"))
        .expect("recovery request should be accepted");

    let result = (0..100).find_map(|_| {
        let result = output.take_remux_recovery_result();
        if result.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        result
    });
    assert_eq!(
        result
            .expect("worker must report recovery")
            .expect("recovery check must succeed"),
        obs_rs_engine::RemuxRecovery::NoCandidate
    );
    assert!(!output.remux_recovery_running());
    assert!(!final_path.exists());
}

#[test]
fn output_runtime_discovers_interrupted_remux_candidates_without_blocking_the_gui() {
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    if !output.remux_recovery_supported() {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-gui-candidates-{token}"));
    std::fs::create_dir(&directory).expect("candidate directory");
    std::fs::write(directory.join("zeta.mkv.part"), [1_u8]).expect("zeta candidate");
    std::fs::write(directory.join("alpha.mkv.part"), [2_u8]).expect("alpha candidate");
    obs_rs_engine::write_interrupted_remux_manifest(directory.join("zeta.mp4"))
        .expect("zeta manifest");
    obs_rs_engine::write_interrupted_remux_manifest(directory.join("alpha.mp4"))
        .expect("alpha manifest");
    let final_path = directory.join("configured.mp4");

    output
        .request_discover_interrupted_remux_candidates(final_path.to_str().expect("UTF-8 path"))
        .expect("candidate discovery request should be accepted");
    let result = (0..100).find_map(|_| {
        let result = output.take_remux_candidate_result();
        if result.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        result
    });
    let candidates = result
        .expect("worker must report candidate discovery")
        .expect("candidate discovery must succeed");
    assert_eq!(
        candidates,
        vec![directory.join("alpha.mp4"), directory.join("zeta.mp4")]
    );
    assert!(!output.remux_recovery_running());
    std::fs::remove_dir_all(directory).expect("remove candidate fixture");
}

#[test]
fn output_runtime_startup_discovery_uses_the_configured_directory() {
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    if !output.remux_recovery_supported() {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-gui-startup-recovery-{token}"));
    std::fs::create_dir(&directory).expect("startup recovery directory");
    std::fs::write(directory.join("alpha.mkv.part"), [1_u8]).expect("alpha source");
    obs_rs_engine::write_interrupted_remux_manifest(directory.join("alpha.mp4"))
        .expect("alpha manifest");

    output
        .request_startup_remux_discovery(
            directory
                .join("configured.mkv")
                .to_str()
                .expect("UTF-8 configured path"),
        )
        .expect("startup discovery request should accept the configured format");
    let result = (0..100).find_map(|_| {
        let result = output.take_remux_candidate_result();
        if result.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        result
    });
    assert_eq!(
        result
            .expect("worker must report startup candidates")
            .expect("startup candidate discovery must succeed"),
        vec![directory.join("alpha.mp4")]
    );
    assert!(!output.remux_recovery_running());
    std::fs::remove_dir_all(directory).expect("remove startup recovery fixture");
}

#[test]
fn output_runtime_routes_reference_split_recording_to_numbered_files() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("obs-rs-gui-split-{token}.obsr"));
    let mut output = OutputRuntime::new(format);
    output.configure_recording(&AppSettings {
        recording_quality: RecordingQuality::Lossless,
        recording_split_enabled: true,
        recording_split_size_mib: 1,
        recording_split_max_segments: 2,
        ..AppSettings::default()
    });
    output
        .start_recording(base.to_str().expect("UTF-8 temp path"))
        .expect("split recording should open");
    output.push_frame(&VideoFrame::solid(
        format,
        Timestamp::ZERO,
        [20, 30, 40, 255],
    ));
    let bytes = output
        .finish_recording()
        .expect("split recording should finalize");

    let segment = base.with_file_name(format!("obs-rs-gui-split-{token}-0001.obsr"));
    assert!(segment.is_file());
    assert_eq!(
        std::fs::metadata(&segment).expect("segment metadata").len(),
        u64::try_from(bytes).expect("bytes fit u64")
    );
    assert!(MemoryMuxer::decode(&std::fs::read(&segment).expect("read segment")).is_ok());
    std::fs::remove_file(segment).expect("remove split fixture");
}

#[test]
fn output_runtime_routes_production_split_recording_to_native_segments() {
    let format = VideoFormat::new(64, 64, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    if !output.capabilities().supports_segmented_recording()
        || !output
            .capabilities()
            .recording_formats()
            .contains(&OutputProfileKind::Mp4H264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("obs-rs-gui-native-split-{token}.mp4"));
    let settings = AppSettings {
        recording_format: crate::settings::RecordingFormat::Mp4,
        recording_split_enabled: true,
        recording_split_size_mib: 1,
        recording_split_max_segments: 3,
        ..AppSettings::default()
    };
    output.configure_stream(&settings);
    output.configure_recording(&settings);
    output
        .start_recording(base.to_str().expect("UTF-8 temp path"))
        .expect("native split recording should open");
    for index in 0..90 {
        let timestamp = u64::try_from(index).expect("index") * 33_333_333;
        output.push_frame(&VideoFrame::solid(
            format,
            Timestamp::from_nanos(timestamp),
            [20, 30, 40, 255],
        ));
    }
    let bytes = output
        .finish_recording()
        .expect("native split recording should finalize");
    assert!(bytes > 0);

    let stem = base
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("stem");
    let mut published = 0_usize;
    for index in 1..=3 {
        let segment = base.with_file_name(format!("{stem}-{index:05}.mp4"));
        if segment.is_file() {
            published = published.saturating_add(1);
            assert!(std::fs::metadata(&segment).expect("segment metadata").len() > 0);
            std::fs::remove_file(segment).expect("remove native segment");
        }
        let temporary = base.with_file_name(format!("{stem}-{index:05}.mp4.part"));
        assert!(
            !temporary.exists(),
            "native temporary segment must be cleaned"
        );
    }
    assert!(
        published > 0,
        "GUI must publish at least one native segment"
    );
}
