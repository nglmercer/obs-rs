#[allow(
    clippy::wildcard_imports,
    reason = "settings page callbacks share the controller boundary imports"
)]
use super::*;

/// Copies committed settings plus live project state into the window's draft.
#[allow(
    clippy::too_many_lines,
    reason = "the settings draft is copied atomically into one Slint window boundary"
)]
pub(super) fn load_draft(
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let settings = controller.settings.borrow();
    let window = &controller.window;
    window.set_dirty(false);
    window.set_language_index(locale_index(state.borrow().locale()));
    window.set_theme_index(i32::try_from(settings.theme).unwrap_or(0));
    window.set_style_index(index_of(&UiStyle::ALL, &settings.style));
    window.set_density_index(index_of(&UiDensity::ALL, &settings.density));
    window.set_font_size(f32::from(settings.font_size));
    window.set_confirm_start_stream(settings.confirm_start_stream);
    window.set_confirm_stop_stream(settings.confirm_stop_stream);
    window.set_confirm_stop_recording(settings.confirm_stop_recording);
    window.set_auto_record_when_streaming(settings.auto_record_when_streaming);
    window.set_snap_distance(i32::from(settings.canvas_snap_distance));
    window.set_show_safe_areas(settings.show_safe_areas);
    populate_stream_models(window, &output.borrow(), controller, &settings);
    let service_index = i32::try_from(
        RTMP_SERVICE_PRESETS
            .iter()
            .position(|preset| preset.matches(&settings.rtmp.service))
            .unwrap_or(0),
    )
    .unwrap_or(0);
    window.set_rtmp_service_index(service_index);
    let service_preset = RTMP_SERVICE_PRESETS
        .get(usize::try_from(service_index).unwrap_or(0))
        .copied()
        .unwrap_or(RTMP_SERVICE_PRESETS[0]);
    populate_stream_server_model(window, service_preset, &settings.rtmp.server);
    window.set_rtmp_server(settings.rtmp.server.as_str().into());
    window.set_rtmp_stream_key(settings.rtmp.stream_key.expose_secret().into());
    window.set_stream_video_bitrate(
        i32::try_from(settings.rtmp.video.bitrate_kbps).unwrap_or(i32::MAX),
    );
    window.set_stream_audio_bitrate(
        i32::try_from(settings.rtmp.audio.bitrate_kbps).unwrap_or(i32::MAX),
    );
    window.set_stream_rate_control(settings.rtmp.video.rate_control.id().to_uppercase().into());
    window.set_stream_keyframe_interval(
        i32::try_from(settings.rtmp.video.keyframe_interval_secs).unwrap_or(i32::MAX),
    );
    window.set_encoder_preset_index(index_of(
        &[
            EncoderPreset::Speed,
            EncoderPreset::Balanced,
            EncoderPreset::Quality,
        ],
        &settings.rtmp.video.preset,
    ));
    window.set_output_mode_index(index_of(&OutputMode::ALL, &settings.output_mode));
    window.set_output_simple_mode(settings.output_mode == OutputMode::Simple);
    window.set_custom_encoder_settings(settings.stream_custom_encoder);
    window.set_stream_encoder_profile(settings.rtmp.video.profile.as_deref().unwrap_or("").into());
    window.set_stream_b_frames(i32::from(settings.rtmp.video.b_frames));
    window.set_stream_reconnect(settings.rtmp.reconnect);
    window.set_stream_maximum_retries(
        i32::try_from(settings.rtmp.maximum_retries).unwrap_or(i32::MAX),
    );
    window.set_stream_network_buffer(
        i32::try_from(settings.rtmp.network_buffer_ms).unwrap_or(i32::MAX),
    );
    load_connection_draft(window, &settings);
    window.set_whip_endpoint(settings.whip_endpoint.as_str().into());
    window.set_reference_address(settings.reference_address.as_str().into());
    load_recording_page_draft(window, controller, &settings);
    window.set_browse_enabled(controller.browse_tool.is_some());
    window.set_browse_hint(
        window
            .global::<I18n>()
            .get_text()
            .settings_ui
            .browse_unavailable_hint,
    );
    window.set_project_path(settings.project_path.as_str().into());
    window.set_diagnostics_path(settings.diagnostics_path.as_str().into());
    window.set_restore_project(settings.restore_project);
    window.set_save_project_on_exit(settings.save_project_on_exit);
    window.set_hotkey_swap(settings.hotkey_swap.as_str().into());
    window.set_hotkey_start_recording(settings.hotkey_start_recording.as_str().into());
    window.set_hotkey_stop_recording(settings.hotkey_stop_recording.as_str().into());
    window.set_hotkey_start_streaming(settings.hotkey_start_streaming.as_str().into());
    window.set_hotkey_stop_streaming(settings.hotkey_stop_streaming.as_str().into());
    window.set_hotkey_undo(settings.hotkey_undo.as_str().into());
    window.set_hotkey_redo(settings.hotkey_redo.as_str().into());
    window.set_hotkey_save_project(settings.hotkey_save_project.as_str().into());
    window.set_hotkey_fade_transition(settings.hotkey_fade_transition.as_str().into());
    window.set_hotkey_save_replay(settings.hotkey_save_replay.as_str().into());
    window.set_hotkey_start_replay(settings.hotkey_start_replay.as_str().into());
    window.set_hotkey_stop_replay(settings.hotkey_stop_replay.as_str().into());
    window
        .set_hotkey_toggle_microphone_mute(settings.hotkey_toggle_microphone_mute.as_str().into());
    window.set_hotkey_toggle_desktop_mute(settings.hotkey_toggle_desktop_mute.as_str().into());
    window.set_hotkeys_conflict(hotkey_conflicts(&settings).join(", ").into());
    window.set_preview_border_color(settings.preview_border_color.as_str().into());
    window.set_program_border_color(settings.program_border_color.as_str().into());
    window.set_preview_swatch(settings.tokens().preview_border);
    window.set_program_swatch(settings.tokens().program_border);

    let audio_format = state.borrow().audio_format();
    window.set_sample_rate_index(index_of(&SAMPLE_RATES, &audio_format.sample_rate()));
    window.set_channel_index(index_of(&CHANNEL_LAYOUTS, &audio_format.layout()));
    window.set_audio_input_sync_offset(
        i32::try_from(settings.audio_input_sync_offset_millis).unwrap_or(0),
    );
    window.set_desktop_audio_sync_offset(
        i32::try_from(settings.desktop_audio_sync_offset_millis).unwrap_or(0),
    );
    populate_audio_devices(window, state, output, controller, &settings);

    load_video_draft(surface, controller, &settings);
}

