use super::*;
use std::path::PathBuf;

use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, EncoderPreset, OutputCapabilities, OutputProfile,
    OutputProfileKind, OutputTransport, RateControl, SegmentedRecordingPolicy, VideoCodec,
    VideoEncoderConfig,
};

use super::capabilities::{
    encoder_option_capabilities, production_profiles_with, property_present, protocol_capabilities,
    video_encoder_capability,
};
use super::pipeline::{element_available, first_matching, parse_element_names};

#[test]
fn rtmp_sink_selection_prefers_rtmp2_and_falls_back_to_legacy() {
    let candidates = ["rtmp2sink", "rtmpsink"];
    assert_eq!(
        first_matching(&candidates, |_| true).as_deref(),
        Some("rtmp2sink")
    );
    assert_eq!(
        first_matching(&candidates, |element| element == "rtmpsink").as_deref(),
        Some("rtmpsink")
    );
    assert_eq!(first_matching(&candidates, |_| false), None);
}

#[test]
fn capability_model_separates_protocol_codec_and_encoder_implementation() {
    let output = OutputCapabilities::approved(
        [
            OutputProfileKind::RtmpH264Aac,
            OutputProfileKind::SrtMpegTsH264Aac,
        ],
        true,
    );
    let protocols = protocol_capabilities(&output);
    assert!(protocols.iter().any(|capability| {
        capability.protocol() == ProductionProtocol::Reference && capability.available()
    }));
    assert!(protocols.iter().any(|capability| {
        capability.protocol() == ProductionProtocol::Rtmp && capability.available()
    }));
    assert!(protocols.iter().any(|capability| {
        capability.protocol() == ProductionProtocol::Rtmps && !capability.available()
    }));

    let hardware = video_encoder_capability("nvh264enc");
    let software = video_encoder_capability("openh264enc");
    assert_eq!(hardware.codec(), VideoCodec::H264);
    assert_eq!(software.codec(), VideoCodec::H264);
    assert!(hardware.hardware());
    assert!(!software.hardware());
    assert_ne!(hardware.id(), software.id());
}

#[test]
fn production_profiles_require_the_elements_used_by_each_graph() {
    let selected = std::collections::BTreeMap::from([
        ("h264", "openh264enc".to_owned()),
        ("aac", "avenc_aac".to_owned()),
        ("rtmp_sink", "rtmp2sink".to_owned()),
    ]);

    let complete = production_profiles_with(&selected, |_| true);
    assert!(complete.contains(&OutputProfileKind::MatroskaH264Aac));
    assert!(complete.contains(&OutputProfileKind::Mp4H264Aac));
    assert!(complete.contains(&OutputProfileKind::RtmpH264Aac));

    let without_pipeline_queue = production_profiles_with(&selected, |element| element != "queue");
    assert!(without_pipeline_queue.is_empty());

    let without_recording_sink =
        production_profiles_with(&selected, |element| element != "filesink");
    assert!(!without_recording_sink.contains(&OutputProfileKind::MatroskaH264Aac));
    assert!(!without_recording_sink.contains(&OutputProfileKind::Mp4H264Aac));
    assert!(without_recording_sink.contains(&OutputProfileKind::RtmpH264Aac));

    let without_h264_parser = production_profiles_with(&selected, |element| element != "h264parse");
    assert!(without_h264_parser.is_empty());
}

#[test]
fn option_discovery_matches_exact_property_names() {
    let inspection =
        "Element Properties:\n  bitrate : target\n  not-bitrate : no\n  gop-size : interval\n";
    assert!(property_present(inspection, "bitrate"));
    assert!(property_present(inspection, "gop-size"));
    assert!(!property_present(inspection, "rate-control"));

    if element_available("openh264enc") {
        let options = encoder_option_capabilities("openh264enc");
        assert!(options.bitrate());
        assert!(options.max_bitrate());
        assert!(options.keyframe_interval());
        assert!(!options.b_frames());
        assert!(options.rate_controls().contains(&RateControl::Cbr));
        assert!(options.presets().contains(&EncoderPreset::Quality));
        assert!(options.profiles().iter().any(|profile| profile == "high"));
    }
}

