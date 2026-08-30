use super::*;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use gstreamer as gst;
use gstreamer::prelude::*;
use obs_rs_audio::{AudioBuffer, AudioFormat};
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{
    AudioEncoderConfig, EncoderImplementation, EncoderPreset, OutputProfile, OutputProfileKind,
    RateControl, ReconnectOutcome, ReconnectPolicy, SegmentedRecordingPolicy, StreamingTransport,
    VideoEncoderConfig,
};

use super::super::{ProductionDestination, ProductionPipelinePlan};

fn plan(profile: OutputProfile) -> ProductionPipelinePlan {
    let video_config = VideoEncoderConfig {
        implementation: EncoderImplementation::new("openh264enc"),
        ..VideoEncoderConfig::default()
    };
    let audio_config = AudioEncoderConfig {
        implementation: EncoderImplementation::new("avenc_aac"),
        ..AudioEncoderConfig::default()
    };
    ProductionPipelinePlan {
        profile,
        video_encoder: "openh264enc".to_owned(),
        audio_encoder: "avenc_aac".to_owned(),
        bounded_queue_bytes: 1_048_576,
        atomic_recording: false,
        video_config,
        audio_config,
        rtmp_sink: matches!(
            profile.kind(),
            OutputProfileKind::RtmpH264Aac | OutputProfileKind::RtmpsH264Aac
        )
        .then(|| "rtmp2sink".to_owned()),
    }
}

#[test]
fn live_protocols_build_exact_mux_and_sink_graphs() {
    gst::init().expect("GStreamer runtime");
    let cases = [
        (
            plan(OutputProfile::rtmp_h264_aac()),
            ProductionDestination::Rtmp {
                endpoint: "rtmp://127.0.0.1/live/key".to_owned(),
            },
            "flvmux",
            "rtmp2sink",
        ),
        (
            plan(OutputProfile::rtmps_h264_aac()),
            ProductionDestination::Rtmps {
                endpoint: "rtmps://127.0.0.1/live/key".to_owned(),
            },
            "flvmux",
            "rtmp2sink",
        ),
        (
            plan(OutputProfile::srt_mpeg_ts_h264_aac()),
            ProductionDestination::Srt {
                endpoint: "srt://127.0.0.1:9000".to_owned(),
                passphrase: None,
            },
            "mpegtsmux",
            "srtsink",
        ),
    ];
    for (plan, destination, mux, sink) in cases {
        let pipeline_description =
            pipeline_description(&plan, &destination).expect("pipeline description");
        assert!(pipeline_description.description.contains(mux));
        assert!(pipeline_description.description.contains(sink));
        assert!(
            pipeline_description.final_path.is_none()
                && pipeline_description.temp_path.is_none()
                && pipeline_description.segmented_policy.is_none()
        );
        let element = gst::parse::launch_full(
            &pipeline_description.description,
            None,
            gst::ParseFlags::FATAL_ERRORS,
        )
        .expect("pipeline parses");
        let pipeline = element.downcast::<gst::Pipeline>().expect("pipeline");
        configure_sink(&pipeline, &destination, None).expect("sink endpoint");
        let output_sink = pipeline.by_name("output_sink").expect("named sink");
        let configured = match &destination {
            ProductionDestination::Rtmp { .. } | ProductionDestination::Rtmps { .. } => {
                output_sink.property::<String>("location")
            }
            ProductionDestination::Srt { .. } => output_sink.property::<String>("uri"),
            _ => unreachable!("only live protocols are in the cases"),
        };
        let (ProductionDestination::Rtmp { endpoint }
        | ProductionDestination::Rtmps { endpoint }
        | ProductionDestination::Srt { endpoint, .. }) = destination
        else {
            unreachable!("only live protocols are in the cases")
        };
        assert_eq!(configured, endpoint);
    }
    assert_eq!(
        OutputProfile::rtmps_h264_aac().kind(),
        OutputProfileKind::RtmpsH264Aac
    );
}