pub(super) fn load_recording_page_draft(
    window: &SettingsWindow,
    controller: &SettingsController,
    settings: &AppSettings,
) {
    window.set_recording_directory(settings.recording_directory.as_str().into());
    window.set_recording_filename_without_spaces(settings.recording_filename_without_spaces);
    window.set_recording_auto_remux(settings.recording_auto_remux);
    window.set_recording_quality_index(index_of(
        &RecordingQuality::ALL,
        &settings.recording_quality,
    ));
    window.set_recording_format_index(index_of(
        &controller.recording_format_ids.borrow(),
        &settings.recording_format,
    ));
    window.set_replay_buffer_duration(
        i32::try_from(settings.replay_buffer_duration_seconds)
            .unwrap_or(i32::try_from(REPLAY_BUFFER_DURATION_DEFAULT).unwrap_or(20)),
    );
    window.set_replay_buffer_capacity(
        i32::try_from(settings.replay_buffer_capacity_mib)
            .unwrap_or(i32::try_from(REPLAY_BUFFER_CAPACITY_MIB_DEFAULT).unwrap_or(64)),
    );
    window.set_recording_split_enabled(settings.recording_split_enabled);
    window.set_recording_split_duration_minutes(
        i32::try_from(settings.recording_split_duration_minutes)
            .unwrap_or(i32::try_from(RECORDING_SPLIT_DURATION_MINUTES_DEFAULT).unwrap_or(60)),
    );
    window.set_recording_split_size_mib(
        i32::try_from(settings.recording_split_size_mib)
            .unwrap_or(i32::try_from(RECORDING_SPLIT_SIZE_MIB_DEFAULT).unwrap_or(64)),
    );
    window.set_recording_split_max_segments(
        i32::try_from(settings.recording_split_max_segments)
            .unwrap_or(i32::try_from(RECORDING_SPLIT_SEGMENTS_DEFAULT).unwrap_or(64)),
    );
    window.set_recording_split_supported(recording_split_available(
        settings.effective_recording_format(),
        controller.segmented_recording_supported,
    ));
    window.set_recording_auto_remux_supported(recording_auto_remux_available(
        settings.effective_recording_format(),
        settings.effective_recording_codec(),
        recording_audio_codec(controller, settings.recording_audio_encoder.id()),
        controller.remux_supported,
    ));
}

