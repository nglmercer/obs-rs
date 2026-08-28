#[allow(
    clippy::wildcard_imports,
    reason = "settings tests use the module namespace as their fixture API"
)]
use super::*;
use crate::dock_tree::DockAxis;

#[test]
fn settings_round_trip_through_the_config_document() {
    let settings = AppSettings {
        locale: "es".to_owned(),
        theme: 2,
        confirm_start_stream: true,
        auto_record_when_streaming: true,
        sample_rate: 2,
        channels: 1,
        desktop_audio_id: "wasapi:{desktop-output}".to_owned(),
        audio_monitor_output_id: "pipewire-output-7".to_owned(),
        microphone_monitor_mode: AudioMonitorMode::MonitorOnly,
        desktop_audio_monitor_mode: AudioMonitorMode::MonitorAndOutput,
        audio_input_sync_offset_millis: 125,
        desktop_audio_sync_offset_millis: 2_500,
        hotkey_swap: "F1".to_owned(),
        hotkey_previous_scene: "Alt+F6".to_owned(),
        hotkey_next_scene: "Alt+F7".to_owned(),
        hotkey_undo: "Alt+U".to_owned(),
        hotkey_redo: "Alt+Y".to_owned(),
        hotkey_save_project: "Alt+S".to_owned(),
        hotkey_cut_transition: "Alt+C".to_owned(),
        hotkey_fade_transition: "Alt+F".to_owned(),
        hotkey_save_replay: "Alt+R".to_owned(),
        hotkey_start_replay: "Shift+Alt+R".to_owned(),
        hotkey_stop_replay: "Ctrl+Alt+R".to_owned(),
        hotkey_toggle_microphone_mute: "Ctrl+M".to_owned(),
        hotkey_toggle_desktop_mute: "Ctrl+Shift+M".to_owned(),
        hotkey_push_to_talk_microphone: "Ctrl+Space".to_owned(),
        hotkey_push_to_mute_microphone: "Ctrl+Alt+Space".to_owned(),
        hotkey_toggle_studio_mode: "Ctrl+Shift+S".to_owned(),
        hotkey_toggle_selected_source_visibility: "Ctrl+Shift+V".to_owned(),
        hotkey_toggle_selected_source_lock: "Ctrl+Shift+L".to_owned(),
        hotkey_toggle_selected_source_projector: "Ctrl+Shift+P".to_owned(),
        hotkey_toggle_preview_scene_projector: "Ctrl+Shift+R".to_owned(),
        preview_border_color: "#00FF88".to_owned(),
        last_preview_scene: "source_scene".to_owned(),
        last_program_scene: "program".to_owned(),
        project_scene_selections: vec![
            ProjectSceneSelection::new(
                "/tmp/a|b;c%ñ.obsrproj",
                "live",
                Some("source_scene".to_owned()),
                Some("program".to_owned()),
            ),
            ProjectSceneSelection::new(
                "/tmp/a|b;c%ñ.obsrproj",
                "alternate",
                Some("alternate_preview".to_owned()),
                Some("alternate_program".to_owned()),
            ),
        ],
        recording_format: RecordingFormat::ReferencePacket,
        recording_path: "/tmp/reference.obsr".to_owned(),
        stream_protocol: StreamProtocol::Rtmps,
        rtmp: RtmpConfig {
            service: "Example Live".to_owned(),
            server: "media.example/live".to_owned(),
            stream_key: SecretString::new("test-key"),
            video: VideoEncoderConfig {
                implementation: EncoderImplementation::new("nvh264enc"),
                rate_control: RateControl::Vbr,
                bitrate_kbps: 8_500,
                keyframe_interval_secs: 3,
                preset: EncoderPreset::Quality,
                profile: Some("main".to_owned()),
                b_frames: 3,
                ..VideoEncoderConfig::default()
            },
            audio: AudioEncoderConfig {
                implementation: EncoderImplementation::new("avenc_aac"),
                bitrate_kbps: 192,
                ..AudioEncoderConfig::default()
            },
            reconnect: false,
            maximum_retries: 7,
            network_buffer_ms: 2_500,
        },
        srt: SrtConfig {
            host: "srt.example".to_owned(),
            port: 10_000,
            mode: SrtMode::Rendezvous,
            latency_ms: 400,
            passphrase: Some(SecretString::new("long-enough-passphrase")),
            pbkeylen: Some(SrtKeyLength::Bits256),
            stream_id: Some("publish/feed".to_owned()),
            connect_timeout_ms: 12_000,
        },
        whip_bearer_token: Some(SecretString::new("private-whip-token")),
        rist: RistConfig {
            shared_secret: Some(SecretString::new("private-rist-secret")),
            ..RistConfig::default()
        },
        ..AppSettings::default()
    };

    let decoded = AppSettings::from_config(&settings.to_config());

    assert_eq!(decoded, settings);
    assert_eq!(decoded.sample_rate_hz(), 96_000);
    assert_eq!(decoded.channel_count(), 1);
}