#[test]
fn whip_pipeline_uses_explicit_raw_av_request_pads_and_signaller_properties() {
    gst::init().expect("GStreamer runtime");
    if gst::ElementFactory::find("whipclientsink").is_none() {
        return;
    }

    let plan = plan(OutputProfile::web_rtc_vp8_opus());
    let destination = ProductionDestination::WebRtc {
        signaling_endpoint: "https://media.example/whip/endpoint".to_owned(),
        bearer_token: Some("token".to_owned()),
    };
    let description = pipeline_description(&plan, &destination).expect("WHIP graph");
    assert!(description.description.contains("output_sink.video_0"));
    assert!(description.description.contains("output_sink.audio_0"));
    assert!(description.description.contains("videoconvert"));
    assert!(description.description.contains("audioconvert"));

    let element = gst::parse::launch_full(
        &description.description,
        None,
        gst::ParseFlags::FATAL_ERRORS,
    )
    .expect("WHIP pipeline parses");
    let pipeline = element.downcast::<gst::Pipeline>().expect("pipeline type");
    configure_sink(&pipeline, &destination, None).expect("WHIP sink configuration");
    let sink = pipeline.by_name("output_sink").expect("WHIP sink");
    assert!(
        sink.lookup("signaller::whip-endpoint").is_ok()
            || sink.find_property("whip-endpoint").is_some()
            || sink.find_property("endpoint").is_some()
    );
    assert!(
        sink.lookup("signaller::auth-token").is_ok()
            || sink.find_property("auth-token").is_some()
            || sink.find_property("bearer-token").is_some()
    );
}