#[test]
fn configured_negotiation_honors_explicit_implementations_and_tuning() {
    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(OutputProfileKind::MatroskaH264Aac)
    {
        return;
    }
    let video_encoder = capabilities
        .video_encoders
        .iter()
        .find(|encoder| encoder.codec() == VideoCodec::H264)
        .expect("H.264 implementation");
    let audio_encoder = capabilities
        .audio_encoders
        .iter()
        .find(|encoder| encoder.codec() == AudioCodec::Aac)
        .expect("AAC implementation");
    let mut video = VideoEncoderConfig {
        implementation: obs_rs_output::EncoderImplementation::new(video_encoder.id()),
        bitrate_kbps: 8_000,
        ..VideoEncoderConfig::default()
    };
    let audio = AudioEncoderConfig {
        implementation: obs_rs_output::EncoderImplementation::new(audio_encoder.id()),
        bitrate_kbps: 192,
        ..AudioEncoderConfig::default()
    };
    let destination = ProductionDestination::Recording(PathBuf::from("configured.mkv"));
    let plan = ProductionPipelinePlan::negotiate_configured(
        OutputProfile::matroska_h264_aac(),
        &destination,
        &capabilities,
        &video,
        &audio,
    )
    .expect("configured plan");
    assert_eq!(plan.video_encoder(), video_encoder.id());
    assert_eq!(plan.audio_encoder(), audio_encoder.id());
    assert_eq!(plan.video_config().bitrate_kbps, 8_000);
    assert_eq!(plan.audio_config().bitrate_kbps, 192);

    video.implementation = obs_rs_output::EncoderImplementation::new("missing-encoder");
    assert!(ProductionPipelinePlan::negotiate_configured(
        OutputProfile::matroska_h264_aac(),
        &destination,
        &capabilities,
        &video,
        &audio,
    )
    .is_err());
}

#[test]
fn disabled_native_feature_never_claims_production_support() {
    if !cfg!(feature = "native") {
        let capabilities = GStreamerCapabilitySnapshot::probe();
        assert!(capabilities.runtime_version().is_none());
        assert!(!capabilities
            .output_capabilities()
            .supports(OutputProfileKind::MatroskaH264Aac));
        let snapshot = capabilities.capabilities();
        assert!(snapshot.native_runtime_version().is_none());
        assert!(!snapshot.native_adapter_compiled());
        assert_eq!(
            snapshot.production_status(),
            ProductionOutputStatus::NativeAdapterNotCompiled
        );
        assert!(snapshot
            .production_status_detail()
            .contains("native GStreamer adapter is not compiled"));
        assert!(!snapshot.supports_production_output());
    }
}

#[test]
fn cached_capability_probe_is_stable_for_the_process() {
    let first = GStreamerCapabilitySnapshot::probe_cached();
    let second = GStreamerCapabilitySnapshot::probe_cached();
    assert_eq!(first, second);
}

#[test]
fn gst_inspect_catalog_parser_ignores_headers_and_summaries() {
    let elements = parse_element_names(
        "audioconvert: audioconvert\n\
         coreelements: queue\n\
         Total count: 2 plugins, 2 features\n\
         malformed: not an element name\n",
    );

    assert_eq!(
        elements,
        ["audioconvert".to_owned(), "queue".to_owned()]
            .into_iter()
            .collect()
    );
}