/// Copies the Video page's values into the draft.
///
/// The canvas the session is actually rendering at wins over the stored one:
/// the project owns the canvas, and a settings document that has drifted from
/// it must not silently resize the renderer on the next Apply.
pub(super) fn load_video_draft(
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SettingsController>,
    settings: &AppSettings,
) {
    let window = &controller.window;
    let video_format = surface.borrow().format;
    let video = VideoSettings {
        base_width: video_format.width(),
        base_height: video_format.height(),
        fps_numerator: video_format.frame_rate().numerator(),
        fps_denominator: video_format.frame_rate().denominator(),
        ..settings.video
    };
    *controller.draft_video.borrow_mut() = video;
    window.set_base_resolution(resolution_text(video.base_width, video.base_height).into());
    window.set_output_resolution(resolution_text(video.output_width, video.output_height).into());
    window.set_base_resolution_valid(true);
    window.set_output_resolution_valid(true);
    window.set_scale_filter_index(index_of(&ScaleFilter::ALL, &video.scale_filter));
    window.set_fps_mode_index(index_of(&FpsMode::ALL, &video.fps_mode));
    window.set_fps_numerator(i32::try_from(video.fps_numerator).unwrap_or(60));
    window.set_fps_denominator(i32::try_from(video.fps_denominator).unwrap_or(1));
    window.set_frame_rate_index(index_of(
        &FRAME_RATES,
        &(video.fps_numerator, video.fps_denominator),
    ));
    show_fps_fields(window, video.fps_mode);
    refresh_video_page(controller);
    refresh_recording_page(controller, settings.recording_quality);
}

/// Copies the protocol-specific connection fields into the draft.
///
/// These belong to the Stream page's destination, not to encoding, so they are
/// loaded together and kept out of the page-wide draft loader.
pub(super) fn load_connection_draft(window: &SettingsWindow, settings: &AppSettings) {
    window.set_srt_mode_index(match settings.srt.mode {
        SrtMode::Caller => 0,
        SrtMode::Listener => 1,
        SrtMode::Rendezvous => 2,
    });
    window.set_srt_host(settings.srt.host.as_str().into());
    window.set_srt_port(i32::from(settings.srt.port));
    window.set_srt_latency(i32::try_from(settings.srt.latency_ms).unwrap_or(i32::MAX));
    window.set_srt_passphrase(
        settings
            .srt
            .passphrase
            .as_ref()
            .map_or("", SecretString::expose_secret)
            .into(),
    );
    window.set_srt_key_length_index(match settings.srt.pbkeylen {
        None => 0,
        Some(SrtKeyLength::Bits128) => 1,
        Some(SrtKeyLength::Bits192) => 2,
        Some(SrtKeyLength::Bits256) => 3,
    });
    window.set_srt_stream_id(settings.srt.stream_id.as_deref().unwrap_or("").into());
    window.set_srt_timeout(i32::try_from(settings.srt.connect_timeout_ms).unwrap_or(i32::MAX));
    load_extended_stream_draft(window, settings);
}