#[test]
fn openh264_and_aac_tuning_reaches_native_encoder_properties() {
    gst::init().expect("GStreamer runtime");
    if gst::ElementFactory::find("openh264enc").is_none()
        || gst::ElementFactory::find("avenc_aac").is_none()
    {
        return;
    }
    let mut plan = plan(OutputProfile::matroska_h264_aac());
    plan.video_config.bitrate_kbps = 7_500;
    plan.video_config.max_bitrate_kbps = Some(8_000);
    plan.video_config.keyframe_interval_secs = 3;
    plan.video_config.rate_control = RateControl::Cqp;
    plan.video_config.preset = EncoderPreset::Quality;
    plan.audio_config.bitrate_kbps = 192;
    let destination = ProductionDestination::Recording(PathBuf::from("configured.mkv"));
    let pipeline_description = pipeline_description(&plan, &destination).expect("description");
    let pipeline = gst::parse::launch(&pipeline_description.description)
        .expect("pipeline")
        .downcast::<gst::Pipeline>()
        .expect("pipeline type");
    let format = VideoFormat::new(
        320,
        180,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    configure_encoders(&pipeline, &plan, format).expect("encoder config");
    let video = pipeline.by_name("video_encoder").expect("video encoder");
    let audio = pipeline.by_name("audio_encoder").expect("audio encoder");
    assert_eq!(video.property::<u32>("bitrate"), 7_500_000);
    assert_eq!(video.property::<u32>("max-bitrate"), 8_000_000);
    assert_eq!(video.property::<u32>("gop-size"), 90);
    assert_eq!(audio.property::<i32>("bitrate"), 192_000);
}

#[test]
fn mov_recording_uses_qtmux_and_a_hidden_atomic_path() {
    gst::init().expect("GStreamer runtime");
    if gst::ElementFactory::find("qtmux").is_none() {
        return;
    }
    let plan = plan(OutputProfile::mov_h264_aac());
    let destination = ProductionDestination::Recording(PathBuf::from("capture.mov"));
    let pipeline_description = pipeline_description(&plan, &destination).expect("MOV graph");
    assert!(pipeline_description.description.contains("qtmux name=mux"));
    assert_eq!(
        pipeline_description.final_path,
        Some(PathBuf::from("capture.mov"))
    );
    assert_eq!(
        pipeline_description.temp_path,
        Some(PathBuf::from("capture.mov.part"))
    );
    assert!(pipeline_description.segmented_policy.is_none());
    gst::parse::launch_full(
        &pipeline_description.description,
        None,
        gst::ParseFlags::FATAL_ERRORS,
    )
    .expect("MOV pipeline parses");
}

#[test]
fn fragmented_mp4_recording_uses_bounded_fragmented_muxing() {
    gst::init().expect("GStreamer runtime");
    if gst::ElementFactory::find("mp4mux").is_none() {
        return;
    }
    let plan = plan(OutputProfile::fragmented_mp4_h264_aac());
    let destination = ProductionDestination::Recording(PathBuf::from("capture.mp4"));
    let pipeline_description =
        pipeline_description(&plan, &destination).expect("fragmented MP4 graph");
    assert!(pipeline_description
        .description
        .contains("fragment-duration=1000"));
    assert!(pipeline_description.description.contains("streamable=true"));
    assert_eq!(
        pipeline_description.final_path,
        Some(PathBuf::from("capture.mp4"))
    );
    assert_eq!(
        pipeline_description.temp_path,
        Some(PathBuf::from("capture.mp4.part"))
    );
    assert!(pipeline_description.segmented_policy.is_none());
    gst::parse::launch_full(
        &pipeline_description.description,
        None,
        gst::ParseFlags::FATAL_ERRORS,
    )
    .expect("fragmented MP4 pipeline parses");
}

#[test]
fn segmented_mp4_recording_uses_bounded_split_muxing_and_hidden_paths() {
    gst::init().expect("GStreamer runtime");
    if gst::ElementFactory::find("splitmuxsink").is_none()
        || gst::ElementFactory::find("mp4mux").is_none()
    {
        return;
    }
    let policy = SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_secs(5), 3)
        .expect("segment policy");
    let base_path = std::env::temp_dir().join("obs-rs-segmented-capture.mp4");
    let destination = ProductionDestination::SegmentedRecording {
        base_path: base_path.clone(),
        policy,
    };
    let plan = plan(OutputProfile::mp4_h264_aac());
    let pipeline_description =
        pipeline_description(&plan, &destination).expect("segmented MP4 graph");
    assert!(pipeline_description
        .description
        .contains("splitmuxsink name=output_sink"));
    assert!(pipeline_description
        .description
        .contains("muxer-factory=mp4mux"));
    assert!(pipeline_description
        .description
        .contains("max-size-time=5000000000"));
    assert!(pipeline_description
        .description
        .contains("max-size-bytes=1000000"));
    assert!(pipeline_description.description.contains("max-files=3"));
    assert_eq!(pipeline_description.final_path, Some(base_path.clone()));
    assert!(pipeline_description.temp_path.is_none());
    assert_eq!(pipeline_description.segmented_policy, Some(policy));

    let element = gst::parse::launch_full(
        &pipeline_description.description,
        None,
        gst::ParseFlags::FATAL_ERRORS,
    )
    .expect("segmented MP4 pipeline parses");
    let pipeline = element.downcast::<gst::Pipeline>().expect("pipeline type");
    configure_sink(&pipeline, &destination, None).expect("segmented sink path");
    let sink = pipeline.by_name("output_sink").expect("split sink");
    assert_eq!(
        sink.property::<String>("location"),
        std::env::temp_dir()
            .join("obs-rs-segmented-capture-%05d.mp4.part")
            .to_string_lossy()
    );
}

#[test]
fn native_segmented_mp4_rolls_over_and_publishes_bounded_files() {
    gst::init().expect("GStreamer runtime");
    if ["splitmuxsink", "mp4mux", "openh264enc", "avenc_aac"]
        .iter()
        .any(|element| gst::ElementFactory::find(element).is_none())
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base_path = std::env::temp_dir().join(format!("obs-rs-native-segmented-{token}.mp4"));
    let policy = SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_millis(500), 3)
        .expect("segment policy");
    let destination = ProductionDestination::SegmentedRecording {
        base_path: base_path.clone(),
        policy,
    };
    let mut plan = plan(OutputProfile::mp4_h264_aac());
    plan.video_config.keyframe_interval_secs = 1;
    let video = VideoFormat::new(
        64,
        64,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let mut session = GStreamerOutputSession::start(&plan, &destination, video, audio)
        .expect("segmented native session");
    for index in 0_u64..180 {
        let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
        session
            .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
            .expect("video submission");
        session
            .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
            .expect("audio submission");
    }
    session.close().expect("segmented close");

    let mut published = 0_usize;
    for index in 1..=policy.max_segments() {
        let (final_path, temp_path) =
            segmented_recording_paths(&base_path, index).expect("segment paths");
        if final_path.exists() {
            published = published.saturating_add(1);
            assert!(
                std::fs::metadata(&final_path)
                    .expect("segment metadata")
                    .len()
                    > 0
            );
        }
        assert!(
            !temp_path.exists(),
            "temporary segment must be hidden/cleaned"
        );
        let _ = std::fs::remove_file(final_path);
    }
    assert!(published >= 2, "expected muxer rollover, got {published}");
}