#[test]
fn destinations_validate_schemes_passphrases_and_redact_secrets() {
    let srt = ProductionDestination::Srt {
        endpoint: "srt://example.invalid:9000".to_owned(),
        passphrase: Some("long-enough-secret".to_owned()),
    };
    assert!(srt
        .validate_for(OutputProfile::srt_mpeg_ts_h264_aac())
        .is_ok());
    assert!(!format!("{srt:?}").contains("long-enough-secret"));
    assert!(ProductionDestination::WebRtc {
        signaling_endpoint: String::new(),
        bearer_token: None,
    }
    .validate_for(OutputProfile::web_rtc_vp8_opus())
    .is_err());

    for (endpoint, expected) in [
        ("rtmp://media.example/live/key", OutputTransport::Rtmp),
        ("rtmps://media.example/live/key", OutputTransport::Rtmps),
        ("srt://media.example:9000", OutputTransport::SrtMpegTs),
    ] {
        let (profile, destination) =
            ProductionDestination::from_stream_endpoint(endpoint).expect("stream endpoint");
        assert_eq!(profile.transport(), expected);
        assert!(destination.validate_for(profile).is_ok());
        assert!(!format!("{destination:?}").contains("media.example"));
    }
    for invalid in [
        "rtmp://media.example/live",
        "rtmps://media.example/",
        "srt://media.example",
        "srt://media.example:9000?passphrase=short",
        "https://media.example/live/key",
    ] {
        assert!(
            ProductionDestination::from_stream_endpoint(invalid).is_err(),
            "{invalid} must be rejected"
        );
    }
}

#[test]
fn production_metadata_decoder_is_bounded_and_rejects_trailing_records() {
    let valid = concat!(
        "OBSRGST1\n",
        "profile=MatroskaH264Aac\n",
        "video=openh264enc\n",
        "audio=avenc_aac\n",
        "queue_bytes=4096\n",
        "atomic=true\n",
    );
    assert_eq!(ProductionPipelinePlan::validate_serialized(valid), Ok(()));
    assert!(ProductionPipelinePlan::validate_serialized(&format!("{valid}trailing=x\n")).is_err());
}

#[test]
fn recording_paths_are_typed_and_must_match_the_container() {
    let destination =
        ProductionDestination::Recording(std::path::Path::new("capture.mkv").to_owned());
    assert!(destination
        .validate_for(OutputProfile::matroska_h264_aac())
        .is_ok());
    let remux = ProductionDestination::RemuxRecording {
        final_path: std::path::Path::new("capture.mp4").to_owned(),
    };
    assert!(remux
        .validate_for(OutputProfile::matroska_h264_aac())
        .is_ok());
    assert!(remux.validate_for(OutputProfile::mp4_h264_aac()).is_err());
    assert!(ProductionDestination::RemuxRecording {
        final_path: std::path::Path::new("capture.mkv").to_owned(),
    }
    .validate_for(OutputProfile::matroska_h264_aac())
    .is_err());
    assert!(destination
        .validate_for(OutputProfile::rtmp_h264_aac())
        .is_err());
    assert!(
        ProductionDestination::Recording(std::path::Path::new("capture.mp4").to_owned())
            .validate_for(OutputProfile::mp4_h264_aac())
            .is_ok()
    );
    assert!(
        ProductionDestination::Recording(std::path::Path::new("capture.mov").to_owned())
            .validate_for(OutputProfile::mov_h264_aac())
            .is_ok()
    );
    assert!(
        ProductionDestination::Recording(std::path::Path::new("capture.flv").to_owned())
            .validate_for(OutputProfile::flv_h264_aac())
            .is_ok()
    );
    assert!(
        ProductionDestination::Recording(std::path::Path::new("capture.mkv").to_owned())
            .validate_for(OutputProfile::mp4_h264_aac())
            .is_err()
    );
    let policy = SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_secs(5), 3)
        .expect("segment policy");
    let segmented = ProductionDestination::SegmentedRecording {
        base_path: std::path::Path::new("capture.mp4").to_owned(),
        policy,
    };
    assert!(segmented
        .validate_for(OutputProfile::mp4_h264_aac())
        .is_ok());
    assert!(segmented
        .validate_for(OutputProfile::matroska_h264_aac())
        .is_err());
    assert!(format!("{segmented:?}").contains("segments"));
}

#[test]
fn webrtc_signaling_lifecycle_is_bounded_and_application_driven() {
    let destination = ProductionDestination::WebRtc {
        signaling_endpoint: "https://signal.invalid/session-secret".to_owned(),
        bearer_token: None,
    };
    let mut session = WebRtcSignalingSession::new(&destination).expect("signaling session");
    session
        .local_description_ready("v=0\r\ns=offer\r\n")
        .expect("offer");
    session
        .remote_description_received("v=0\r\ns=answer\r\n")
        .expect("answer");
    session.connected().expect("connected");
    session.retry(2).expect("retry");
    assert_eq!(session.retries(), 1);
    assert_eq!(
        session.state(),
        WebRtcSignalingState::AwaitingLocalDescription
    );
    assert!(session.local_description_ready("").is_err());
    session.close();
    assert_eq!(session.state(), WebRtcSignalingState::Closed);
}