#[test]
fn hotkeys_load_in_canonical_form_and_invalid_values_use_defaults() {
    let defaults = AppSettings::default();
    let mut config = defaults.to_config();
    config
        .set("hotkey_swap", " option + shift + f9 ")
        .expect("hotkey key");
    config
        .set("hotkey_start_recording", "Ctrl+Ctrl+R")
        .expect("hotkey key");
    config.set("hotkey_stop_recording", "").expect("hotkey key");
    config
        .set("hotkey_cut_transition", " option + c ")
        .expect("hotkey key");
    config
        .set("hotkey_previous_scene", " Shift + F6 ")
        .expect("hotkey key");
    config
        .set("hotkey_toggle_studio_mode", "Ctrl+Shift+S")
        .expect("hotkey key");
    config
        .set("hotkey_push_to_talk_microphone", "Ctrl+Space")
        .expect("hotkey key");
    config
        .set("hotkey_push_to_mute_microphone", "Ctrl+Alt+Space")
        .expect("hotkey key");
    config
        .set("hotkey_toggle_selected_source_visibility", "Ctrl+Shift+V")
        .expect("hotkey key");
    config
        .set("hotkey_toggle_selected_source_lock", "Ctrl+Shift+L")
        .expect("hotkey key");
    config
        .set("hotkey_toggle_selected_source_projector", "Ctrl+Shift+P")
        .expect("hotkey key");
    config
        .set("hotkey_toggle_preview_scene_projector", "Ctrl+Shift+R")
        .expect("hotkey key");

    let decoded = AppSettings::from_config(&config);

    assert_eq!(decoded.hotkey_swap, "Shift+Alt+F9");
    assert_eq!(
        decoded.hotkey_start_recording,
        defaults.hotkey_start_recording
    );
    assert!(decoded.hotkey_stop_recording.is_empty());
    assert_eq!(decoded.hotkey_cut_transition, "Alt+C");
    assert_eq!(decoded.hotkey_previous_scene, "Shift+F6");
    assert_eq!(decoded.hotkey_push_to_talk_microphone, "Ctrl+Space");
    assert_eq!(decoded.hotkey_push_to_mute_microphone, "Ctrl+Alt+Space");
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Ctrl+Shift+S"
            && *action == UiAction::ToggleStudioMode));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Ctrl+Space"
            && *action == UiAction::PushToTalkMicrophone));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(
            |(shortcut, action)| shortcut.to_string() == "Ctrl+Alt+Space"
                && *action == UiAction::PushToMuteMicrophone
        ));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Ctrl+Shift+V"
            && *action == UiAction::ToggleSelectedSourceVisibility));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Ctrl+Shift+L"
            && *action == UiAction::ToggleSelectedSourceLock));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Ctrl+Shift+P"
            && *action == UiAction::ToggleSelectedSourceProjector));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Ctrl+Shift+R"
            && *action == UiAction::TogglePreviewSceneProjector));
    assert!(shortcut_bindings(&decoded)
        .expect("shortcut bindings")
        .iter()
        .any(|(shortcut, action)| shortcut.to_string() == "Alt+C"
            && *action == UiAction::CutTransition));
    assert_eq!(validated_hotkey("ctrl + r", "F1"), "Ctrl+R");
    assert_eq!(validated_hotkey("Ctrl+", "F1"), "F1");
}

#[test]
fn hotkey_conflicts_use_canonical_shortcuts_and_ignore_unbindings() {
    let settings = AppSettings {
        hotkey_start_recording: "Ctrl+R".to_owned(),
        hotkey_stop_recording: "Alt+R".to_owned(),
        hotkey_start_streaming: String::new(),
        hotkey_undo: " control + r ".to_owned(),
        hotkey_redo: String::new(),
        hotkey_save_project: "CTRL+R".to_owned(),
        hotkey_cut_transition: String::new(),
        hotkey_fade_transition: String::new(),
        hotkey_save_replay: String::new(),
        hotkey_start_replay: String::new(),
        hotkey_stop_replay: String::new(),
        ..AppSettings::default()
    };

    assert_eq!(hotkey_conflicts(&settings), vec!["Ctrl+R"]);
}