#[test]
fn native_matroska_remux_publishes_mp4_without_replacing_existing_output() {
    gst::init().expect("GStreamer runtime");
    if [
        "matroskamux",
        "matroskademux",
        "mp4mux",
        "h264parse",
        "aacparse",
        "queue",
        "openh264enc",
        "avenc_aac",
        "filesrc",
        "filesink",
    ]
    .iter()
    .any(|element| gst::ElementFactory::find(element).is_none())
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("obs-rs-remux-source-{token}.mkv"));
    let destination = std::env::temp_dir().join(format!("obs-rs-remux-destination-{token}.mp4"));
    let temporary = destination.with_extension("mp4.part");
    let plan = plan(OutputProfile::matroska_h264_aac());
    let video = VideoFormat::new(
        64,
        64,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let source_destination = ProductionDestination::Recording(source.clone());
    let mut session = GStreamerOutputSession::start(&plan, &source_destination, video, audio)
        .expect("Matroska source session");
    for index in 0_u64..60 {
        let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
        session
            .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
            .expect("video submission");
        session
            .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
            .expect("audio submission");
    }
    session.close().expect("Matroska source close");
    assert!(source.is_file());

    let bytes = remux_matroska_to_mp4(&source, &destination).expect("remux Matroska");
    let persisted = std::fs::read(&destination).expect("read remuxed MP4");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
    assert!(!temporary.exists());

    let existing = persisted.clone();
    let error = remux_matroska_to_mp4(&source, &destination)
        .expect_err("remux must not replace an existing destination");
    assert!(error.to_string().contains("without replacing"));
    assert_eq!(
        std::fs::read(&destination).expect("read preserved MP4"),
        existing
    );
    assert!(!temporary.exists());
    std::fs::remove_file(source).expect("remove Matroska source");
    std::fs::remove_file(destination).expect("remove remuxed MP4");
}

