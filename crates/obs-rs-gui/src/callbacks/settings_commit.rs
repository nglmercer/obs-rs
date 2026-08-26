#[allow(
    clippy::wildcard_imports,
    reason = "settings commit callbacks share the controller boundary imports"
)]
use super::*;

pub(super) fn install_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let apply_state = Rc::clone(state);
    let apply_surface = Rc::clone(surface);
    let apply_output = Rc::clone(output);
    let apply_controller = Rc::clone(controller);
    controller.window.on_apply_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        commit(
            &ui,
            &apply_state,
            &apply_surface,
            &apply_output,
            &apply_controller,
        );
    });

    // Re-running discovery is the hot-plug path: a device connected after the
    // window opened is only visible once `pw-dump` is asked again, and the
    // draft selection is preserved across the rebuild.
    let refresh_state = Rc::clone(state);
    let refresh_output = Rc::clone(output);
    let refresh_controller = Rc::clone(controller);
    controller.window.on_refresh_audio_devices(move || {
        refresh_output.borrow_mut().refresh_audio_devices();
        // The picker's current choice, not the committed one, is what the user
        // is in the middle of making, so it is what survives the rebuild.
        let draft = draft_audio_input_id(&refresh_controller);
        let draft_monitor_output = draft_audio_monitor_output_id(&refresh_controller);
        let settings = AppSettings {
            audio_input_id: draft,
            audio_monitor_output_id: draft_monitor_output,
            microphone_monitor_mode: draft_monitor_mode(
                refresh_controller
                    .window
                    .get_microphone_monitor_mode_index(),
            ),
            desktop_audio_monitor_mode: draft_monitor_mode(
                refresh_controller.window.get_desktop_monitor_mode_index(),
            ),
            ..refresh_controller.settings.borrow().clone()
        };
        populate_audio_devices(
            &refresh_controller.window,
            &refresh_state,
            &refresh_output,
            &refresh_controller,
            &settings,
        );
    });

    let weak = ui.as_weak();
    let accept_state = Rc::clone(state);
    let accept_surface = Rc::clone(surface);
    let accept_output = Rc::clone(output);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        commit(
            &ui,
            &accept_state,
            &accept_surface,
            &accept_output,
            &accept_controller,
        );
        let _ = accept_controller.window.hide();
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
    let cancel_controller = Rc::clone(controller);
    controller.window.on_cancel_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Theme and language were previewed live, so Cancel has to put the
        // committed document back rather than merely closing the window.
        let committed = cancel_controller.settings.borrow().clone();
        push_palette(&ui, &cancel_controller, &committed);
        let locale = committed.ui_locale();
        if state.borrow().locale() != locale
            && state
                .borrow_mut()
                .dispatch(UiCommand::SetLocale { locale })
                .is_ok()
        {
            refresh_ui(&ui, &state, &surface);
        }
        cancel_controller.sync_theme(locale);
        cancel_controller.window.set_dirty(false);
        let _ = cancel_controller.window.hide();
    });
}

/// Applies a canvas change to the active profile and rebuilds the engine.
///
/// The two halves have to agree: the project is the record of what the canvas
/// is, and the engine is what encodes at that size. If the engine cannot be
/// rebuilt the project change is rolled back, because leaving a project that
/// claims one resolution while the engine encodes another produces a recording
/// whose geometry matches neither.
///
/// # Errors
///
/// Returns the reason the change could not be applied, after the rollback.
pub(crate) fn apply_video_format(
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
    format: VideoFormat,
) -> Result<(), String> {
    let (profile, previous) = {
        let state = state.borrow();
        let project = state.project_session().project();
        (
            project.active_profile().to_string(),
            project
                .active_profile_spec()
                .map(obs_rs_project::Profile::video_format),
        )
    };
    if previous == Some(format) {
        return Ok(());
    }
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetProfileVideoFormat {
            profile: profile.clone(),
            format,
        }))
        .map_err(|error| error.to_string())?;

    let (project, revision) = {
        let state = state.borrow();
        (
            state.project_session().project().clone(),
            state.project_session().revision(),
        )
    };
    let Err(error) = output.borrow_mut().sync_project(project, revision) else {
        return Ok(());
    };

    let reason = error.to_string();
    // The rollback is itself a project command, so it is undoable and the
    // engine picks it up on the next idle sync exactly like any other edit.
    if let Some(previous) = previous {
        let rolled_back = state.borrow_mut().dispatch(UiCommand::Project(
            ProjectCommand::SetProfileVideoFormat {
                profile,
                format: previous,
            },
        ));
        if let Err(rollback) = rolled_back {
            return Err(format!("{reason}; rollback also failed: {rollback}"));
        }
        return Err(format!("{reason}; the previous canvas was restored"));
    }
    Err(reason)
}