#[test]
fn dock_tree_round_trips_without_losing_legacy_order() {
    let tree = DockNode::Split {
        axis: DockAxis::Vertical,
        ratio_milli: 625,
        first: Box::new(DockNode::Tabs {
            docks: vec![1, 0],
            active: 0,
        }),
        second: Box::new(DockNode::Split {
            axis: DockAxis::Horizontal,
            ratio_milli: 400,
            first: Box::new(DockNode::Dock(2)),
            second: Box::new(DockNode::Tabs {
                docks: vec![3, 4, 5],
                active: 1,
            }),
        }),
    };
    let mut settings = AppSettings::default();
    settings.layout.dock_tree = tree.clone();
    settings.layout.panel_order = tree.leaf_order();

    let decoded = AppSettings::from_config(&settings.to_config());

    assert_eq!(decoded.layout.dock_tree, tree);
    assert_eq!(decoded.layout.panel_order, vec![1, 0, 2, 3, 4, 5]);
}

#[test]
fn floating_geometry_round_trips_and_rejects_unsafe_records() {
    let mut settings = AppSettings::default();
    settings.layout.floating_geometry = vec![
        FloatingGeometry::new(2, -1_920, 84, 720, 520, 1_250).expect("valid geometry"),
        FloatingGeometry::new(4, 2_560, 120, 480, 700, 2_000).expect("valid geometry"),
    ];

    let decoded = AppSettings::from_config(&settings.to_config());

    assert_eq!(
        decoded.layout.floating_geometry,
        settings.layout.floating_geometry
    );

    let mut config = Config::new();
    config
        .set(
            "layout_floating_geometry",
            "v1:2:-1920:84:720:520:1250;4:0:0:99999:700:2000;bogus",
        )
        .expect("geometry key");
    let decoded = AppSettings::from_config(&config);
    assert_eq!(
        decoded.layout.floating_geometry,
        vec![FloatingGeometry::new(2, -1_920, 84, 720, 520, 1_250).expect("valid geometry")]
    );
}

#[test]
fn projector_geometry_round_trips_and_rejects_unsafe_records() {
    let mut settings = AppSettings::default();
    settings.layout.projector_geometry = vec![
        ProjectorGeometry::new(ProjectorKind::Preview, -1_920, 84, 960, 540, 1_250)
            .expect("valid geometry"),
        ProjectorGeometry::new(ProjectorKind::Scene, 2_560, 120, 1_280, 720, 2_000)
            .expect("valid geometry")
            .with_fullscreen(true)
            .with_open(true),
    ];

    let decoded = AppSettings::from_config(&settings.to_config());

    assert_eq!(
        decoded.layout.projector_geometry,
        settings.layout.projector_geometry
    );

    let mut config = Config::new();
    config
            .set(
                "layout_projector_geometry",
                "v1:preview:-1920:84:960:540:1250;scene:2560:120:1280:720:2000;preview:300:200:960:540:1250;source:0:0:99999:540:1250;unknown:0:0:960:540:1250",
            )
            .expect("geometry key");
    let decoded = AppSettings::from_config(&config);
    assert_eq!(
        decoded.layout.projector_geometry,
        vec![
            ProjectorGeometry::new(ProjectorKind::Preview, -1_920, 84, 960, 540, 1_250)
                .expect("valid geometry"),
            ProjectorGeometry::new(ProjectorKind::Scene, 2_560, 120, 1_280, 720, 2_000)
                .expect("valid geometry"),
        ]
    );

    config
            .set(
                "layout_projector_geometry",
                "v2:preview:-1920:84:960:540:1250:0;scene:2560:120:1280:720:2000:1;source:0:0:960:540:1250:9;multiview:0:0:960:540:1250:1;multiview:10:10:960:540:1250:0",
            )
            .expect("fullscreen geometry key");
    let decoded = AppSettings::from_config(&config);
    assert!(!decoded.layout.projector_geometry[0].fullscreen);
    assert!(decoded
        .layout
        .projector_geometry
        .iter()
        .find(|entry| entry.projector == ProjectorKind::Scene)
        .is_some_and(|entry| entry.fullscreen));
    assert_eq!(
        decoded
            .layout
            .projector_geometry
            .iter()
            .filter(|entry| entry.projector == ProjectorKind::Multiview)
            .count(),
        1,
        "the first valid duplicate wins"
    );
    assert!(decoded
        .layout
        .projector_geometry
        .iter()
        .find(|entry| entry.projector == ProjectorKind::Multiview)
        .is_some_and(|entry| entry.fullscreen));
    assert!(!decoded
        .layout
        .projector_geometry
        .iter()
        .find(|entry| entry.projector == ProjectorKind::Multiview)
        .is_some_and(|entry| entry.open));

    config
            .set(
                "layout_projector_geometry",
                "v3:preview:-1920:84:960:540:1250:0:1;scene:2560:120:1280:720:2000:1:1;source:0:0:960:540:1250:1:9;multiview:0:0:960:540:1250:1:1;multiview:10:10:960:540:1250:0:0",
            )
            .expect("projector lifecycle key");
    let decoded = AppSettings::from_config(&config);
    assert!(decoded
        .layout
        .projector_geometry
        .iter()
        .find(|entry| entry.projector == ProjectorKind::Preview)
        .is_some_and(|entry| entry.open));
    assert!(decoded
        .layout
        .projector_geometry
        .iter()
        .find(|entry| entry.projector == ProjectorKind::Scene)
        .is_some_and(|entry| entry.fullscreen && entry.open));
    assert_eq!(
        decoded
            .layout
            .projector_geometry
            .iter()
            .filter(|entry| entry.projector == ProjectorKind::Multiview)
            .count(),
        1,
        "the first valid duplicate wins"
    );
}