#[test]
fn native_remux_recording_session_keeps_source_hidden_until_mp4_is_published() {
    gst::init().expect("GStreamer runtime");
    if [
        "matroskamux",
        "matroskademux",
        "mp4mux",
        "h264parse",
        "aacparse",
        "queue",
        "openh264enc",
        "avenc_aac",
        "filesrc",
        "filesink",
    ]
    .iter()
    .any(|element| gst::ElementFactory::find(element).is_none())
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-auto-remux-{token}.mp4"));
    let source_path = final_path.with_extension("mkv.part");
    let destination = ProductionDestination::RemuxRecording {
        final_path: final_path.clone(),
    };
    let plan = plan(OutputProfile::matroska_h264_aac());
    let video = VideoFormat::new(
        64,
        64,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let mut session = GStreamerOutputSession::start(&plan, &destination, video, audio)
        .expect("remux recording session");
    for index in 0_u64..30 {
        let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
        session
            .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
            .expect("video submission");
        session
            .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
            .expect("audio submission");
    }
    assert!(
        source_path.exists(),
        "source must remain hidden while recording"
    );
    let manifest_path = remux_manifest_path(&final_path);
    assert!(
        manifest_path.exists(),
        "remux manifest must be durable while recording"
    );
    session.close().expect("automatic remux close");

    let bytes = std::fs::metadata(&final_path).expect("published MP4").len();
    assert!(bytes > 0);
    assert_eq!(
        session.committed_bytes(),
        Some(usize::try_from(bytes).expect("size"))
    );
    assert!(
        !source_path.exists(),
        "hidden Matroska source must be consumed"
    );
    assert!(!final_path.with_extension("mp4.part").exists());
    assert!(
        !manifest_path.exists(),
        "published recordings must remove the manifest"
    );
    std::fs::remove_file(final_path).expect("remove remuxed MP4");
}

#[test]
fn remux_manifest_is_bounded_atomic_and_matches_only_its_recording() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-remux-manifest-{token}"));
    std::fs::create_dir(&directory).expect("manifest directory");
    let final_path = directory.join("capture.mp4");
    let source_path = final_path.with_extension("mkv.part");
    write_interrupted_remux_manifest(&final_path).expect("write remux manifest");
    let manifest_path = remux_manifest_path(&final_path);
    assert!(manifest_path.is_file());
    assert!(!manifest_path.with_extension("json.part").exists());
    assert!(remux_manifest_matches(&source_path, &final_path).expect("match manifest"));
    assert!(
        !remux_manifest_matches(&directory.join("other.mkv.part"), &final_path)
            .expect("mismatched manifest")
    );
    let document = std::fs::read_to_string(&manifest_path).expect("read manifest");
    assert!(document.len() <= MAX_REMUX_MANIFEST_BYTES);
    assert!(document.contains(REMUX_MANIFEST_FORMAT));
    std::fs::write(&manifest_path, [0xff_u8]).expect("corrupt manifest");
    assert!(!remux_manifest_matches(&source_path, &final_path).expect("invalid manifest"));
    recover_stale_remux_manifest(&final_path).expect("remove manifest");
    assert!(!manifest_path.exists());
    std::fs::remove_dir_all(directory).expect("remove manifest directory");
}

#[test]
fn remux_candidate_discovery_is_sorted_bounded_and_skips_published_artifacts() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-remux-candidates-{token}"));
    std::fs::create_dir(&directory).expect("candidate directory");
    std::fs::write(directory.join("zeta.mkv.part"), [1_u8, 2]).expect("zeta candidate");
    std::fs::write(directory.join("alpha.mkv.part"), [3_u8]).expect("alpha candidate");
    write_interrupted_remux_manifest(directory.join("zeta.mp4")).expect("zeta manifest");
    write_interrupted_remux_manifest(directory.join("alpha.mp4")).expect("alpha manifest");
    std::fs::write(directory.join("empty.mkv.part"), []).expect("empty candidate");
    std::fs::write(directory.join("published.mkv.part"), [4_u8]).expect("published source");
    std::fs::write(directory.join("published.mp4"), [5_u8]).expect("published destination");
    write_interrupted_remux_manifest(directory.join("published.mp4")).expect("published manifest");
    std::fs::write(directory.join("ordinary.mkv.part"), [7_u8]).expect("ordinary Matroska source");
    std::fs::write(directory.join("ignore.txt"), [6_u8]).expect("unrelated file");
    std::fs::create_dir(directory.join("nested.mkv.part")).expect("nested directory");

    let candidates =
        discover_interrupted_remux_candidates(&directory).expect("discover candidates");
    assert_eq!(
        candidates,
        vec![directory.join("alpha.mp4"), directory.join("zeta.mp4")]
    );
    assert_eq!(
        remux_final_path_from_source(Path::new("/tmp/Case.MKV.PART")),
        Some(PathBuf::from("/tmp/Case.mp4"))
    );
    assert!(
        discover_interrupted_remux_candidates(directory.join("missing"))
            .expect("missing directory is empty")
            .is_empty()
    );
    let error = discover_interrupted_remux_candidates(directory.join("ignore.txt"))
        .expect_err("file is not a candidate directory");
    assert!(error.to_string().contains("must be a directory"));
    std::fs::remove_dir_all(directory).expect("remove candidate directory");
}