#[cfg(feature = "native")]
#[test]
fn native_matroska_codecs_finalize_atomically_when_runtime_is_available() {
    use obs_rs_audio::{AudioBuffer, AudioFormat};
    use obs_rs_media::{FrameRate, PixelFormat, RawVideoFrame, Timestamp, VideoFormat, VideoFrame};

    let capabilities = GStreamerCapabilitySnapshot::probe();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let rate = FrameRate::new(30, 1).expect("rate");
    let video = VideoFormat::new(64, 64, rate).expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    for (suffix, profile) in [
        ("h264", OutputProfile::matroska_h264_aac()),
        ("hevc", OutputProfile::matroska_hevc_aac()),
        ("av1", OutputProfile::matroska_av1_aac()),
    ] {
        if !capabilities.output_capabilities().supports(profile.kind()) {
            continue;
        }
        let path = std::env::temp_dir().join(format!("obs-rs-gstreamer-{token}-{suffix}.mkv"));
        let destination = ProductionDestination::Recording(path.clone());
        let plan = ProductionPipelinePlan::negotiate(profile, &destination, &capabilities)
            .expect("approved pipeline");
        let mut session = GStreamerOutputSession::start(&plan, &destination, video, audio)
            .expect("start native session");
        for index in 0_u64..4 {
            let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
            if index == 0 {
                let mut nv12 = vec![16; video.pixel_count()];
                nv12.resize(PixelFormat::Nv12.bytes_for(video).expect("NV12 size"), 128);
                session
                    .push_raw_video(
                        RawVideoFrame::new(video, PixelFormat::Nv12, timestamp, nv12)
                            .expect("NV12 frame"),
                    )
                    .expect("NV12 video submission");
            } else {
                session
                    .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
                    .expect("RGBA video submission");
            }
            session
                .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("audio buffer"))
                .expect("audio submission");
        }
        session.close().expect("atomic finalization");
        let metadata = std::fs::metadata(&path).expect("published recording");
        assert!(metadata.len() > 0, "{suffix} recording must not be empty");
        assert!(!path.with_extension("mkv.part").exists());
        assert_eq!(session.telemetry().video_submitted(), 4);
        assert_eq!(session.telemetry().audio_submitted(), 4);
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(feature = "native")]
#[test]
fn native_hls_session_writes_a_bounded_playlist_and_segments() {
    use obs_rs_audio::{AudioBuffer, AudioFormat};
    use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

    let capabilities = GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(OutputProfileKind::HlsH264Aac)
    {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-hls-{token}"));
    let destination = ProductionDestination::Hls {
        directory: directory.clone(),
        segment_duration_secs: 1,
        playlist_size: 3,
        low_latency: false,
    };
    let profile = OutputProfile::hls_h264_aac();
    let plan =
        ProductionPipelinePlan::negotiate(profile, &destination, &capabilities).expect("HLS plan");
    let rate = FrameRate::new(30, 1).expect("rate");
    let video = VideoFormat::new(64, 64, rate).expect("video format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let mut session =
        GStreamerOutputSession::start(&plan, &destination, video, audio).expect("HLS session");
    for index in 0_u64..40 {
        let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
        session
            .push_video(VideoFrame::solid(video, timestamp, [40, 80, 120, 255]))
            .expect("video");
        session
            .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
            .expect("audio");
    }
    session.close().expect("HLS close");
    let playlist =
        std::fs::read_to_string(directory.join("playlist.m3u8")).expect("published playlist");
    assert!(playlist.starts_with("#EXTM3U"));
    let segments = std::fs::read_dir(&directory)
        .expect("HLS directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|value| value == "ts"))
        .count();
    assert!((1..=4).contains(&segments));
    let _ = std::fs::remove_dir_all(directory);
}