/// Theme and language change the moment they are picked, because judging either
/// from a dropdown label alone is not realistic.
pub(super) fn install_previews(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let theme_controller = Rc::clone(controller);
    controller.window.on_preview_theme(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        preview_appearance(&ui, &theme_controller);
        let _ = index;
    });

    let weak = ui.as_weak();
    let style_controller = Rc::clone(controller);
    controller.window.on_preview_style(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        preview_appearance(&ui, &style_controller);
        let _ = index;
    });

    // Density and font size change geometry rather than colour, so they push
    // the metric set instead of the palette. Both are previewed for the same
    // reason the theme is: a name in a dropdown does not tell you what the
    // window will look like.
    let density_controller = Rc::clone(controller);
    controller.window.on_preview_density(move |index| {
        density_controller.push_metrics(draft_metrics(&density_controller));
        let _ = index;
    });

    let font_controller = Rc::clone(controller);
    controller.window.on_preview_font_size(move |value| {
        font_controller.push_metrics(draft_metrics(&font_controller));
        let _ = value;
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
    let language_controller = Rc::clone(controller);
    controller.window.on_preview_language(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(locale) = UiLocale::supported()
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
        else {
            return;
        };
        if state
            .borrow_mut()
            .dispatch(UiCommand::SetLocale { locale })
            .is_ok()
        {
            refresh_ui(&ui, &state, &surface);
            language_controller.sync_theme(locale);
        }
    });
}

/// Repaints every window with the theme and style the Appearance page shows.
pub(super) fn preview_appearance(ui: &MainWindow, controller: &Rc<SettingsController>) {
    let window = &controller.window;
    let theme = usize::try_from(window.get_theme_index())
        .unwrap_or(0)
        .min(THEMES.len() - 1);
    let style = draft_style(window);
    let tokens = controller.settings.borrow().tokens_for(theme, style);
    push_palette_tokens(ui, controller, &tokens);
}