#[test]
fn projector_targets_round_trip_with_bounded_escaped_components() {
    let mut settings = AppSettings::default();
    settings.layout.projector_targets = vec![
        ProjectorTarget::Source {
            scene: "scene;one".to_owned(),
            item: "group|item".to_owned(),
        },
        ProjectorTarget::Scene {
            scene: "program".to_owned(),
        },
    ];
    let decoded = AppSettings::from_config(&settings.to_config());
    assert_eq!(
        decoded.layout.projector_targets,
        settings.layout.projector_targets
    );

    let mut config = Config::new();
    config
        .set(
            PROJECTOR_TARGETS_KEY,
            "v1;source|scene%3Bone|group%7Citem;scene|program;source|later|item;scene||bad",
        )
        .expect("projector target key");
    let decoded = AppSettings::from_config(&config);
    assert_eq!(
        decoded.layout.projector_targets,
        vec![
            ProjectorTarget::Source {
                scene: "scene;one".to_owned(),
                item: "group|item".to_owned(),
            },
            ProjectorTarget::Scene {
                scene: "program".to_owned(),
            },
        ]
    );
}

#[test]
fn projector_monitors_round_trip_with_bounded_escaped_ids() {
    let mut settings = AppSettings::default();
    settings.layout.projector_monitors = vec![
        ProjectorMonitor::new(ProjectorKind::Program, "HDMI-1".to_owned()).expect("valid monitor"),
        ProjectorMonitor::new(ProjectorKind::Preview, "DP-1|desk;left".to_owned())
            .expect("valid monitor"),
    ];
    let decoded = AppSettings::from_config(&settings.to_config());
    assert_eq!(
        decoded.layout.projector_monitors,
        settings.layout.projector_monitors
    );

    let mut config = Config::new();
    config
        .set(
            PROJECTOR_MONITORS_KEY,
            "v1;preview|DP-1%7Cdesk%3Bleft;program|HDMI-1;preview|later;scene||bad;unknown|DP-2",
        )
        .expect("projector monitor key");
    let decoded = AppSettings::from_config(&config);
    assert_eq!(
        decoded.layout.projector_monitors,
        vec![
            ProjectorMonitor::new(ProjectorKind::Program, "HDMI-1".to_owned())
                .expect("valid monitor"),
            ProjectorMonitor::new(ProjectorKind::Preview, "DP-1|desk;left".to_owned())
                .expect("valid monitor"),
        ]
    );
}

#[test]
fn appearance_video_and_output_round_trip_through_the_document() {
    let settings = AppSettings {
        style: UiStyle::Contrast,
        font_size: 16,
        density: UiDensity::Comfortable,
        output_mode: OutputMode::Advanced,
        stream_custom_encoder: true,
        recording_quality: RecordingQuality::IndistinguishableQuality,
        recording_directory: "/tmp/obs-rs-recordings".to_owned(),
        recording_filename_without_spaces: true,
        recording_auto_remux: true,
        recording_audio_encoder: EncoderImplementation::new("avenc_aac"),
        replay_buffer_duration_seconds: 90,
        replay_buffer_capacity_mib: 128,
        recording_split_enabled: true,
        recording_split_duration_minutes: 90,
        recording_split_size_mib: 128,
        recording_split_max_segments: 12,
        video: VideoSettings {
            base_width: 2_560,
            base_height: 1_440,
            output_width: 1_600,
            output_height: 900,
            scale_filter: ScaleFilter::Lanczos,
            fps_mode: FpsMode::Fractional,
            fps_numerator: 30_000,
            fps_denominator: 1_001,
        },
        ..AppSettings::default()
    };

    let decoded = AppSettings::from_config(&settings.to_config());

    assert_eq!(decoded, settings);
    assert_eq!(decoded.video.frame_rate().numerator(), 30_000);
    assert_eq!(decoded.video.frame_rate().denominator(), 1_001);
    assert!(!decoded.video.is_unscaled());
}

