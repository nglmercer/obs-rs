use super::*;

#[test]
fn dirty_native_window_close_is_kept_for_discard_prompt() {
    assert_eq!(
        close_request_response(true),
        CloseRequestResponse::KeepWindowShown
    );
    assert_eq!(
        close_request_response(false),
        CloseRequestResponse::HideWindow
    );
}

#[test]
fn stream_protocol_status_uses_redacted_scheme_labels() {
    let cases = [
        ("srt://media.example:9000?passphrase=top-secret", "SRT"),
        ("rtmp://media.example/live/top-secret", "RTMP"),
        ("rtmps://media.example/live/top-secret", "RTMPS"),
        ("ws://127.0.0.1:9000/private", "OBSR-WebSocket"),
        ("127.0.0.1:9000", "OBSR-TCP"),
    ];
    for (endpoint, expected) in cases {
        let label = stream_protocol_label(endpoint);
        assert_eq!(label, expected);
        assert!(!label.contains("top-secret"));
        assert!(!label.contains("media.example"));
    }
}

#[test]
fn output_runtime_exposes_backend_protocol_capabilities() {
    let format = initial_project()
        .expect("project")
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let output = OutputRuntime::new(format);
    assert!(output.capabilities().protocols().iter().any(|capability| {
        capability.protocol() == obs_rs_engine::ProductionProtocol::Reference
            && capability.available()
    }));
}

#[test]
fn output_runtime_projects_bounded_multiview_telemetry() {
    let format = initial_project()
        .expect("project")
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let output = OutputRuntime::new(format);
    let telemetry = output.multiview_telemetry();
    assert_eq!(telemetry.audio_peak_milli, 0);
    assert!(telemetry.metrics.contains("frames=0"));
    assert!(telemetry.metrics.contains("dropped=0"));
    assert!(telemetry.metrics.contains("queued=0 B"));
}

#[test]
fn output_runtime_applies_bounded_replay_configuration() {
    let format = initial_project()
        .expect("project")
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let mut output = OutputRuntime::new(format);
    let settings = AppSettings {
        replay_buffer_duration_seconds: 90,
        replay_buffer_capacity_mib: 128,
        ..AppSettings::default()
    };

    output.configure_replay(&settings);
    assert_eq!(output.replay_configuration_label(), "90 s / 128 MiB");

    let invalid = AppSettings {
        replay_buffer_duration_seconds: 0,
        replay_buffer_capacity_mib: u32::MAX,
        ..settings
    };
    output.configure_replay(&invalid);
    assert_eq!(output.replay_configuration_label(), "1 s / 256 MiB");
}

#[test]
fn output_runtime_applies_split_configuration_to_supported_recordings() {
    let format = initial_project()
        .expect("project")
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let mut output = OutputRuntime::new(format);
    let settings = AppSettings {
        recording_quality: RecordingQuality::Lossless,
        recording_split_enabled: true,
        recording_split_duration_minutes: 90,
        recording_split_size_mib: 128,
        recording_split_max_segments: 12,
        ..AppSettings::default()
    };

    output.configure_recording(&settings);
    let policy = output
        .segmented_recording_policy()
        .expect("reference recording should have a split policy");
    assert_eq!(policy.max_segment_duration().as_secs(), 90 * 60);
    assert_eq!(policy.max_segment_bytes(), 128 * 1024 * 1024);
    assert_eq!(policy.max_segments(), 12);

    let production = AppSettings {
        recording_split_enabled: true,
        ..AppSettings::default()
    };
    output.configure_recording(&production);
    let production_split_supported = output.capabilities().supports_segmented_recording()
        && output
            .capabilities()
            .recording_formats()
            .contains(&OutputProfileKind::MatroskaH264Aac);
    assert_eq!(
        output.segmented_recording_policy().is_some(),
        production_split_supported,
        "production split must follow the native capability boundary"
    );
}

#[test]
fn gui_project_has_control_room_scenes() {
    let project = initial_project().expect("initial GUI project should validate");
    let profile = project
        .profiles()
        .next()
        .expect("GUI project has a profile");
    let scenes = profile
        .scenes()
        .map(|scene| scene.id().to_string())
        .collect::<Vec<_>>();
    assert_eq!(scenes, ["intermission", "preview", "program"]);
}

