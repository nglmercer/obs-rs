#[allow(
    clippy::wildcard_imports,
    reason = "settings helper callbacks share the controller boundary imports"
)]
use super::*;

/// Returns the canvas format the Video page asks the renderer for.
pub(super) fn video_format_from(video: VideoSettings) -> Option<VideoFormat> {
    let rate = FrameRate::new(video.fps_numerator, video.fps_denominator).ok()?;
    VideoFormat::new(video.base_width, video.base_height, rate).ok()
}

/// Pushes the values the studio window reads directly.
pub(super) fn apply_to_studio(ui: &MainWindow, settings: &AppSettings) {
    ui.set_hotkey_undo(settings.hotkey_undo.as_str().into());
    ui.set_hotkey_redo(settings.hotkey_redo.as_str().into());
    ui.set_hotkey_previous_scene(settings.hotkey_previous_scene.as_str().into());
    ui.set_hotkey_next_scene(settings.hotkey_next_scene.as_str().into());
    ui.set_hotkey_save_project(settings.hotkey_save_project.as_str().into());
    ui.set_hotkey_cut_transition(settings.hotkey_cut_transition.as_str().into());
    ui.set_hotkey_fade_transition(settings.hotkey_fade_transition.as_str().into());
    ui.set_hotkey_save_replay(settings.hotkey_save_replay.as_str().into());
    ui.set_confirm_start_stream(settings.confirm_start_stream);
    ui.set_confirm_stop_stream(settings.confirm_stop_stream);
    ui.set_confirm_stop_recording(settings.confirm_stop_recording);
    ui.set_auto_record_when_streaming(settings.auto_record_when_streaming);
    ui.set_show_safe_areas(settings.show_safe_areas);
    ui.set_project_path(settings.project_path.as_str().into());
    ui.set_diagnostics_path(settings.diagnostics_path.as_str().into());
    ui.set_recording_path(settings.recording_path.as_str().into());
    ui.set_streaming_address(stream_display_label(settings).into());
}

pub(super) fn unsigned(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

pub(super) fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

pub(super) fn stream_display_label(settings: &AppSettings) -> String {
    match settings.stream_protocol {
        StreamProtocol::Rtmp => format!("RTMP · {}", settings.rtmp.server),
        StreamProtocol::Rtmps => format!("RTMPS · {}", settings.rtmp.server),
        StreamProtocol::Srt => format!("SRT · {}:{}", settings.srt.host, settings.srt.port),
        StreamProtocol::Whip => format!("WHIP · {}", settings.whip_endpoint),
        StreamProtocol::Hls => format!("HLS · {}", settings.hls.directory.display()),
        StreamProtocol::Rist => format!("RIST · {}:{}", settings.rist.host, settings.rist.port),
        StreamProtocol::Reference => format!("Reference · {}", settings.reference_address),
    }
}

/// Globals are per component tree, so every window is painted explicitly.
pub(super) fn push_palette(
    ui: &MainWindow,
    controller: &SettingsController,
    settings: &AppSettings,
) {
    push_palette_tokens(ui, controller, &settings.tokens());
}

pub(super) fn push_palette_tokens(
    ui: &MainWindow,
    controller: &SettingsController,
    tokens: &crate::ThemeTokens,
) {
    ui.global::<Palette>().set_tokens(tokens.clone());
    controller
        .window
        .global::<Palette>()
        .set_tokens(tokens.clone());
    controller.add_source.set_tokens(tokens.clone());
    controller.properties.set_tokens(tokens.clone());
    controller.filters.set_tokens(tokens.clone());
    controller.transform.set_tokens(tokens.clone());
    controller.monitor.set_tokens(tokens.clone());
    controller.docks.set_tokens(tokens);
    controller.projectors.set_tokens(tokens);
}

pub(super) fn string_model(values: impl Iterator<Item = SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(values.collect::<Vec<_>>()))
}

pub(super) fn language_label(locale: UiLocale) -> SharedString {
    match locale {
        UiLocale::English => "English".into(),
        UiLocale::Spanish => "Español".into(),
    }
}

pub(super) fn locale_index(locale: UiLocale) -> i32 {
    i32::try_from(
        UiLocale::supported()
            .iter()
            .position(|value| *value == locale)
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

pub(super) fn index_of<T: PartialEq>(values: &[T], needle: &T) -> i32 {
    i32::try_from(values.iter().position(|value| value == needle).unwrap_or(0)).unwrap_or(0)
}

pub(super) fn frame_rate_label((numerator, denominator): (u32, u32)) -> String {
    if denominator == 1 {
        format!("{numerator}")
    } else {
        // 60000/1001 reads as 59.94 the way OBS presents NTSC rates.
        let value = f64::from(numerator) / f64::from(denominator);
        format!("{value:.2}")
    }
}