/// The settings pages by name, in sidebar order.
///
/// The screenshot harness names pages rather than numbering them, so a
/// reordered sidebar cannot silently change which page a golden image holds.
pub(crate) const SETTINGS_PAGES: [(&str, i32); 9] = [
    ("general", 0),
    ("appearance", 1),
    ("stream", 2),
    ("output", 3),
    ("audio", 4),
    ("video", 5),
    ("hotkeys", 6),
    ("accessibility", 7),
    ("advanced", 8),
];

/// Returns the sidebar index for a page name.
pub(crate) fn settings_category(page: &str) -> Option<i32> {
    SETTINGS_PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, index)| *index)
}

/// The Video category's index in the settings window's sidebar.
///
/// Validation failures switch to the page carrying the invalid field, so the
/// user is looking at the row that stopped the commit.
const SETTINGS_CATEGORY_VIDEO: i32 = 5;

/// The Hotkeys category's index in the settings window's sidebar.
const SETTINGS_CATEGORY_HOTKEYS: i32 = 6;

/// Applies an output-scaling change that was staged while an output was
/// running.
///
/// Called from the same idle boundary as the staged canvas change, which is
/// the first moment the encoders can safely be rebuilt.
pub(crate) fn apply_staged_output_scaling(
    output: &Rc<RefCell<OutputRuntime>>,
) -> Option<Result<(u32, u32), String>> {
    let (width, height, filter) = output.borrow_mut().take_staged_output_scaling()?;
    Some(
        output
            .borrow_mut()
            .set_output_scaling(width, height, filter)
            .map(|()| (width, height))
            .map_err(|error| error.to_string()),
    )
}

/// Applies a canvas change that was staged while an output was running.
///
/// Called from the idle boundary in the preview timer, which is the first
/// moment the encoders can safely be rebuilt.
pub(crate) fn apply_staged_video_format(
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) -> Option<Result<VideoFormat, String>> {
    let format = output.borrow_mut().take_staged_video_format()?;
    Some(apply_video_format(state, output, format).map(|()| format))
}

/// Applies an audio-format change that was staged while an output was
/// running.
///
/// Called from the same idle boundary as the staged video change, after the
/// old recording/streaming container has been closed.
pub(crate) fn apply_staged_audio_format(
    output: &Rc<RefCell<OutputRuntime>>,
) -> Option<Result<AudioFormat, String>> {
    let format = output.borrow_mut().take_staged_audio_format()?;
    Some(
        output
            .borrow_mut()
            .set_audio_format(format)
            .map(|()| format)
            .map_err(|error| error.to_string()),
    )
}

