use super::*;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat};
use obs_rs_plugin_api::{Plugin, SourceError, VideoRequest};

fn settings(color: &str) -> Config {
    let mut config = Config::new();
    config.set("width", "2").expect("valid width");
    config.set("height", "2").expect("valid height");
    config.set("color", color).expect("valid color text");
    config
}

fn text_settings(text: &str, color: &str, font_size: &str) -> Config {
    let mut config = Config::new();
    config.set("width", "32").expect("valid width");
    config.set("height", "16").expect("valid height");
    config.set("color", color).expect("valid color text");
    config.set("text", text).expect("valid text");
    config.set("font_size", font_size).expect("valid font size");
    config
}

#[test]
fn builtins_register_and_render_a_color_source() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == COLOR_SOURCE_KIND)
        .expect("color factory");
    let mut source = factory
        .create("background", &settings("#102030FF"))
        .expect("valid source");
    let format =
        VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate")).expect("valid format");
    let frame = source
        .render(&VideoRequest::new(Timestamp::ZERO, format))
        .expect("render succeeds")
        .expect("color source always has a frame");

    assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
}

#[test]
fn invalid_color_is_rejected_at_creation() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == COLOR_SOURCE_KIND)
        .expect("color factory");

    assert!(matches!(
        factory.create("background", &settings("red")),
        Err(SourceError::InvalidSetting { key, .. }) if key == "color"
    ));
}

#[test]
fn text_source_renders_and_updates_a_bounded_bitmap() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == TEXT_SOURCE_KIND)
        .expect("text factory");
    let format =
        VideoFormat::new(32, 16, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let mut source = factory
        .create("caption", &text_settings("A", "#102030FF", "7"))
        .expect("valid text source");
    let request = VideoRequest::new(Timestamp::ZERO, format);
    let first = source
        .render(&request)
        .expect("render succeeds")
        .expect("text source always has a frame");
    assert_eq!(first.pixel(1, 0), Some([0x10, 0x20, 0x30, 0xff]));
    assert_eq!(first.pixel(0, 0), Some([0, 0, 0, 0]));

    source
        .update(&text_settings("B", "#A0B0C0FF", "7"))
        .expect("text update succeeds");
    let second = source
        .render(&request)
        .expect("render after update succeeds")
        .expect("updated text has a frame");
    assert_eq!(second.pixel(0, 0), Some([0xA0, 0xB0, 0xC0, 0xff]));
    assert_ne!(first.pixels(), second.pixels());
}

#[test]
fn text_source_rejects_control_text_and_font_size() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == TEXT_SOURCE_KIND)
        .expect("text factory");
    assert!(matches!(
        factory.create("caption", &text_settings("\u{1}", "#FFFFFFFF", "24")),
        Err(SourceError::InvalidSetting { key, .. }) if key == "text"
    ));
    assert!(matches!(
        factory.create("caption", &text_settings("text", "#FFFFFFFF", "129")),
        Err(SourceError::InvalidSetting { key, .. }) if key == "font_size"
    ));
}

#[test]
fn builtins_expose_the_capture_source_kind() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");

    assert!(plugin
        .source_factories()
        .iter()
        .any(|factory| factory.kind().as_str() == BUILTIN_TEST_PATTERN_SOURCE_KIND));
    assert!(plugin
        .source_factories()
        .iter()
        .any(|factory| factory.kind().as_str() == TEXT_SOURCE_KIND));
    assert_eq!(BUILTIN_TEST_PATTERN_SOURCE_KIND, "test_pattern");
}

#[test]
fn builtins_expose_simulated_platform_capture_kinds() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    for kind in [
        BUILTIN_SCREEN_SOURCE_KIND,
        BUILTIN_WINDOW_SOURCE_KIND,
        BUILTIN_CAMERA_SOURCE_KIND,
    ] {
        let factory = plugin
            .source_factories()
            .iter()
            .find(|factory| factory.kind().as_str() == kind)
            .expect("capture factory");
        let mut source = factory
            .create("capture", &settings("#000000FF"))
            .expect("capture source");
        let frame = source
            .render(&VideoRequest::new(Timestamp::ZERO, format))
            .expect("render")
            .expect("frame");
        assert_eq!(frame.format(), format);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn builtins_expose_the_direct_linux_x11_source_kind() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    assert!(plugin
        .source_factories()
        .iter()
        .any(|factory| factory.kind().as_str() == BUILTIN_X11_SCREEN_SOURCE_KIND));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a live X11 display and ffmpeg/x11grab"]
fn x11_source_produces_a_frame_even_when_get_image_is_rejected() {
    let display = std::env::var("DISPLAY").expect("live DISPLAY");
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == BUILTIN_X11_SCREEN_SOURCE_KIND)
        .expect("X11 factory");
    let mut config = settings("#000000FF");
    config.set("width", "64").expect("width");
    config.set("height", "36").expect("height");
    config.set("display", &display).expect("display setting");
    let mut source = factory.create("screen", &config).expect("screen source");
    let format = VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("rate")).expect("format");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let frame = loop {
        if let Some(frame) = source
            .render(&VideoRequest::new(Timestamp::ZERO, format))
            .expect("screen render")
        {
            break frame;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "screen did not deliver a frame"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(frame.format(), format);
}

#[test]
fn builtins_expose_a_deterministic_capture_discovery_snapshot() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let devices = plugin
        .discover_capture_devices()
        .expect("discover fallbacks");
    assert_eq!(devices.len(), 4);
    assert_eq!(devices[0].id().as_str(), "test-pattern");
    assert_eq!(devices[3].id().as_str(), "camera-0");
}
