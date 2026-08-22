use crate::*;

#[test]
fn production_profile_presets_are_versioned_and_bounded() {
    let profiles = [
        OutputProfile::reference(),
        OutputProfile::matroska_h264_aac(),
        OutputProfile::matroska_hevc_aac(),
        OutputProfile::matroska_av1_aac(),
        OutputProfile::mp4_h264_aac(),
        OutputProfile::rtmp_h264_aac(),
        OutputProfile::rtmps_h264_aac(),
        OutputProfile::srt_mpeg_ts_h264_aac(),
        OutputProfile::web_rtc_vp8_opus(),
        OutputProfile::hls_h264_aac(),
        OutputProfile::rist_mpeg_ts_h264_aac(),
    ];
    assert!(profiles
        .iter()
        .all(|profile| profile.version() == OUTPUT_PROFILE_VERSION));
    assert!(profiles
        .iter()
        .all(|profile| profile.queue_bytes() > 0 && profile.latency_millis() > 0));
    assert_eq!(profiles[1].transport(), OutputTransport::Matroska);
    assert_eq!(profiles[4].transport(), OutputTransport::Mp4);
    assert_eq!(profiles[7].transport(), OutputTransport::SrtMpegTs);
    assert_eq!(profiles[8].video_codec(), OutputVideoCodec::Vp8);
    assert_eq!(profiles[8].audio_codec(), OutputAudioCodec::Opus);
    assert_eq!(profiles[2].video_codec(), OutputVideoCodec::Hevc);
    assert_eq!(profiles[3].video_codec(), OutputVideoCodec::Av1);
    for profile in profiles {
        assert_eq!(
            OutputProfileKind::from_id(profile.kind().id()),
            Some(profile.kind())
        );
    }
}

#[test]
fn capability_negotiation_never_claims_an_unavailable_production_profile() {
    let reference = OutputCapabilities::reference_only();
    assert!(reference.negotiate(OutputProfile::reference()).is_ok());
    assert_eq!(
        reference.negotiate(OutputProfile::rtmp_h264_aac()),
        Err(OutputError::ProfileUnavailable {
            profile: OutputProfileKind::RtmpH264Aac
        })
    );

    let production = OutputCapabilities::approved(
        [
            OutputProfileKind::MatroskaH264Aac,
            OutputProfileKind::Mp4H264Aac,
            OutputProfileKind::RtmpH264Aac,
            OutputProfileKind::RtmpsH264Aac,
        ],
        true,
    );
    let selected = production
        .negotiate(OutputProfile::matroska_h264_aac())
        .expect("approved profile");
    assert!(selected.hardware_video());
    assert_eq!(selected.profile(), OutputProfile::matroska_h264_aac());
    assert!(!production.supports(OutputProfileKind::WebRtcVp8Opus));
}
