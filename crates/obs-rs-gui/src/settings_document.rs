#[allow(
    clippy::wildcard_imports,
    reason = "settings submodules share the validated settings namespace"
)]
use super::*;

/// Reads a Slint model into a plain vector.
pub(super) fn read_model<T: Clone + 'static>(model: &ModelRc<T>) -> Vec<T> {
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

pub(super) fn flag(config: &Config, key: &str, fallback: bool) -> bool {
    config
        .get(key)
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(fallback)
}

pub(super) fn text(config: &Config, key: &str, fallback: &str) -> String {
    config
        .get(key)
        .map_or_else(|| fallback.to_owned(), str::to_owned)
}

/// Reads a shortcut through the shared structured parser and stores its
/// canonical display form. Empty input intentionally remains an unbinding;
/// malformed input falls back to the last known-good setting.
pub(crate) fn validated_hotkey(value: &str, fallback: &str) -> String {
    match Shortcut::parse(value) {
        Ok(Some(shortcut)) => shortcut.to_string(),
        Ok(None) => String::new(),
        Err(_) => fallback.to_owned(),
    }
}

/// Compiles the settings document into the bounded, toolkit-neutral shortcut
/// table used by the running desktop state.
///
/// Empty values are intentional unbindings. Settings loading and draft
/// validation already canonicalize each value, but parsing again here keeps
/// the runtime boundary defensive if a caller constructs an `AppSettings`
/// value directly.
pub(crate) fn shortcut_bindings(
    settings: &AppSettings,
) -> Result<Vec<(Shortcut, UiAction)>, UiError> {
    let values = [
        (settings.hotkey_swap.as_str(), UiAction::SwapPreviewProgram),
        (
            settings.hotkey_previous_scene.as_str(),
            UiAction::PreviousPreviewScene,
        ),
        (
            settings.hotkey_next_scene.as_str(),
            UiAction::NextPreviewScene,
        ),
        (
            settings.hotkey_start_recording.as_str(),
            UiAction::StartRecording,
        ),
        (
            settings.hotkey_stop_recording.as_str(),
            UiAction::StopRecording,
        ),
        (
            settings.hotkey_start_streaming.as_str(),
            UiAction::StartStreaming,
        ),
        (
            settings.hotkey_stop_streaming.as_str(),
            UiAction::StopStreaming,
        ),
        (settings.hotkey_undo.as_str(), UiAction::Undo),
        (settings.hotkey_redo.as_str(), UiAction::Redo),
        (settings.hotkey_save_project.as_str(), UiAction::SaveProject),
        (
            settings.hotkey_cut_transition.as_str(),
            UiAction::CutTransition,
        ),
        (
            settings.hotkey_fade_transition.as_str(),
            UiAction::FadeTransition,
        ),
        (
            settings.hotkey_save_replay.as_str(),
            UiAction::SaveReplayBuffer,
        ),
        (
            settings.hotkey_start_replay.as_str(),
            UiAction::StartReplayBuffer,
        ),
        (
            settings.hotkey_stop_replay.as_str(),
            UiAction::StopReplayBuffer,
        ),
        (
            settings.hotkey_toggle_microphone_mute.as_str(),
            UiAction::ToggleMicrophoneMute,
        ),
        (
            settings.hotkey_toggle_desktop_mute.as_str(),
            UiAction::ToggleDesktopMute,
        ),
        (
            settings.hotkey_toggle_studio_mode.as_str(),
            UiAction::ToggleStudioMode,
        ),
        (
            settings.hotkey_toggle_selected_source_visibility.as_str(),
            UiAction::ToggleSelectedSourceVisibility,
        ),
        (
            settings.hotkey_toggle_selected_source_lock.as_str(),
            UiAction::ToggleSelectedSourceLock,
        ),
        (
            settings.hotkey_toggle_selected_source_projector.as_str(),
            UiAction::ToggleSelectedSourceProjector,
        ),
    ];
    let mut bindings = Vec::with_capacity(values.len());
    for (text, action) in values {
        if let Some(shortcut) = Shortcut::parse(text)? {
            bindings.push((shortcut, action));
        }
    }
    Ok(bindings)
}