#[test]
fn interrupted_remux_recovery_publishes_mp4_and_consumes_source() {
    gst::init().expect("GStreamer runtime");
    if [
        "matroskamux",
        "matroskademux",
        "mp4mux",
        "h264parse",
        "aacparse",
        "queue",
        "openh264enc",
        "avenc_aac",
        "filesrc",
        "filesink",
    ]
    .iter()
    .any(|element| gst::ElementFactory::find(element).is_none())
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-remux-recovery-{token}.mp4"));
    let completed_source = final_path.with_extension("mkv");
    let interrupted_source = final_path.with_extension("mkv.part");
    let plan = plan(OutputProfile::matroska_h264_aac());
    let video = VideoFormat::new(
        64,
        64,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let source_destination = ProductionDestination::Recording(completed_source.clone());
    let mut session = GStreamerOutputSession::start(&plan, &source_destination, video, audio)
        .expect("Matroska source session");
    for index in 0_u64..30 {
        let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
        session
            .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
            .expect("video submission");
        session
            .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
            .expect("audio submission");
    }
    session.close().expect("Matroska source close");
    std::fs::rename(&completed_source, &interrupted_source).expect("hide source");
    write_interrupted_remux_manifest(&final_path).expect("recovery manifest");

    assert_eq!(
        recover_interrupted_remux_recording(&final_path).expect("recover remux"),
        RemuxRecovery::Recovered {
            bytes: usize::try_from(std::fs::metadata(&final_path).expect("MP4 metadata").len(),)
                .expect("MP4 size")
        }
    );
    assert_eq!(
        std::fs::read(&final_path)
            .expect("read recovered MP4")
            .get(4..8),
        Some(&b"ftyp"[..])
    );
    assert!(!interrupted_source.exists());
    assert!(!final_path.with_extension("mp4.part").exists());
    assert!(!remux_manifest_path(&final_path).exists());
    std::fs::remove_file(final_path).expect("remove recovered MP4");
}

#[test]
fn interrupted_remux_recovery_reports_missing_source_without_creating_output() {
    gst::init().expect("GStreamer runtime");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-remux-no-candidate-{token}.mp4"));
    assert_eq!(
        recover_interrupted_remux_recording(&final_path).expect("recovery check"),
        RemuxRecovery::NoCandidate
    );
    assert!(!final_path.exists());
    assert!(!final_path.with_extension("mkv.part").exists());
}

#[test]
fn stale_recording_artifact_cleanup_is_bounded_and_typed() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let stale = std::env::temp_dir().join(format!("obs-rs-native-recovery-{token}.mkv.part"));
    std::fs::write(&stale, [1, 2, 3]).expect("write stale artifact");
    recover_stale_recording_artifact(Some(&stale)).expect("remove stale artifact");
    assert!(!stale.exists());
    recover_stale_recording_artifact(Some(&stale)).expect("missing artifact is harmless");

    std::fs::create_dir(&stale).expect("create invalid artifact");
    let error = recover_stale_recording_artifact(Some(&stale))
        .expect_err("directory must not be treated as a recording artifact");
    assert!(error
        .to_string()
        .contains("remove stale production recording artifact"));
    std::fs::remove_dir(&stale).expect("remove invalid artifact");
}

#[test]
fn native_publication_is_no_clobber_and_rejects_empty_artifacts() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir();
    let temp = directory.join(format!("obs-rs-native-publish-{token}.part"));
    let final_path = directory.join(format!("obs-rs-native-publish-{token}.mp4"));
    std::fs::write(&temp, [1, 2, 3]).expect("write temporary recording");
    publish_recording_artifact(&temp, &final_path).expect("publish recording");
    assert_eq!(
        std::fs::read(&final_path).expect("read recording"),
        [1, 2, 3]
    );
    assert!(!temp.exists());

    std::fs::write(&temp, [4, 5, 6]).expect("write second temporary recording");
    let error = publish_recording_artifact(&temp, &final_path)
        .expect_err("existing final path must not be replaced");
    assert!(error.to_string().contains("without replacing"));
    assert_eq!(
        std::fs::read(&temp).expect("read preserved temporary"),
        [4, 5, 6]
    );
    std::fs::remove_file(&temp).expect("remove preserved temporary");

    let empty_final = directory.join(format!("obs-rs-native-publish-{token}-empty.mp4"));
    std::fs::write(&temp, []).expect("write empty recording");
    let error = publish_recording_artifact(&temp, &empty_final)
        .expect_err("empty final artifact must be rejected");
    assert!(error.to_string().contains("empty production recording"));
    std::fs::remove_file(&temp).expect("remove empty temporary");
    std::fs::remove_file(&final_path).expect("remove published recording");
}