/// Reads every editable field out of the window into a settings document.
///
/// Committing is two distinct jobs — collecting the draft and acting on it —
/// and separating them keeps the "what did the user type" logic away from the
/// "what does that change in the running session" logic.
#[allow(
    clippy::too_many_lines,
    reason = "the settings draft is read into one validated application document"
)]
pub(super) fn read_draft(controller: &SettingsController) -> AppSettings {
    let window = &controller.window;
    let mut settings = controller.settings.borrow().clone();
    settings.theme = usize::try_from(window.get_theme_index())
        .unwrap_or(0)
        .min(THEMES.len() - 1);
    settings.style = draft_style(window);
    settings.density = draft_density(window);
    settings.font_size = draft_font_size(window);
    settings.video = read_video_draft(controller);
    settings.confirm_start_stream = window.get_confirm_start_stream();
    settings.confirm_stop_stream = window.get_confirm_stop_stream();
    settings.confirm_stop_recording = window.get_confirm_stop_recording();
    settings.auto_record_when_streaming = window.get_auto_record_when_streaming();
    read_canvas_draft(window, &mut settings);
    settings.sample_rate = usize::try_from(window.get_sample_rate_index())
        .unwrap_or(0)
        .min(SAMPLE_RATES.len() - 1);
    settings.channels = usize::try_from(window.get_channel_index())
        .unwrap_or(0)
        .min(CHANNEL_LAYOUTS.len() - 1);
    settings.audio_input_sync_offset_millis =
        unsigned(window.get_audio_input_sync_offset()).min(*AUDIO_SYNC_OFFSET_RANGE.end());
    settings.desktop_audio_sync_offset_millis =
        unsigned(window.get_desktop_audio_sync_offset()).min(*AUDIO_SYNC_OFFSET_RANGE.end());
    settings.microphone_monitor_mode =
        draft_monitor_mode(window.get_microphone_monitor_mode_index());
    settings.desktop_audio_monitor_mode =
        draft_monitor_mode(window.get_desktop_monitor_mode_index());
    read_hotkey_draft(window, &mut settings);
    settings.preview_border_color = window.get_preview_border_color().to_string();
    settings.program_border_color = window.get_program_border_color().to_string();
    settings.project_path = window.get_project_path().to_string();
    settings.diagnostics_path = window.get_diagnostics_path().to_string();
    read_recording_draft(controller, &mut settings);
    settings.stream_protocol = controller
        .protocol_ids
        .borrow()
        .get(usize::try_from(window.get_protocol_index()).unwrap_or(0))
        .copied()
        .unwrap_or(StreamProtocol::Reference);
    if let Some(service) =
        RTMP_SERVICE_PRESETS.get(usize::try_from(window.get_rtmp_service_index()).unwrap_or(0))
    {
        service.id().clone_into(&mut settings.rtmp.service);
    }
    settings.rtmp.server = window.get_rtmp_server().to_string();
    settings.rtmp.stream_key = SecretString::new(window.get_rtmp_stream_key().to_string());
    if let Some(encoder) = controller
        .video_encoder_ids
        .borrow()
        .get(usize::try_from(window.get_video_encoder_index()).unwrap_or(0))
    {
        settings.rtmp.video.implementation = EncoderImplementation::new(encoder.clone());
    }
    if let Some(encoder) = controller
        .audio_encoder_ids
        .borrow()
        .get(usize::try_from(window.get_audio_encoder_index()).unwrap_or(0))
    {
        settings.rtmp.audio.implementation = EncoderImplementation::new(encoder.clone());
    }
    settings.rtmp.video.bitrate_kbps = unsigned(window.get_stream_video_bitrate());
    settings.rtmp.audio.bitrate_kbps = unsigned(window.get_stream_audio_bitrate());
    settings.rtmp.video.rate_control =
        RateControl::from_id(&window.get_stream_rate_control()).unwrap_or(RateControl::Cbr);
    settings.rtmp.video.keyframe_interval_secs = unsigned(window.get_stream_keyframe_interval());
    settings.rtmp.video.preset = [
        EncoderPreset::Speed,
        EncoderPreset::Balanced,
        EncoderPreset::Quality,
    ]
    .get(usize::try_from(window.get_encoder_preset_index()).unwrap_or(1))
    .copied()
    .unwrap_or(EncoderPreset::Balanced);
    settings.output_mode = OutputMode::ALL
        .get(usize::try_from(window.get_output_mode_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    settings.stream_custom_encoder = window.get_custom_encoder_settings();
    settings.rtmp.video.profile = nonempty(window.get_stream_encoder_profile().to_string());
    settings.rtmp.video.b_frames = u8::try_from(window.get_stream_b_frames()).unwrap_or(0);
    settings.rtmp.reconnect = window.get_stream_reconnect();
    settings.rtmp.maximum_retries = unsigned(window.get_stream_maximum_retries());
    settings.rtmp.network_buffer_ms = unsigned(window.get_stream_network_buffer());
    read_connection_draft(window, &mut settings);
    settings.restore_project = window.get_restore_project();
    settings.save_project_on_exit = window.get_save_project_on_exit();
    if let Some(device_id) = controller
        .audio_device_ids
        .borrow()
        .get(usize::try_from(window.get_audio_device_index()).unwrap_or(0))
    {
        device_id.clone_into(&mut settings.audio_input_id);
    } else {
        settings.audio_input_id.clear();
    }
    if let Some(device_id) = controller
        .audio_monitor_output_ids
        .borrow()
        .get(usize::try_from(window.get_audio_monitor_output_index()).unwrap_or(0))
    {
        device_id.clone_into(&mut settings.audio_monitor_output_id);
    } else {
        settings.audio_monitor_output_id.clear();
    }
    if let Some(locale) = UiLocale::supported()
        .get(usize::try_from(window.get_language_index()).unwrap_or(0))
        .copied()
    {
        locale.code().clone_into(&mut settings.locale);
    }
    settings
}

/// Reads the hotkey fields through the shared typed parser before Apply can
/// publish them to the persistent settings document.
pub(super) fn read_hotkey_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.hotkey_swap =
        crate::settings::validated_hotkey(window.get_hotkey_swap().as_str(), &settings.hotkey_swap);
    settings.hotkey_previous_scene = crate::settings::validated_hotkey(
        window.get_hotkey_previous_scene().as_str(),
        &settings.hotkey_previous_scene,
    );
    settings.hotkey_next_scene = crate::settings::validated_hotkey(
        window.get_hotkey_next_scene().as_str(),
        &settings.hotkey_next_scene,
    );
    settings.hotkey_start_recording = crate::settings::validated_hotkey(
        window.get_hotkey_start_recording().as_str(),
        &settings.hotkey_start_recording,
    );
    settings.hotkey_stop_recording = crate::settings::validated_hotkey(
        window.get_hotkey_stop_recording().as_str(),
        &settings.hotkey_stop_recording,
    );
    settings.hotkey_start_streaming = crate::settings::validated_hotkey(
        window.get_hotkey_start_streaming().as_str(),
        &settings.hotkey_start_streaming,
    );
    settings.hotkey_stop_streaming = crate::settings::validated_hotkey(
        window.get_hotkey_stop_streaming().as_str(),
        &settings.hotkey_stop_streaming,
    );
    settings.hotkey_undo =
        crate::settings::validated_hotkey(window.get_hotkey_undo().as_str(), &settings.hotkey_undo);
    settings.hotkey_redo =
        crate::settings::validated_hotkey(window.get_hotkey_redo().as_str(), &settings.hotkey_redo);
    settings.hotkey_save_project = crate::settings::validated_hotkey(
        window.get_hotkey_save_project().as_str(),
        &settings.hotkey_save_project,
    );
    settings.hotkey_cut_transition = crate::settings::validated_hotkey(
        window.get_hotkey_cut_transition().as_str(),
        &settings.hotkey_cut_transition,
    );
    settings.hotkey_fade_transition = crate::settings::validated_hotkey(
        window.get_hotkey_fade_transition().as_str(),
        &settings.hotkey_fade_transition,
    );
    settings.hotkey_save_replay = crate::settings::validated_hotkey(
        window.get_hotkey_save_replay().as_str(),
        &settings.hotkey_save_replay,
    );
    settings.hotkey_start_replay = crate::settings::validated_hotkey(
        window.get_hotkey_start_replay().as_str(),
        &settings.hotkey_start_replay,
    );
    settings.hotkey_stop_replay = crate::settings::validated_hotkey(
        window.get_hotkey_stop_replay().as_str(),
        &settings.hotkey_stop_replay,
    );
    settings.hotkey_toggle_microphone_mute = crate::settings::validated_hotkey(
        window.get_hotkey_toggle_microphone_mute().as_str(),
        &settings.hotkey_toggle_microphone_mute,
    );
    settings.hotkey_toggle_desktop_mute = crate::settings::validated_hotkey(
        window.get_hotkey_toggle_desktop_mute().as_str(),
        &settings.hotkey_toggle_desktop_mute,
    );
    settings.hotkey_push_to_talk_microphone = crate::settings::validated_hotkey(
        window.get_hotkey_push_to_talk_microphone().as_str(),
        &settings.hotkey_push_to_talk_microphone,
    );
    settings.hotkey_push_to_mute_microphone = crate::settings::validated_hotkey(
        window.get_hotkey_push_to_mute_microphone().as_str(),
        &settings.hotkey_push_to_mute_microphone,
    );
    settings.hotkey_toggle_studio_mode = crate::settings::validated_hotkey(
        window.get_hotkey_toggle_studio_mode().as_str(),
        &settings.hotkey_toggle_studio_mode,
    );
    settings.hotkey_toggle_selected_source_visibility = crate::settings::validated_hotkey(
        window
            .get_hotkey_toggle_selected_source_visibility()
            .as_str(),
        &settings.hotkey_toggle_selected_source_visibility,
    );
    settings.hotkey_toggle_selected_source_lock = crate::settings::validated_hotkey(
        window.get_hotkey_toggle_selected_source_lock().as_str(),
        &settings.hotkey_toggle_selected_source_lock,
    );
    settings.hotkey_toggle_selected_source_projector = crate::settings::validated_hotkey(
        window
            .get_hotkey_toggle_selected_source_projector()
            .as_str(),
        &settings.hotkey_toggle_selected_source_projector,
    );
    settings.hotkey_toggle_preview_scene_projector = crate::settings::validated_hotkey(
        window.get_hotkey_toggle_preview_scene_projector().as_str(),
        &settings.hotkey_toggle_preview_scene_projector,
    );
}

/// Reads the canvas-only settings as one validated presentation policy.
pub(super) fn read_canvas_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.canvas_snap_distance = u16::try_from(window.get_snap_distance())
        .unwrap_or(CANVAS_SNAP_DISTANCE_DEFAULT)
        .clamp(
            *CANVAS_SNAP_DISTANCE_RANGE.start(),
            *CANVAS_SNAP_DISTANCE_RANGE.end(),
        );
    settings.show_safe_areas = window.get_show_safe_areas();
}