#[test]
fn the_shipped_defaults_match_the_reference_output_setup() {
    let settings = AppSettings::default();

    assert_eq!(settings.output_mode, OutputMode::Simple);
    assert_eq!(settings.video.base_width, 1_920);
    assert_eq!(settings.video.base_height, 1_080);
    assert_eq!(settings.video.output_width, 1_280);
    assert_eq!(settings.video.output_height, 720);
    assert_eq!(settings.video.scale_filter, ScaleFilter::Bicubic);
    assert_eq!(settings.video.fps_mode, FpsMode::Common);
    assert_eq!(settings.video.frame_rate().numerator(), 60);
    assert_eq!(settings.rtmp.video.bitrate_kbps, 6_000);
    assert_eq!(settings.rtmp.audio.bitrate_kbps, 160);
    assert_eq!(settings.density, UiDensity::Normal);
    assert_eq!(settings.font_size, DEFAULT_FONT_SIZE);
}

#[test]
fn appearance_and_video_values_outside_their_range_fall_back() {
    let mut config = AppSettings::default().to_config();
    for (key, value) in [
        ("appearance_font_size", "96"),
        ("appearance_density", "roomy"),
        ("appearance_style", "neon"),
        ("video_base_width", "0"),
        ("video_output_height", "99999"),
        ("video_scale_filter", "nearest"),
        ("video_fps_mode", "smpte"),
        ("canvas_snap_distance", "0"),
        ("show_safe_areas", "not-bool"),
        ("recording_quality", "perfect"),
        ("replay_buffer_duration_seconds", "0"),
        ("replay_buffer_capacity_mib", "999"),
        ("recording_split_duration_minutes", "0"),
        ("recording_split_size_mib", "999"),
        ("recording_split_max_segments", "0"),
        ("output_mode", "expert"),
    ] {
        config.set(key, value).expect("settings key");
    }

    let decoded = AppSettings::from_config(&config);
    let defaults = AppSettings::default();

    assert_eq!(decoded.font_size, defaults.font_size);
    assert_eq!(decoded.density, defaults.density);
    assert_eq!(decoded.style, defaults.style);
    assert_eq!(decoded.video, defaults.video);
    assert_eq!(decoded.recording_quality, defaults.recording_quality);
    assert_eq!(decoded.canvas_snap_distance, defaults.canvas_snap_distance);
    assert_eq!(decoded.show_safe_areas, defaults.show_safe_areas);
    assert_eq!(
        decoded.replay_buffer_duration_seconds,
        defaults.replay_buffer_duration_seconds
    );
    assert_eq!(
        decoded.replay_buffer_capacity_mib,
        defaults.replay_buffer_capacity_mib
    );
    assert_eq!(
        decoded.recording_split_duration_minutes,
        defaults.recording_split_duration_minutes
    );
    assert_eq!(
        decoded.recording_split_size_mib,
        defaults.recording_split_size_mib
    );
    assert_eq!(
        decoded.recording_split_max_segments,
        defaults.recording_split_max_segments
    );
    assert_eq!(decoded.output_mode, defaults.output_mode);
}

#[test]
fn a_document_written_before_these_settings_existed_still_loads() {
    // Everything the new pages own is absent here, which is exactly what a
    // settings file from an older build looks like.
    let mut config = Config::new();
    config.set("theme", "slate").expect("theme key");
    config.set("locale", "es").expect("locale key");

    let decoded = AppSettings::from_config(&config);
    let defaults = AppSettings::default();

    assert_eq!(decoded.theme, 3);
    assert_eq!(decoded.locale, "es");
    assert_eq!(decoded.video, defaults.video);
    assert_eq!(decoded.style, defaults.style);
    assert_eq!(decoded.output_mode, defaults.output_mode);
    assert_eq!(decoded.recording_quality, defaults.recording_quality);
}