/// Returns the style the Appearance page currently shows.
pub(super) fn draft_style(window: &SettingsWindow) -> UiStyle {
    UiStyle::ALL
        .get(usize::try_from(window.get_style_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

/// Returns the density the Appearance page currently shows.
pub(super) fn draft_density(window: &SettingsWindow) -> UiDensity {
    UiDensity::ALL
        .get(usize::try_from(window.get_density_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

/// Returns the font size the Appearance page currently shows.
pub(super) fn draft_font_size(window: &SettingsWindow) -> u8 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the slider is bounded to the font size range before the cast"
    )]
    let value = window
        .get_font_size()
        .clamp(
            f32::from(*FONT_SIZE_RANGE.start()),
            f32::from(*FONT_SIZE_RANGE.end()),
        )
        .round() as u8;
    value
}

/// Builds the metric set for the appearance the window currently shows.
pub(super) fn draft_metrics(controller: &Rc<SettingsController>) -> UiMetrics {
    AppSettings::metrics_for(
        draft_density(&controller.window),
        draft_font_size(&controller.window),
    )
}

/// Wires the Video page's editable resolutions and frame-rate modes.
///
/// The typed value is validated on every keystroke and only a usable pair is
/// written to the draft, so a field left half-typed cannot be committed and
/// cannot overwrite the resolution the session is already rendering at.
pub(super) fn install_video_editing(controller: &Rc<SettingsController>) {
    let base_controller = Rc::clone(controller);
    controller.window.on_edit_base_resolution(move |value| {
        let parsed = parse_resolution(&value);
        base_controller
            .window
            .set_base_resolution_valid(parsed.is_some());
        if let Some((width, height)) = parsed {
            let mut video = base_controller.draft_video.borrow_mut();
            video.base_width = width;
            video.base_height = height;
        }
        refresh_video_page(&base_controller);
    });

    let output_controller = Rc::clone(controller);
    controller.window.on_edit_output_resolution(move |value| {
        let parsed = parse_resolution(&value);
        output_controller
            .window
            .set_output_resolution_valid(parsed.is_some());
        if let Some((width, height)) = parsed {
            let mut video = output_controller.draft_video.borrow_mut();
            video.output_width = width;
            video.output_height = height;
        }
        refresh_video_page(&output_controller);
    });

    let mode_controller = Rc::clone(controller);
    controller.window.on_select_fps_mode(move |index| {
        let mode = FpsMode::ALL
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or_default();
        mode_controller.draft_video.borrow_mut().fps_mode = mode;
        show_fps_fields(&mode_controller.window, mode);
        // Switching to an explicit mode starts from the rate that is already
        // selected, so the frame rate never jumps because the presentation of
        // it changed.
        let (numerator, denominator) = {
            let video = mode_controller.draft_video.borrow();
            (video.fps_numerator, video.fps_denominator)
        };
        mode_controller
            .window
            .set_fps_numerator(i32::try_from(numerator).unwrap_or(60));
        mode_controller
            .window
            .set_fps_denominator(i32::try_from(denominator).unwrap_or(1));
    });

    let rate_controller = Rc::clone(controller);
    controller.window.on_select_frame_rate(move |index| {
        let Some((numerator, denominator)) = FRAME_RATES
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
        else {
            return;
        };
        let mut video = rate_controller.draft_video.borrow_mut();
        video.fps_numerator = numerator;
        video.fps_denominator = denominator;
    });
}

/// Shows the frame-rate control the selected FPS mode uses.
pub(super) fn show_fps_fields(window: &SettingsWindow, mode: FpsMode) {
    window.set_fps_common(mode == FpsMode::Common);
    window.set_fps_integer(mode == FpsMode::Integer);
    window.set_fps_fractional(mode == FpsMode::Fractional);
}

/// Recomputes the aspect ratios and the downscale state from the draft.
pub(super) fn refresh_video_page(controller: &Rc<SettingsController>) {
    let video = *controller.draft_video.borrow();
    let window = &controller.window;
    window.set_base_aspect_ratio(aspect_ratio_text(video.base_width, video.base_height).into());
    window
        .set_output_aspect_ratio(aspect_ratio_text(video.output_width, video.output_height).into());
    window.set_downscale_active(!video.is_unscaled());
    // The picker points at the entry the value matches; a custom size leaves
    // it where it was rather than jumping to an unrelated resolution.
    if let Some(index) = suggestion_index(video.base_width, video.base_height) {
        window.set_base_suggestion_index(index);
    }
    if let Some(index) = suggestion_index(video.output_width, video.output_height) {
        window.set_output_suggestion_index(index);
    }
}

/// Returns the offered resolution matching `width` and `height`, if any.
pub(super) fn suggestion_index(width: u32, height: u32) -> Option<i32> {
    RESOLUTIONS
        .iter()
        .position(|entry| *entry == (width, height))
        .and_then(|index| i32::try_from(index).ok())
}

/// Wires the Output page's mode switch, quality preset, and Browse button.
pub(super) fn install_output_page(controller: &Rc<SettingsController>) {
    let mode_controller = Rc::clone(controller);
    controller.window.on_select_output_mode(move |index| {
        let mode = OutputMode::ALL
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or_default();
        mode_controller
            .window
            .set_output_simple_mode(mode == OutputMode::Simple);
    });

    let quality_controller = Rc::clone(controller);
    controller.window.on_select_recording_quality(move |index| {
        let quality = RecordingQuality::ALL
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or_default();
        refresh_recording_page(&quality_controller, quality);
    });

    let format_controller = Rc::clone(controller);
    controller.window.on_select_recording_format(move |_index| {
        refresh_recording_page(
            &format_controller,
            draft_recording_quality(&format_controller.window),
        );
    });

    let refresh_controller = Rc::clone(controller);
    controller.window.on_refresh_recording_page(move || {
        refresh_recording_page(
            &refresh_controller,
            draft_recording_quality(&refresh_controller.window),
        );
    });

    let browse_controller = Rc::clone(controller);
    controller.window.on_browse_recording_directory(move || {
        let Some(tool) = browse_controller.browse_tool else {
            return;
        };
        let start = browse_controller
            .window
            .get_recording_directory()
            .to_string();
        let Some(directory) = choose_directory(tool, &start) else {
            return;
        };
        browse_controller
            .window
            .set_recording_directory(directory.as_str().into());
        browse_controller.window.set_dirty(true);
        refresh_recording_page(
            &browse_controller,
            draft_recording_quality(&browse_controller.window),
        );
    });
}

/// Returns the recording quality the Output page currently shows.
pub(super) fn draft_recording_quality(window: &SettingsWindow) -> RecordingQuality {
    RecordingQuality::ALL
        .get(usize::try_from(window.get_recording_quality_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

/// Recomputes everything the recording group derives from its own controls.
pub(super) fn refresh_recording_page(
    controller: &Rc<SettingsController>,
    quality: RecordingQuality,
) {
    let window = &controller.window;
    window.set_recording_format_locked(quality.is_lossless());
    let format = if quality.is_lossless() {
        RecordingFormat::ReferencePacket
    } else {
        controller
            .recording_format_ids
            .borrow()
            .get(usize::try_from(window.get_recording_format_index()).unwrap_or(0))
            .copied()
            .unwrap_or_default()
    };
    window.set_recording_split_supported(recording_split_available(
        format,
        controller.segmented_recording_supported,
    ));
    window.set_recording_auto_remux_supported(recording_auto_remux_available(
        format,
        draft_recording_codec(controller, quality),
        draft_recording_audio_codec(controller),
        controller.remux_supported,
    ));
    let settings = AppSettings {
        recording_directory: window.get_recording_directory().to_string(),
        recording_filename_without_spaces: window.get_recording_filename_without_spaces(),
        recording_quality: quality,
        recording_format: format,
        recording_auto_remux: window.get_recording_auto_remux(),
        recording_split_enabled: window.get_recording_split_enabled(),
        ..controller.settings.borrow().clone()
    };
    // Only the path is pushed; the sentence around it is composed in the page
    // so it follows the catalog the window is currently showing.
    window.set_recording_file_preview(
        settings
            .recording_file_path(&recording_stamp(std::time::SystemTime::now()))
            .into(),
    );
    // Two software encodes of every frame is the one combination worth warning
    // about, so the warning depends on the encoder actually selected.
    let software_stream = controller
        .video_encoder_hardware
        .borrow()
        .get(usize::try_from(window.get_video_encoder_index()).unwrap_or(0))
        .is_some_and(|hardware| !hardware);
    window.set_software_encoding_warning(
        software_stream && quality != RecordingQuality::SameAsStream,
    );
}

pub(super) fn recording_split_available(format: RecordingFormat, native_supported: bool) -> bool {
    format == RecordingFormat::ReferencePacket || native_supported
}

pub(super) fn recording_auto_remux_available(
    format: RecordingFormat,
    video_codec: VideoCodec,
    audio_codec: AudioCodec,
    native_supported: bool,
) -> bool {
    format == RecordingFormat::Matroska
        && video_codec == VideoCodec::H264
        && audio_codec == AudioCodec::Aac
        && native_supported
}

pub(super) fn recording_audio_codec(
    controller: &SettingsController,
    encoder_id: &str,
) -> AudioCodec {
    controller
        .recording_audio_encoder_ids
        .borrow()
        .iter()
        .position(|id| id == encoder_id)
        .and_then(|index| {
            controller
                .recording_audio_encoder_codecs
                .borrow()
                .get(index)
                .copied()
        })
        .unwrap_or(AudioCodec::Pcm)
}

pub(super) fn draft_recording_codec(
    controller: &SettingsController,
    quality: RecordingQuality,
) -> VideoCodec {
    if quality.is_lossless() {
        return VideoCodec::ReferenceRle;
    }
    controller
        .recording_codec_ids
        .borrow()
        .get(usize::try_from(controller.window.get_recording_codec_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

pub(super) fn draft_recording_audio_codec(controller: &SettingsController) -> AudioCodec {
    controller
        .recording_audio_encoder_codecs
        .borrow()
        .get(usize::try_from(controller.window.get_recording_audio_encoder_index()).unwrap_or(0))
        .copied()
        .unwrap_or(AudioCodec::Pcm)
}