/// Returns canonical hotkeys that are assigned to more than one local action.
///
/// Empty values are intentional unbindings and are ignored. Parsing here also
/// makes conflict detection agree with runtime matching when users type aliases
/// such as `Option+F9` and `Alt+F9` into different fields.
#[must_use]
pub(crate) fn hotkey_conflicts(settings: &AppSettings) -> Vec<String> {
    let values = [
        settings.hotkey_swap.as_str(),
        settings.hotkey_previous_scene.as_str(),
        settings.hotkey_next_scene.as_str(),
        settings.hotkey_start_recording.as_str(),
        settings.hotkey_stop_recording.as_str(),
        settings.hotkey_start_streaming.as_str(),
        settings.hotkey_stop_streaming.as_str(),
        settings.hotkey_undo.as_str(),
        settings.hotkey_redo.as_str(),
        settings.hotkey_save_project.as_str(),
        settings.hotkey_cut_transition.as_str(),
        settings.hotkey_fade_transition.as_str(),
        settings.hotkey_save_replay.as_str(),
        settings.hotkey_start_replay.as_str(),
        settings.hotkey_stop_replay.as_str(),
        settings.hotkey_toggle_microphone_mute.as_str(),
        settings.hotkey_toggle_desktop_mute.as_str(),
        settings.hotkey_toggle_studio_mode.as_str(),
        settings.hotkey_toggle_selected_source_visibility.as_str(),
        settings.hotkey_toggle_selected_source_lock.as_str(),
        settings.hotkey_toggle_selected_source_projector.as_str(),
    ];
    let mut counts = BTreeMap::new();
    for value in values {
        if let Ok(Some(shortcut)) = Shortcut::parse(value) {
            *counts.entry(shortcut).or_insert(0usize) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(shortcut, count)| (count > 1).then(|| shortcut.to_string()))
        .collect()
}

pub(super) fn hotkey(config: &Config, key: &str, fallback: &str) -> String {
    validated_hotkey(config.get(key).unwrap_or(fallback), fallback)
}

pub(super) fn serialize_project_scene_selections(
    selections: &[ProjectSceneSelection],
    preferred_key: Option<&str>,
) -> String {
    // Keep a small bounded insertion-ordered set. The profile is part of the
    // identity, so two profiles for one document must survive independently;
    // preserving order also keeps settings round-trips stable for callers that
    // compare the typed snapshot vector.
    let mut unique: Vec<&ProjectSceneSelection> =
        Vec::with_capacity(MAX_PERSISTED_PROJECT_SCENE_SELECTIONS * 2);
    for selection in selections {
        let key = selection.key();
        if key.is_empty()
            || key.len() > MAX_PERSISTED_SELECTION_KEY_BYTES
            || selection.profile().is_empty()
        {
            continue;
        }
        let existing = unique
            .iter()
            .position(|current| current.key() == key && current.profile() == selection.profile());
        if let Some(existing) = existing {
            unique[existing] = selection;
        } else if unique.len() < MAX_PERSISTED_PROJECT_SCENE_SELECTIONS * 2 {
            unique.push(selection);
        }
    }

    let mut encoded = String::from("v1");
    let ordered = unique
        .iter()
        .filter(|selection| preferred_key == Some(selection.key()))
        .copied()
        .chain(
            unique
                .iter()
                .filter(|selection| preferred_key != Some(selection.key()))
                .copied(),
        )
        .take(MAX_PERSISTED_PROJECT_SCENE_SELECTIONS);
    for selection in ordered {
        let record = [
            selection_component(selection.key()),
            selection_component(selection.profile()),
            selection_component(selection.preview().unwrap_or_default()),
            selection_component(selection.program().unwrap_or_default()),
        ]
        .join("|");
        let required = 1_usize.saturating_add(record.len());
        if encoded.len().saturating_add(required) > obs_rs_config::MAX_VALUE_BYTES {
            break;
        }
        encoded.push(';');
        encoded.push_str(&record);
    }
    encoded
}

pub(super) fn parse_project_scene_selections(value: &str) -> Vec<ProjectSceneSelection> {
    let mut records: Vec<ProjectSceneSelection> =
        Vec::with_capacity(MAX_PERSISTED_PROJECT_SCENE_SELECTIONS);
    let mut parts = value.split(';');
    if parts.next() != Some("v1") {
        return Vec::new();
    }
    for record in parts {
        if records.len() == MAX_PERSISTED_PROJECT_SCENE_SELECTIONS || record.is_empty() {
            break;
        }
        let mut fields = record.split('|');
        let (Some(key), Some(profile), Some(preview), Some(program)) = (
            fields.next().and_then(selection_component_decode),
            fields.next().and_then(selection_component_decode),
            fields.next().and_then(selection_component_decode),
            fields.next().and_then(selection_component_decode),
        ) else {
            continue;
        };
        if fields.next().is_some()
            || key.is_empty()
            || key.len() > MAX_PERSISTED_SELECTION_KEY_BYTES
            || profile.is_empty()
        {
            continue;
        }
        let selection = ProjectSceneSelection::new(
            key,
            profile,
            (!preview.is_empty()).then_some(preview),
            (!program.is_empty()).then_some(program),
        );
        if let Some(existing) = records.iter().position(|current| {
            current.key() == selection.key() && current.profile() == selection.profile()
        }) {
            records[existing] = selection;
        } else {
            records.push(selection);
        }
    }
    records
}

pub(super) fn selection_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if (b' '..=b'~').contains(&byte) && !matches!(byte, b'%' | b';' | b'|') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    encoded
}