/// Reads the protocol-specific connection fields out of the draft.
pub(super) fn read_connection_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.srt.mode = match window.get_srt_mode_index() {
        1 => SrtMode::Listener,
        2 => SrtMode::Rendezvous,
        _ => SrtMode::Caller,
    };
    settings.srt.host = window.get_srt_host().to_string();
    settings.srt.port = u16::try_from(window.get_srt_port()).unwrap_or(9_000);
    settings.srt.latency_ms = unsigned(window.get_srt_latency());
    settings.srt.passphrase =
        nonempty(window.get_srt_passphrase().to_string()).map(SecretString::new);
    settings.srt.pbkeylen = match window.get_srt_key_length_index() {
        1 => Some(SrtKeyLength::Bits128),
        2 => Some(SrtKeyLength::Bits192),
        3 => Some(SrtKeyLength::Bits256),
        _ => None,
    };
    settings.srt.stream_id = nonempty(window.get_srt_stream_id().to_string());
    settings.srt.connect_timeout_ms = unsigned(window.get_srt_timeout());
    settings.whip_endpoint = window.get_whip_endpoint().to_string();
    read_extended_stream_draft(window, settings);
    settings.reference_address = window.get_reference_address().to_string();
}

pub(super) fn load_extended_stream_draft(window: &SettingsWindow, settings: &AppSettings) {
    window.set_whip_bearer_token(
        settings
            .whip_bearer_token
            .as_ref()
            .map_or("", SecretString::expose_secret)
            .into(),
    );
    window.set_hls_directory(settings.hls.directory.to_string_lossy().as_ref().into());
    window.set_hls_segment_duration(
        i32::try_from(settings.hls.segment_duration_secs).unwrap_or(i32::MAX),
    );
    window.set_hls_playlist_size(i32::try_from(settings.hls.playlist_size).unwrap_or(i32::MAX));
    window.set_hls_low_latency(settings.hls.low_latency);
    window.set_rist_host(settings.rist.host.as_str().into());
    window.set_rist_port(i32::from(settings.rist.port));
    window.set_rist_buffer(i32::try_from(settings.rist.sender_buffer_ms).unwrap_or(i32::MAX));
    window.set_rist_shared_secret(
        settings
            .rist
            .shared_secret
            .as_ref()
            .map_or("", SecretString::expose_secret)
            .into(),
    );
}