#[test]
fn transition_labels_are_user_facing() {
    assert_eq!(
        transition_label_for_locale(UiLocale::English, FrameTransition::Cut),
        "Cut"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::English,
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
        ),
        "Fade 500/1000"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::Spanish,
            FrameTransition::FadeToColor {
                progress_milli: 500,
                color: [0, 255, 0, 255],
            },
        ),
        "Desvanecer a color 500/1000 #00FF00FF"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::English,
            FrameTransition::Slide {
                progress_milli: 500,
                direction: obs_rs_media::SlideDirection::Left,
            },
        ),
        "Slide 500/1000 (left)"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::English,
            FrameTransition::Swipe {
                progress_milli: 500,
                direction: obs_rs_media::SlideDirection::Left,
                swipe_in: false,
            },
        ),
        "Swipe 500/1000 (left)"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::English,
            FrameTransition::Swipe {
                progress_milli: 500,
                direction: obs_rs_media::SlideDirection::Left,
                swipe_in: true,
            },
        ),
        "Swipe In 500/1000 (left)"
    );
    assert_eq!(
        transition_label_for_locale(
            UiLocale::English,
            FrameTransition::LumaWipe {
                progress_milli: 500,
                pattern: obs_rs_media::LumaWipePattern::LinearVertical,
                invert: true,
                softness_milli: 85,
            },
        ),
        "Luma Wipe linear-v 500/1000 (softness 85, invert)"
    );
}

#[test]
fn mixer_peak_meter_uses_a_bounded_dbfs_scale() {
    assert!((peak_db(0) - -60.0).abs() < f32::EPSILON);
    assert!((peak_db(1_000) - 0.0).abs() < f32::EPSILON);
    assert!((peak_db(500) - -6.020_600_3).abs() < 0.001);
    assert!(
        (peak_db(1) - -60.0).abs() < f32::EPSILON,
        "the visible meter has a -60 dB floor"
    );
}

#[test]
fn gui_catalog_switches_complete_copy_between_supported_locales() {
    let english = catalog(UiLocale::English);
    let spanish = catalog(UiLocale::Spanish);
    assert_eq!(english.scenes_title, "Scenes");
    assert_eq!(spanish.scenes_title, "Escenas");
    assert_eq!(english.add_source, "Add source");
    assert_eq!(spanish.add_source, "Añadir fuente");
    assert_eq!(english.dont_save, "Don't save");
    assert_eq!(spanish.dont_save, "No guardar");
    assert_ne!(english.shortcuts, spanish.shortcuts);
}

#[test]
fn preview_renderer_uses_the_project_scene_sources() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let frame = wait_for_frame(|| renderer.render("preview"))
        .expect("preview scene should produce a frame");
    assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
}

#[test]
fn preview_renderer_composes_scene_transitions() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let frame = wait_for_frame(|| {
        renderer.render_transition(
            "preview",
            "program",
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
        )
    })
    .expect("transition should produce a frame");
    assert_eq!(frame.pixel(0, 0), Some([0x18, 0x28, 0x38, 0xff]));
}

#[test]
fn preview_renderer_advances_animated_capture_sources() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let first = wait_for_frame(|| renderer.render("intermission"))
        .expect("first pattern frame should render");
    let second = wait_for_frame(|| renderer.render("intermission"))
        .expect("second pattern frame should render");
    assert_ne!(first.pixels(), second.pixels());
}

#[test]
fn preview_renderer_reuses_static_scene_composition() {
    let project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    wait_for_frame(|| renderer.render("preview")).expect("static scene should render");
    let first = renderer.runtime.compositor_metrics();
    wait_for_frame(|| renderer.render("preview")).expect("cached static scene should render");
    let second = renderer.runtime.compositor_metrics();
    assert_eq!(second.render_calls(), first.render_calls());
    assert_eq!(second.source_requests(), first.source_requests());
}

#[test]
fn capture_source_defaults_have_a_real_selectable_device_id() {
    for kind in ["screen_capture", "window_capture", "camera_capture"] {
        let settings = source_settings(kind).expect("capture defaults");
        let device_id = settings.get("device_id").expect("device id");
        assert!(!device_id.is_empty(), "{kind} must have a device id");
        let devices = capture_devices(kind);
        if kind == "camera_capture" && devices.is_empty() {
            assert_eq!(device_id, "nokhwa-camera-0");
        } else {
            assert!(
                devices.iter().any(|(id, _)| id == device_id),
                "{kind} default must be in its device catalog"
            );
        }
    }
}