#[test]
fn setup_state_round_trips_and_legacy_documents_do_not_open_the_wizard() {
    let settings = AppSettings {
        setup_state: SetupState::Skipped,
        setup_benchmark_summary: "recommended=720p30".to_owned(),
        ..AppSettings::default()
    };
    let decoded = AppSettings::from_config(&settings.to_config());
    assert_eq!(decoded.setup_state, SetupState::Skipped);
    assert_eq!(
        decoded.setup_benchmark_summary,
        settings.setup_benchmark_summary
    );

    let path = std::env::temp_dir().join("obs-rs-settings-legacy-setup-test.toml");
    let mut legacy = Config::new();
    legacy.set("theme", "dark").expect("legacy theme");
    std::fs::write(&path, legacy.serialize()).expect("write legacy settings");
    let loaded = AppSettings::load_with_status(&path);
    assert!(!loaded.show_setup);
    assert_eq!(loaded.settings.setup_state, SetupState::Completed);
    std::fs::remove_file(&path).expect("remove legacy settings");
}

#[test]
fn missing_settings_are_pending_first_run() {
    let path = std::env::temp_dir().join("obs-rs-settings-first-run-test.toml");
    let _ = std::fs::remove_file(&path);
    let loaded = AppSettings::load_with_status(&path);
    assert!(loaded.show_setup);
    assert_eq!(loaded.settings.setup_state, SetupState::Pending);
}

#[test]
fn unreadable_and_invalid_documents_fall_back_to_defaults() {
    let missing = std::env::temp_dir().join("obs-rs-settings-does-not-exist.toml");
    assert_eq!(
        AppSettings::load_with_status(&missing).settings,
        AppSettings::default()
    );

    let mut config = Config::new();
    config.set("theme", "not-a-theme").expect("theme key");
    config
        .set("audio_sample_rate", "12345")
        .expect("sample rate key");
    config
        .set("preview_border_color", "not-a-colour")
        .expect("colour key");
    let decoded = AppSettings::from_config(&config);

    assert_eq!(decoded.theme, AppSettings::default().theme);
    assert_eq!(decoded.sample_rate, AppSettings::default().sample_rate);
    assert_eq!(
        decoded.preview_border_color,
        AppSettings::default().preview_border_color
    );
}

#[test]
fn audio_sync_offsets_are_bounded_before_runtime_use() {
    let defaults = AppSettings::default();
    let mut config = defaults.to_config();
    config
        .set("audio_input_sync_offset_millis", "5001")
        .expect("input offset key");
    config
        .set("desktop_audio_sync_offset_millis", "-1")
        .expect("desktop offset key");

    let decoded = AppSettings::from_config(&config);

    assert_eq!(
        decoded.audio_input_sync_offset_millis,
        defaults.audio_input_sync_offset_millis
    );
    assert_eq!(
        decoded.desktop_audio_sync_offset_millis,
        defaults.desktop_audio_sync_offset_millis
    );
}

#[test]
fn audio_monitor_settings_reject_unknown_modes_and_round_trip_valid_values() {
    let settings = AppSettings {
        desktop_audio_id: "pipewire-output-42".to_owned(),
        audio_monitor_output_id: "pipewire-output-7".to_owned(),
        microphone_monitor_mode: AudioMonitorMode::MonitorOnly,
        desktop_audio_monitor_mode: AudioMonitorMode::MonitorAndOutput,
        ..AppSettings::default()
    };
    let mut config = settings.to_config();
    config
        .set("microphone_monitor_mode", "future_mode")
        .expect("future mode");
    let decoded = AppSettings::from_config(&config);

    assert_eq!(
        decoded.audio_monitor_output_id,
        settings.audio_monitor_output_id
    );
    assert_eq!(decoded.desktop_audio_id, settings.desktop_audio_id);
    assert_eq!(
        decoded.microphone_monitor_mode,
        AppSettings::default().microphone_monitor_mode
    );
    assert_eq!(
        decoded.desktop_audio_monitor_mode,
        settings.desktop_audio_monitor_mode
    );
    assert_eq!(
        audio_monitor_mode_from_id(audio_monitor_mode_id(AudioMonitorMode::MonitorOnly)),
        Some(AudioMonitorMode::MonitorOnly)
    );
}

#[test]
fn standard_audio_layout_indices_round_trip_by_channel_count() {
    let mut config = AppSettings::default().to_config();
    config.set("audio_channels", "6").expect("layout key");

    let decoded = AppSettings::from_config(&config);

    assert_eq!(decoded.channels, 4);
    assert_eq!(decoded.channel_count(), 6);
    assert_eq!(
        CHANNEL_LAYOUTS[decoded.channels],
        AudioChannelLayout::FivePointOne
    );

    config.set("audio_channels", "7").expect("discrete key");
    let fallback = AppSettings::from_config(&config);
    assert_eq!(fallback.channels, AppSettings::default().channels);
}