#[test]
fn hls_and_rist_sinks_receive_bounded_typed_configuration() {
    gst::init().expect("GStreamer runtime");
    let hls = ProductionDestination::Hls {
        directory: PathBuf::from("hls-output"),
        segment_duration_secs: 3,
        playlist_size: 5,
        low_latency: false,
    };
    let mut hls_plan = plan(OutputProfile::hls_h264_aac());
    hls_plan.atomic_recording = false;
    let hls_description = pipeline_description(&hls_plan, &hls).expect("HLS graph");
    assert!(hls_description.description.contains("hlssink2"));

    let rist = ProductionDestination::Rist {
        host: "127.0.0.1".to_owned(),
        port: 5_000,
        sender_buffer_ms: 750,
        shared_secret: None,
    };
    let rist_plan = plan(OutputProfile::rist_mpeg_ts_h264_aac());
    let rist_description = pipeline_description(&rist_plan, &rist).expect("RIST graph");
    assert!(rist_description.description.contains("mpegtsmux"));
    assert!(rist_description.description.contains("rtpmp2tpay"));
    let pipeline = gst::parse::launch(&rist_description.description)
        .expect("RIST pipeline")
        .downcast::<gst::Pipeline>()
        .expect("pipeline type");
    configure_sink(&pipeline, &rist, None).expect("RIST configuration");
    let sink = pipeline.by_name("output_sink").expect("RIST sink");
    assert_eq!(sink.property::<String>("address"), "127.0.0.1");
    assert_eq!(sink.property::<u32>("port"), 5_000);
    assert_eq!(sink.property::<u32>("sender-buffer"), 750);
}