pub(super) fn selection_component_decode(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let high = bytes.next().and_then(hex_digit)?;
        let low = bytes.next().and_then(hex_digit)?;
        decoded.push((high << 4) | low);
    }
    String::from_utf8(decoded).ok()
}

pub(super) fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn bounded_text(config: &Config, key: &str, fallback: &str, maximum: usize) -> String {
    let mut value = text(config, key, fallback);
    value.truncate(maximum);
    value
}

pub(super) fn optional_text(config: &Config, key: &str) -> Option<String> {
    config
        .get(key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn optional_secret(config: &Config, key: &str) -> Option<SecretString> {
    optional_text(config, key).map(SecretString::new)
}

pub(super) fn rtmp_from_config(config: &Config, defaults: &RtmpConfig) -> RtmpConfig {
    RtmpConfig {
        service: text(config, "rtmp_service", &defaults.service),
        server: text(config, "rtmp_server", &defaults.server),
        stream_key: SecretString::new(text(
            config,
            "rtmp_stream_key",
            defaults.stream_key.expose_secret(),
        )),
        video: VideoEncoderConfig {
            codec: VideoCodec::H264,
            implementation: EncoderImplementation::new(text(
                config,
                "stream_video_encoder",
                defaults.video.implementation.id(),
            )),
            rate_control: config
                .get("stream_rate_control")
                .and_then(RateControl::from_id)
                .unwrap_or(defaults.video.rate_control),
            bitrate_kbps: number(
                config,
                "stream_video_bitrate_kbps",
                defaults.video.bitrate_kbps,
            ),
            max_bitrate_kbps: None,
            keyframe_interval_secs: number(
                config,
                "stream_keyframe_interval_secs",
                defaults.video.keyframe_interval_secs,
            ),
            preset: config
                .get("stream_preset")
                .and_then(EncoderPreset::from_id)
                .unwrap_or(defaults.video.preset),
            profile: optional_text(config, "stream_profile")
                .or_else(|| defaults.video.profile.clone()),
            b_frames: number(config, "stream_b_frames", defaults.video.b_frames),
        },
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            implementation: EncoderImplementation::new(text(
                config,
                "stream_audio_encoder",
                defaults.audio.implementation.id(),
            )),
            bitrate_kbps: number(
                config,
                "stream_audio_bitrate_kbps",
                defaults.audio.bitrate_kbps,
            ),
            sample_rate: number(
                config,
                "stream_audio_sample_rate",
                defaults.audio.sample_rate,
            ),
            channels: number(config, "stream_audio_channels", defaults.audio.channels),
            complexity: config
                .get("stream_audio_complexity")
                .and_then(|value| value.parse().ok()),
        },
        reconnect: flag(config, "stream_reconnect", defaults.reconnect),
        maximum_retries: number(config, "stream_maximum_retries", defaults.maximum_retries),
        network_buffer_ms: number(
            config,
            "stream_network_buffer_ms",
            defaults.network_buffer_ms,
        ),
    }
}

pub(super) fn srt_from_config(config: &Config, defaults: &SrtConfig) -> SrtConfig {
    SrtConfig {
        host: text(config, "srt_host", &defaults.host),
        port: number(config, "srt_port", defaults.port),
        mode: config
            .get("srt_mode")
            .and_then(SrtMode::from_id)
            .unwrap_or(defaults.mode),
        latency_ms: number(config, "srt_latency_ms", defaults.latency_ms),
        passphrase: optional_secret(config, "srt_passphrase"),
        pbkeylen: config
            .get("srt_pbkeylen")
            .and_then(|value| value.parse::<u16>().ok())
            .and_then(SrtKeyLength::from_bytes),
        stream_id: optional_text(config, "srt_stream_id"),
        connect_timeout_ms: number(
            config,
            "srt_connect_timeout_ms",
            defaults.connect_timeout_ms,
        ),
    }
}

pub(super) fn number<T>(config: &Config, key: &str, fallback: T) -> T
where
    T: std::str::FromStr,
{
    config
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// Colour fields fall back rather than persisting text the palette cannot use.
pub(super) fn colour_text(config: &Config, key: &str, fallback: &str) -> String {
    config
        .get(key)
        .filter(|value| parse_colour(value).is_some())
        .map_or_else(|| fallback.to_owned(), str::to_owned)
}