#[test]
fn default_stream_config_selects_the_production_rtmp_path() {
    assert_eq!(
        AppSettings::default().stream_endpoint().as_deref(),
        Some("rtmp://127.0.0.1/live/stream")
    );
}

#[test]
fn settings_fall_back_to_portable_output_when_native_backend_is_missing() {
    let mut settings = AppSettings::default();

    settings.adapt_to_output_capabilities(false);

    assert_eq!(settings.stream_protocol, StreamProtocol::Reference);
    assert_eq!(settings.recording_format, RecordingFormat::ReferencePacket);
    assert!(!settings.recording_auto_remux);
    assert_eq!(
        Path::new(&settings.recording_path)
            .extension()
            .and_then(|value| value.to_str()),
        Some("obsr")
    );
}

#[test]
fn production_settings_are_preserved_when_native_backend_is_ready() {
    let mut settings = AppSettings::default();

    settings.adapt_to_output_capabilities(true);

    assert_eq!(settings.stream_protocol, StreamProtocol::Rtmp);
    assert_eq!(settings.recording_format, RecordingFormat::Matroska);
    assert!(settings.recording_path.ends_with(".mkv"));
}

fn expected_recording_path(settings: &AppSettings, extension: &str) -> String {
    std::path::Path::new(&settings.recording_directory)
        .join(format!("2024-02-29 12-30-45.{extension}"))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn matroska_is_the_default_production_recording_format() {
    let settings = AppSettings::default();
    assert_eq!(settings.recording_format, RecordingFormat::Matroska);
    assert_eq!(
        Path::new(&settings.recording_path)
            .extension()
            .and_then(|value| value.to_str()),
        Some("mkv")
    );
}

#[test]
fn mp4_recording_format_selects_a_production_extension() {
    let settings = AppSettings {
        recording_format: RecordingFormat::Mp4,
        ..AppSettings::default()
    };
    assert_eq!(
        settings.recording_file_path("2024-02-29 12-30-45"),
        expected_recording_path(&settings, "mp4")
    );
}

#[test]
fn automatic_remux_selects_an_mp4_final_path_only_for_unsplit_matroska() {
    let settings = AppSettings {
        recording_auto_remux: true,
        ..AppSettings::default()
    };
    assert_eq!(
        settings.recording_file_path("2024-02-29 12-30-45"),
        expected_recording_path(&settings, "mp4")
    );

    let split = AppSettings {
        recording_split_enabled: true,
        ..settings.clone()
    };
    assert_eq!(
        Path::new(&split.recording_file_path("stamp"))
            .extension()
            .and_then(|value| value.to_str()),
        Some("mkv")
    );

    let lossless = AppSettings {
        recording_quality: RecordingQuality::Lossless,
        ..settings
    };
    assert_eq!(
        Path::new(&lossless.recording_file_path("stamp"))
            .extension()
            .and_then(|value| value.to_str()),
        Some("obsr")
    );
}

#[test]
fn fragmented_mp4_recording_format_selects_the_same_container_extension() {
    let settings = AppSettings {
        recording_format: RecordingFormat::FragmentedMp4,
        ..AppSettings::default()
    };
    assert_eq!(
        settings.recording_file_path("2024-02-29 12-30-45"),
        expected_recording_path(&settings, "mp4")
    );
    assert_eq!(
        RecordingFormat::from_id("fragmented-mp4"),
        Some(RecordingFormat::FragmentedMp4)
    );
}

#[test]
fn flv_recording_format_selects_a_production_extension() {
    let settings = AppSettings {
        recording_format: RecordingFormat::Flv,
        ..AppSettings::default()
    };
    assert_eq!(
        settings.recording_file_path("2024-02-29 12-30-45"),
        expected_recording_path(&settings, "flv")
    );
}

#[test]
fn mov_recording_format_selects_a_production_extension() {
    let settings = AppSettings {
        recording_format: RecordingFormat::Mov,
        ..AppSettings::default()
    };
    assert_eq!(
        settings.recording_file_path("2024-02-29 12-30-45"),
        expected_recording_path(&settings, "mov")
    );
}

#[test]
fn settings_debug_output_redacts_stream_secrets() {
    let settings = AppSettings {
        rtmp: RtmpConfig {
            stream_key: SecretString::new("private-stream-key"),
            ..RtmpConfig::default()
        },
        srt: SrtConfig {
            passphrase: Some(SecretString::new("private-passphrase")),
            ..SrtConfig::default()
        },
        ..AppSettings::default()
    };
    let debug = format!("{settings:?}");
    assert!(!debug.contains("private-stream-key"));
    assert!(!debug.contains("private-passphrase"));
    assert!(!debug.contains("private-whip-token"));
    assert!(!debug.contains("private-rist-secret"));
}

#[test]
fn recording_stamps_are_sortable_utc_civil_times() {
    let epoch = recording_stamp(std::time::UNIX_EPOCH);
    assert_eq!(epoch, "1970-01-01 00-00-00");

    let leap_day = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_709_209_845);
    assert_eq!(recording_stamp(leap_day), "2024-02-29 12-30-45");

    // A name generated without spaces must still be a legal file name and
    // must not lose any of the fields the stamp encodes.
    let settings = AppSettings {
        recording_directory: "/tmp".to_owned(),
        recording_filename_without_spaces: true,
        ..AppSettings::default()
    };
    let expected = Path::new("/tmp")
        .join("2024-02-29-12-30-45.mkv")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        settings.recording_file_path("2024-02-29 12-30-45"),
        expected
    );
    let spaced = AppSettings {
        recording_filename_without_spaces: false,
        ..settings
    };
    let expected_spaced = Path::new("/tmp")
        .join("2024-02-29 12-30-45.mkv")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        spaced.recording_file_path("2024-02-29 12-30-45"),
        expected_spaced
    );
}

