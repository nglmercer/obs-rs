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
fn builtins_expose_the_capture_source_kind() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");

    assert!(plugin
        .source_factories()
        .iter()
        .any(|factory| factory.kind().as_str() == BUILTIN_TEST_PATTERN_SOURCE_KIND));
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