#[test]
fn only_screen_capture_kinds_offer_the_monitor_picker() {
    #[cfg(target_os = "linux")]
    {
        assert!(kind_selects_monitor("x11_screen_capture"));
        assert!(kind_selects_monitor("wayland_screen_capture"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        assert!(!kind_selects_monitor("x11_screen_capture"));
        assert!(!kind_selects_monitor("wayland_screen_capture"));
    }
    assert!(!kind_selects_monitor("camera_capture"));
    #[cfg(target_os = "windows")]
    assert!(kind_selects_monitor("screen_capture"));
    #[cfg(not(target_os = "windows"))]
    assert!(!kind_selects_monitor("screen_capture"));
}

#[test]
fn the_desktop_channel_names_its_monitor_or_admits_it_is_silent() {
    let format = VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);

    // Whether a playback monitor exists depends on the machine running the
    // suite, so the invariant under test is that the two answers stay
    // distinguishable: a named device, or nothing at all. What must never
    // happen is a channel that claims a device it is not reading.
    match output.desktop_audio_name() {
        Some(name) => assert!(!name.trim().is_empty(), "a captured monitor has a name"),
        None => assert!(
            output
                .diagnostics_document()
                .contains("desktop_audio_active=false"),
            "a silent desktop channel says so in diagnostics"
        ),
    }
}

#[test]
fn a_selected_audio_input_survives_disappearing_from_the_graph() {
    let format = VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    // An ID no graph will ever contain stands in for a device that has been
    // unplugged since it was chosen.
    let missing = "pipewire-node-999999";

    let entries = output.audio_input_entries(missing);

    let selected = entries
        .iter()
        .find(|entry| entry.id == missing)
        .expect("a missing selection must still be offered, not silently dropped");
    assert!(
        !selected.available,
        "the entry has to say the device is not connected"
    );
    assert!(
        entries.iter().filter(|entry| entry.id == missing).count() == 1,
        "the placeholder must not duplicate a device discovery already found"
    );

    // The automatic route is always resolvable, so it is never reported missing.
    assert!(output.audio_input_entries("").is_empty() || output.audio_input_available());
}

#[test]
fn a_canvas_change_applies_immediately_while_the_output_is_idle() {
    let (state, output) = canvas_fixture();
    let wider = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");

    crate::callbacks::settings::apply_video_format(&state, &output, wider)
        .expect("an idle session applies the change at once");

    assert_eq!(profile_video_format(&state), wider);
    assert_eq!(output.borrow().video_format(), wider);
}

#[test]
fn a_canvas_change_staged_during_output_applies_at_the_next_idle_tick() {
    let (state, output) = canvas_fixture();
    let original = profile_video_format(&state);
    let wider = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");

    output.borrow_mut().stage_video_format(wider);
    assert!(output.borrow().has_staged_video_format());
    assert_eq!(
        profile_video_format(&state),
        original,
        "staging must not touch the project while the output is running"
    );

    let applied = crate::callbacks::settings::apply_staged_video_format(&state, &output)
        .expect("a staged change is pending")
        .expect("the idle boundary applies it");

    assert_eq!(applied, wider);
    assert_eq!(profile_video_format(&state), wider);
    assert!(
        !output.borrow().has_staged_video_format(),
        "a staged change is applied exactly once"
    );
    assert!(
        crate::callbacks::settings::apply_staged_video_format(&state, &output).is_none(),
        "an idle tick with nothing staged must do no work"
    );
}

#[test]
fn the_encoders_receive_the_scaled_output_resolution() {
    let canvas = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(canvas);

    output
        .set_output_scaling(64, 36, ScaleFilter::Bicubic)
        .expect("an idle session rescales at once");

    assert_eq!(output.encoded_output_format().width(), 64);
    assert_eq!(output.encoded_output_format().height(), 36);
    assert_eq!(
        output.encoded_output_format().frame_rate().numerator(),
        30,
        "scaling changes geometry, never pacing"
    );
    assert!(
        !output.accepts_raw_frames(),
        "packed frames cannot be resampled, so the RGBA path has to be used"
    );

    // A canvas frame must be resampled rather than rejected: a format drop here
    // would mean the recording silently contained nothing.
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-scaled-{token}.obsr"));
    output
        .start_recording(&path.to_string_lossy())
        .expect("recording should open");
    output.push_frame(&VideoFrame::solid(canvas, Timestamp::ZERO, [9, 9, 9, 255]));
    let bytes = output
        .finish_recording()
        .expect("recording should finalize");

    assert!(bytes > 0, "the scaled frame has to reach the muxer");
    assert!(
        !output.output_metrics().contains("format_drops=1"),
        "a canvas frame must not be dropped while the output is scaled"
    );
    assert!(
        output.output_metrics().contains("av_sync_obs=1"),
        "output diagnostics expose the engine's bounded A/V observations"
    );
    std::fs::remove_file(path).expect("remove scaled fixture");
}

#[test]
fn an_output_scaling_change_staged_during_output_applies_at_the_next_idle_tick() {
    let (_state, output) = canvas_fixture();
    let original = output.borrow().encoded_output_format();

    output
        .borrow_mut()
        .stage_output_scaling(32, 18, ScaleFilter::Lanczos);
    assert_eq!(
        output.borrow().encoded_output_format(),
        original,
        "staging must not touch the encoders while the output is running"
    );

    let (width, height) = crate::callbacks::settings::apply_staged_output_scaling(&output)
        .expect("a staged change is pending")
        .expect("the idle boundary applies it");

    assert_eq!((width, height), (32, 18));
    assert_eq!(output.borrow().encoded_output_format().width(), 32);
    assert!(
        crate::callbacks::settings::apply_staged_output_scaling(&output).is_none(),
        "an idle tick with nothing staged must do no work"
    );
}

#[test]
fn a_canvas_change_the_engine_rejects_is_rolled_back() {
    let (state, output) = canvas_fixture();
    let original = profile_video_format(&state);
    let wider = VideoFormat::new(128, 72, FrameRate::new(30, 1).expect("rate")).expect("format");
    // An open recording makes the engine refuse a rebuild, which is the
    // failure the rollback exists for.
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-rollback-{token}.obsr"));
    output
        .borrow_mut()
        .start_recording(&path.to_string_lossy())
        .expect("open a recording");

    let error = crate::callbacks::settings::apply_video_format(&state, &output, wider)
        .expect_err("the engine refuses to rebuild under an open output");

    assert!(error.contains("restored"), "the message says what happened");
    assert_eq!(
        profile_video_format(&state),
        original,
        "the project must not keep a canvas the engine never adopted"
    );
    assert_eq!(output.borrow().video_format(), original);
    output.borrow_mut().abort_recording();
}

#[test]
#[ignore = "requires a live Wayland or X11 compositor for MainWindow::new"]
fn a_failed_output_clears_the_claim_and_offers_guided_recovery() {
    let ui = MainWindow::new().expect("window");
    let (state, output) = canvas_fixture();
    // The desktop optimistically believes it is streaming; the engine never
    // connected. Reconciling is what turns that mismatch into an answer the
    // operator can act on rather than a control stuck in the on position.
    state
        .borrow_mut()
        .dispatch(obs_rs_ui::UiCommand::StartStreaming)
        .expect("claim streaming");
    ui.set_streaming(true);
    output
        .borrow_mut()
        .start_streaming("127.0.0.1:1")
        .expect("the non-blocking start request is accepted");

    // The worker publishes its snapshot after replying, so the failure can be
    // one tick behind the rejected connect. In the app that only delays the
    // dialog by a frame; here it has to be waited for.
    for _ in 0..100 {
        if output.borrow().lifecycles().1 == obs_rs_engine::OutputLifecycle::Failed {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    crate::callbacks::reconcile_output_lifecycle(&ui, &state, &output);

    assert!(!ui.get_streaming(), "the control stops claiming an output");
    assert!(!state.borrow().streaming());
    assert_eq!(
        ui.get_active_modal(),
        11,
        "a failure opens the recovery dialog rather than only logging a status line"
    );
    assert!(!ui.get_status_message().is_empty());
}