#[test]
fn the_lossless_preset_forces_the_container_that_can_carry_it() {
    let settings = AppSettings {
        recording_quality: RecordingQuality::Lossless,
        recording_format: RecordingFormat::Matroska,
        recording_codec: VideoCodec::H264,
        ..AppSettings::default()
    };

    assert_eq!(
        settings.effective_recording_format(),
        RecordingFormat::ReferencePacket
    );
    assert_eq!(
        settings.effective_recording_codec(),
        VideoCodec::ReferenceRle
    );
    assert_eq!(
        Path::new(&settings.recording_file_path("stamp"))
            .extension()
            .and_then(|value| value.to_str()),
        Some("obsr")
    );
}

#[test]
fn styles_transform_the_theme_rather_than_replacing_it() {
    let default_style = AppSettings::default();
    let flat = AppSettings {
        style: UiStyle::Flat,
        ..AppSettings::default()
    };
    let contrast = AppSettings {
        style: UiStyle::Contrast,
        ..AppSettings::default()
    };

    // Flat merges the panel into the window; the default keeps them apart.
    assert_ne!(
        default_style.tokens().panel_bg,
        default_style.tokens().window_bg
    );
    assert_eq!(flat.tokens().panel_bg, flat.tokens().window_bg);
    // Contrast lifts the text away from the background it sits on.
    assert_ne!(contrast.tokens().text, default_style.tokens().text);
    assert_eq!(
        contrast.tokens().window_bg,
        default_style.tokens().window_bg
    );
}

#[test]
fn colour_parsing_accepts_hashed_and_bare_hex_only() {
    assert_eq!(parse_colour("#FF8800"), Some(colour([0xFF, 0x88, 0x00])));
    assert_eq!(parse_colour(" ff8800 "), Some(colour([0xFF, 0x88, 0x00])));
    assert_eq!(parse_colour("#FF88"), None);
    assert_eq!(parse_colour("#GGGGGG"), None);
}

#[test]
fn accessibility_colours_override_the_theme_preset() {
    let settings = AppSettings {
        program_border_color: "#00FF00".to_owned(),
        ..AppSettings::default()
    };

    let tokens = settings.tokens();

    assert_eq!(
        tokens.program_border,
        Brush::SolidColor(colour([0x00, 0xFF, 0x00]))
    );
}

#[test]
fn settings_document_persists_to_disk_and_reloads() {
    let path = std::env::temp_dir().join("obs-rs-settings-persist-test.toml");
    let settings = AppSettings {
        theme: 3,
        locale: "es".to_owned(),
        hotkey_start_streaming: "F9".to_owned(),
        program_border_color: "#123456".to_owned(),
        ..AppSettings::default()
    };

    settings.save(&path).expect("settings should persist");
    let reloaded = AppSettings::load_with_status(&path).settings;

    assert_eq!(reloaded, settings);
    assert_eq!(reloaded.ui_locale(), UiLocale::Spanish);
    std::fs::remove_file(&path).expect("remove settings fixture");
}