#[test]
fn live_session_is_live_and_enforces_reconnect_budget() {
    let plan = plan(OutputProfile::rtmp_h264_aac());
    let destination = ProductionDestination::Rtmp {
        endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
    };
    let video = VideoFormat::new(
        16,
        16,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let mut session = GStreamerOutputSession::start_with_reconnect_policy(
        &plan,
        &destination,
        video,
        audio,
        ReconnectPolicy::immediate(1),
    )
    .expect("live session");

    assert!(session.video.is_live());
    assert!(session.audio.is_live());
    StreamingTransport::reconnect(&mut session).expect("first reconnect");
    assert_eq!(session.telemetry().reconnects(), 1);
    assert!(StreamingTransport::reconnect(&mut session).is_err());
    assert_eq!(session.state(), NativeOutputState::Failed);
}

#[test]
fn native_reconnect_honors_a_bounded_deferred_retry() {
    let plan = plan(OutputProfile::rtmp_h264_aac());
    let destination = ProductionDestination::Rtmp {
        endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
    };
    let video = VideoFormat::new(
        16,
        16,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let policy = ReconnectPolicy::with_backoff(
        2,
        std::time::Duration::from_millis(100),
        std::time::Duration::from_millis(250),
    );
    let mut session = GStreamerOutputSession::start_with_reconnect_policy(
        &plan,
        &destination,
        video,
        audio,
        policy,
    )
    .expect("live session");
    let now = Instant::now();
    session.schedule_reconnect(now);

    assert_eq!(
        session.reconnect_live_at(now),
        Ok(ReconnectOutcome::Deferred {
            retry_after: std::time::Duration::from_millis(100),
        })
    );
    assert_eq!(session.state(), NativeOutputState::Retrying);
    assert_eq!(
        session.reconnect_live_at(now + std::time::Duration::from_millis(100)),
        Ok(ReconnectOutcome::Reconnected)
    );
    assert_eq!(session.state(), NativeOutputState::Ready);
}

#[test]
fn native_reconnect_build_failures_use_the_remaining_retry_budget() {
    let plan = plan(OutputProfile::rtmp_h264_aac());
    let destination = ProductionDestination::Rtmp {
        endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
    };
    let video = VideoFormat::new(
        16,
        16,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let policy = ReconnectPolicy::with_backoff(
        3,
        std::time::Duration::from_millis(100),
        std::time::Duration::from_millis(250),
    );
    let mut session = GStreamerOutputSession::start_with_reconnect_policy(
        &plan,
        &destination,
        video,
        audio,
        policy,
    )
    .expect("live session");
    let now = Instant::now();
    session.reconnect_attempts = 1;

    assert_eq!(
        session.defer_reconnect_after_failure(
            now,
            GStreamerError::Native("temporary sink failure".to_owned()),
        ),
        Ok(ReconnectOutcome::Deferred {
            retry_after: std::time::Duration::from_millis(200),
        })
    );
    assert_eq!(session.state(), NativeOutputState::Retrying);

    session.reconnect_attempts = policy.max_attempts();
    assert!(matches!(
        session.defer_reconnect_after_failure(
            now,
            GStreamerError::Native("retry budget exhausted".to_owned()),
        ),
        Err(GStreamerError::Native(message)) if message == "retry budget exhausted"
    ));
    assert_eq!(session.state(), NativeOutputState::Failed);
}

#[test]
fn native_recovery_error_is_bounded_and_cleared_after_reconnect() {
    let plan = plan(OutputProfile::rtmp_h264_aac());
    let destination = ProductionDestination::Rtmp {
        endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
    };
    let video = VideoFormat::new(
        16,
        16,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let mut session = GStreamerOutputSession::start_with_reconnect_policy(
        &plan,
        &destination,
        video,
        audio,
        ReconnectPolicy::immediate(1),
    )
    .expect("live session");

    session.record_error(&GStreamerError::Native("x".repeat(2_000)));
    let error = session.last_error().expect("recovery error");
    assert_eq!(error.chars().count(), 1_025);
    assert!(error.ends_with('…'));

    session.state = NativeOutputState::Retrying;
    session.next_reconnect_at = Some(Instant::now());
    session.poll_health().expect("reconnect clears old error");
    assert_eq!(session.last_error(), None);
}

#[test]
fn native_health_poll_services_a_pending_reconnect_deadline() {
    let plan = plan(OutputProfile::rtmp_h264_aac());
    let destination = ProductionDestination::Rtmp {
        endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
    };
    let video = VideoFormat::new(
        16,
        16,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let mut session = GStreamerOutputSession::start_with_reconnect_policy(
        &plan,
        &destination,
        video,
        audio,
        ReconnectPolicy::immediate(1),
    )
    .expect("live session");

    // A prior poll has already moved the session into its retry state. The
    // next poll must make the due reconnect attempt even when the old bus has
    // no second error message to consume.
    session.state = NativeOutputState::Retrying;
    session.next_reconnect_at = Some(Instant::now());
    session.poll_health().expect("pending reconnect");

    assert_eq!(session.telemetry().reconnects(), 1);
    assert_eq!(session.state(), NativeOutputState::Ready);
}

#[test]
fn native_media_submission_drops_frames_during_reconnect_backoff() {
    let plan = plan(OutputProfile::rtmp_h264_aac());
    let destination = ProductionDestination::Rtmp {
        endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
    };
    let video = VideoFormat::new(
        16,
        16,
        obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
    )
    .expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let policy = ReconnectPolicy::with_backoff(
        1,
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    );
    let mut session = GStreamerOutputSession::start_with_reconnect_policy(
        &plan,
        &destination,
        video,
        audio,
        policy,
    )
    .expect("live session");
    session.state = NativeOutputState::Retrying;
    session.next_reconnect_at = Some(Instant::now() + std::time::Duration::from_secs(1));

    session
        .push_video(VideoFrame::solid(video, Timestamp::ZERO, [0, 0, 0, 255]))
        .expect("backoff should not fail media submission");

    assert_eq!(session.telemetry().video_submitted(), 0);
    assert_eq!(session.telemetry().dropped(), 1);
}