pub(super) fn read_extended_stream_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.whip_bearer_token =
        nonempty(window.get_whip_bearer_token().to_string()).map(SecretString::new);
    settings.hls.directory = PathBuf::from(window.get_hls_directory().to_string());
    settings.hls.segment_duration_secs = unsigned(window.get_hls_segment_duration());
    settings.hls.playlist_size = unsigned(window.get_hls_playlist_size());
    settings.hls.low_latency = window.get_hls_low_latency();
    settings.rist.host = window.get_rist_host().to_string();
    settings.rist.port = u16::try_from(window.get_rist_port()).unwrap_or(0);
    settings.rist.sender_buffer_ms = unsigned(window.get_rist_buffer());
    settings.rist.shared_secret =
        nonempty(window.get_rist_shared_secret().to_string()).map(SecretString::new);
}

pub(super) fn read_recording_draft(controller: &SettingsController, settings: &mut AppSettings) {
    let window = &controller.window;
    settings.recording_quality = draft_recording_quality(window);
    settings.recording_format = controller
        .recording_format_ids
        .borrow()
        .get(usize::try_from(window.get_recording_format_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    settings.recording_directory = window.get_recording_directory().to_string();
    settings.recording_filename_without_spaces = window.get_recording_filename_without_spaces();
    settings.recording_auto_remux = window.get_recording_auto_remux();
    settings.replay_buffer_duration_seconds = u32::try_from(window.get_replay_buffer_duration())
        .ok()
        .filter(|duration| REPLAY_BUFFER_DURATION_RANGE.contains(duration))
        .unwrap_or(REPLAY_BUFFER_DURATION_DEFAULT);
    settings.replay_buffer_capacity_mib = u32::try_from(window.get_replay_buffer_capacity())
        .ok()
        .filter(|capacity| REPLAY_BUFFER_CAPACITY_MIB_RANGE.contains(capacity))
        .unwrap_or(REPLAY_BUFFER_CAPACITY_MIB_DEFAULT);
    settings.recording_split_enabled = window.get_recording_split_enabled();
    settings.recording_split_duration_minutes =
        u32::try_from(window.get_recording_split_duration_minutes())
            .ok()
            .filter(|duration| RECORDING_SPLIT_DURATION_MINUTES_RANGE.contains(duration))
            .unwrap_or(RECORDING_SPLIT_DURATION_MINUTES_DEFAULT);
    settings.recording_split_size_mib = u32::try_from(window.get_recording_split_size_mib())
        .ok()
        .filter(|capacity| RECORDING_SPLIT_SIZE_MIB_RANGE.contains(capacity))
        .unwrap_or(RECORDING_SPLIT_SIZE_MIB_DEFAULT);
    settings.recording_split_max_segments =
        u32::try_from(window.get_recording_split_max_segments())
            .ok()
            .filter(|segments| RECORDING_SPLIT_SEGMENTS_RANGE.contains(segments))
            .unwrap_or(RECORDING_SPLIT_SEGMENTS_DEFAULT);
    // The concrete file is derived here rather than typed, so the name, the
    // extension, and the container always agree. The studio's own start-
    // recording dialog can still edit the path afterwards.
    settings.recording_path =
        settings.recording_file_path(&recording_stamp(std::time::SystemTime::now()));
    if let Some(codec) = controller
        .recording_codec_ids
        .borrow()
        .get(usize::try_from(window.get_recording_codec_index()).unwrap_or(0))
        .copied()
    {
        settings.recording_codec = codec;
    }
    if let Some(encoder) = controller
        .recording_audio_encoder_ids
        .borrow()
        .get(usize::try_from(window.get_recording_audio_encoder_index()).unwrap_or(0))
    {
        settings.recording_audio_encoder = EncoderImplementation::new(encoder.clone());
    }
}

/// Reads the Video page's draft, taking the frame rate from whichever control
/// the selected FPS mode shows.
pub(super) fn read_video_draft(controller: &SettingsController) -> VideoSettings {
    let window = &controller.window;
    let mut video = *controller.draft_video.borrow();
    video.scale_filter = ScaleFilter::ALL
        .get(usize::try_from(window.get_scale_filter_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    video.fps_mode = FpsMode::ALL
        .get(usize::try_from(window.get_fps_mode_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    match video.fps_mode {
        FpsMode::Common => {}
        FpsMode::Integer => {
            video.fps_numerator = unsigned(window.get_fps_numerator()).max(1);
            video.fps_denominator = 1;
        }
        FpsMode::Fractional => {
            video.fps_numerator = unsigned(window.get_fps_numerator()).max(1);
            video.fps_denominator = unsigned(window.get_fps_denominator()).max(1);
        }
    }
    video
}

pub(super) fn commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let window = &controller.window;
    // Validation runs before anything is written: a rejected field leaves the
    // window open with the offending row marked and commits nothing at all,
    // rather than persisting the settings that happened to be readable.
    if !window.get_base_resolution_valid() || !window.get_output_resolution_valid() {
        window.set_category(SETTINGS_CATEGORY_VIDEO);
        ui.set_status_message(
            crate::i18n::with_catalog(controller.settings.borrow().ui_locale(), |text| {
                text.settings_ui.resolution_invalid.clone()
            })
            .to_string()
            .into(),
        );
        return;
    }
    let settings = read_draft(controller);
    let conflicts = hotkey_conflicts(&settings);
    if !conflicts.is_empty() {
        let conflict_list = conflicts.join(", ");
        window.set_category(SETTINGS_CATEGORY_HOTKEYS);
        window.set_hotkeys_conflict(conflict_list.as_str().into());
        ui.set_status_message(
            crate::i18n::with_catalog(controller.settings.borrow().ui_locale(), |text| {
                format!("{}: {conflict_list}", text.settings_ui.hotkeys_conflict)
            })
            .into(),
        );
        return;
    }
    window.set_hotkeys_conflict("".into());
    apply_settings_snapshot(ui, state, surface, output, controller, &settings);
}

/// Applies a validated settings snapshot from either the settings draft or the
/// first-run setup wizard.
///
/// Setup deliberately comes through the same path as the ordinary settings
/// window so audio routing, output staging, project video geometry, palette,
/// and persistence cannot drift into two subtly different implementations.
#[allow(
    clippy::too_many_lines,
    reason = "settings application keeps the runtime and persisted snapshot atomic"
)]
pub(crate) fn apply_settings_snapshot(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
    settings: &AppSettings,
) {
    let window = &controller.window;

    // Publish the complete typed shortcut table first. This keeps the running
    // event path and the persisted settings atomic: a malformed or conflicting
    // map cannot leave the UI displaying one binding set while the state owns
    // another.
    let bindings = match shortcut_bindings(settings) {
        Ok(bindings) => bindings,
        Err(error) => {
            ui.set_status_message(format!("Hotkeys could not be applied: {error}").into());
            window.set_category(SETTINGS_CATEGORY_HOTKEYS);
            return;
        }
    };
    if let Err(error) = state.borrow_mut().replace_shortcuts(&bindings) {
        ui.set_status_message(format!("Hotkeys could not be applied: {error}").into());
        window.set_category(SETTINGS_CATEGORY_HOTKEYS);
        return;
    }

    let mut notes = Vec::new();
    let output_active = {
        let state = state.borrow();
        state.recording() || state.streaming()
    };

    // Audio: rebuild the mixer only when the format actually differs.
    let audio_format = AudioFormat::new(settings.sample_rate_hz(), settings.channel_count());
    if let Err(error) = state.borrow_mut().dispatch(UiCommand::SetAudioFormat {
        sample_rate: settings.sample_rate_hz(),
        channels: settings.channel_count(),
    }) {
        notes.push(format!("audio: {error}"));
    }
    if let Ok(audio_format) = audio_format {
        if output.borrow().audio_format() != audio_format {
            if output_active {
                output.borrow_mut().stage_audio_format(audio_format);
                notes.push(format!(
                    "audio: {} Hz / {} channels is staged and applies when the output stops",
                    audio_format.sample_rate(),
                    audio_format.channels()
                ));
            } else if let Err(error) = output.borrow_mut().set_audio_format(audio_format) {
                notes.push(format!("audio runtime: {error}"));
            }
        }
    }

    if let Err(error) = output.borrow_mut().set_audio_input_id(
        (!settings.audio_input_id.is_empty()).then_some(settings.audio_input_id.as_str()),
    ) {
        notes.push(format!("audio input: {error}"));
    }
    if let Err(error) = output.borrow_mut().set_channel_sync_offset_millis(
        crate::MIC_CHANNEL_ID,
        settings.audio_input_sync_offset_millis,
    ) {
        notes.push(format!("microphone sync offset: {error}"));
    }
    if let Err(error) = output.borrow_mut().set_channel_sync_offset_millis(
        crate::DESKTOP_CHANNEL_ID,
        settings.desktop_audio_sync_offset_millis,
    ) {
        notes.push(format!("desktop audio sync offset: {error}"));
    }
    if let Err(error) = output
        .borrow_mut()
        .set_channel_monitor_mode(crate::MIC_CHANNEL_ID, settings.microphone_monitor_mode)
    {
        notes.push(format!("microphone monitor mode: {error}"));
    }
    if let Err(error) = output.borrow_mut().set_channel_monitor_mode(
        crate::DESKTOP_CHANNEL_ID,
        settings.desktop_audio_monitor_mode,
    ) {
        notes.push(format!("desktop audio monitor mode: {error}"));
    }
    if let Err(error) = output.borrow_mut().set_monitor_output_id(
        (!settings.audio_monitor_output_id.is_empty())
            .then_some(settings.audio_monitor_output_id.as_str()),
    ) {
        notes.push(format!("monitor output: {error}"));
    }

    output.borrow_mut().configure_stream(settings);
    output.borrow_mut().configure_replay(settings);
    output.borrow_mut().configure_recording(settings);
    // A selection the graph cannot resolve is kept rather than reset, so the
    // user has to be told the engine is on the fallback in the meantime;
    // silently recording from a different source would be worse.
    if !output.borrow_mut().audio_input_available() {
        notes.push(format!(
            "audio input {} is not connected; the fallback is capturing until it returns",
            settings.audio_input_id
        ));
    }
    if !output.borrow_mut().audio_monitor_output_available() {
        notes.push(format!(
            "monitor output {} is not connected; monitoring will retry when it returns",
            settings.audio_monitor_output_id
        ));
    }
    // The mixer row names the device it is capturing, so the fader and meter
    // are visibly tied to the input the user just chose.
    let input_name = output.borrow_mut().audio_input_name();
    if let Err(error) = state
        .borrow_mut()
        .set_channel_name(crate::MIC_CHANNEL_ID, &input_name)
    {
        notes.push(format!("mixer label: {error}"));
    }

    // Video: write the canvas back to the active profile so it persists with
    // the project and the surface rebuilds on the next sync, and configure the
    // encoders for the scaled output geometry beside it. Both are canvas-class
    // changes, so both are staged together while an output is running.
    if let Some(format) = video_format_from(settings.video) {
        if output_active {
            // Changing the canvas mid-output would rebuild the encoders under a
            // container whose frames are already committed at the old geometry,
            // so the change is held rather than applied or thrown away.
            output.borrow_mut().stage_video_format(format);
            notes.push(format!(
                "video: {}x{} is staged and applies when the output stops",
                format.width(),
                format.height()
            ));
        } else if let Err(error) = apply_video_format(state, output, format) {
            notes.push(format!("video: {error}"));
        }
    }
    if output_active {
        output.borrow_mut().stage_output_scaling(
            settings.video.output_width,
            settings.video.output_height,
            settings.video.scale_filter,
        );
        notes.push(format!(
            "output scaling: {}x{} is staged and applies when the output stops",
            settings.video.output_width, settings.video.output_height
        ));
    } else if let Err(error) = output.borrow_mut().set_output_scaling(
        settings.video.output_width,
        settings.video.output_height,
        settings.video.scale_filter,
    ) {
        notes.push(format!("output scaling: {error}"));
    }

    if let Err(error) = settings.save(&controller.path) {
        notes.push(format!("settings file: {error}"));
    }

    *controller.settings.borrow_mut() = settings.clone();
    controller
        .canvas
        .set_snap_distance(settings.canvas_snap_distance);
    apply_to_studio(ui, settings);
    push_palette(ui, controller, settings);
    // The previewed geometry becomes the committed geometry, so a later Cancel
    // restores what was applied rather than what the window opened with.
    controller.push_metrics(settings.metrics());
    window.set_preview_swatch(settings.tokens().preview_border);
    window.set_program_swatch(settings.tokens().program_border);
    window.set_dirty(false);
    refresh_ui(ui, state, surface);
    ui.set_canvas_width(i32::try_from(surface.borrow().format.width()).unwrap_or(1920));
    ui.set_canvas_height(i32::try_from(surface.borrow().format.height()).unwrap_or(1080));

    if !notes.is_empty() {
        ui.set_status_message(
            format!("Settings applied with warnings: {}", notes.join("; ")).into(),
        );
    }
}
