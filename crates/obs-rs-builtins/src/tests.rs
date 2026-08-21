use super::*;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat};
use obs_rs_plugin_api::{Plugin, SourceError, VideoRequest};
use std::time::Instant;

fn image_settings(path: &str, width: &str, height: &str) -> Config {
    let mut config = Config::new();
    config.set("width", width).expect("valid image width");
    config.set("height", height).expect("valid image height");
    config.set("path", path).expect("valid image path");
    config
}

fn slideshow_settings(paths: &str, width: &str, height: &str, slide_time_ms: &str) -> Config {
    let mut config = Config::new();
    config.set("width", width).expect("valid slideshow width");
    config
        .set("height", height)
        .expect("valid slideshow height");
    config.set("paths", paths).expect("valid slideshow paths");
    config
        .set("slide_time_ms", slide_time_ms)
        .expect("valid slideshow interval");
    config.set("loop", "true").expect("valid slideshow loop");
    config
        .set("randomize", "false")
        .expect("valid slideshow randomization");
    config
}

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
fn image_source_decodes_and_keeps_the_last_frame_on_failed_update() {
    let path = std::env::temp_dir().join(format!("obs-rs-image-source-{}.ppm", std::process::id()));
    std::fs::write(&path, b"P6\n2 1\n255\n\xFF\x00\x00\x00\x00\xFF").expect("write image fixture");

    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == IMAGE_SOURCE_KIND)
        .expect("image factory");
    let format =
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let mut source = factory
        .create(
            "still",
            &image_settings(path.to_str().expect("fixture path is UTF-8"), "2", "1"),
        )
        .expect("valid image source");
    let request = VideoRequest::new(Timestamp::ZERO, format);
    let first = source
        .render(&request)
        .expect("render succeeds")
        .expect("image has a frame");
    assert_eq!(first.pixel(0, 0), Some([0xFF, 0, 0, 0xFF]));
    assert_eq!(first.pixel(1, 0), Some([0, 0, 0xFF, 0xFF]));

    let error = source
        .update(&image_settings("/definitely/missing/image.png", "2", "1"))
        .expect_err("missing image is rejected");
    assert!(matches!(error, SourceError::InvalidSetting { key, .. } if key == "path"));
    let retained = source
        .render(&request)
        .expect("render after failed update succeeds")
        .expect("last valid image remains");
    assert_eq!(retained.pixels(), first.pixels());

    source
        .update(&image_settings("", "2", "1"))
        .expect("empty path clears the source without an error");
    assert!(source
        .render(&request)
        .expect("empty image render succeeds")
        .is_none());
    std::fs::remove_file(path).expect("remove image fixture");
}

#[test]
fn image_source_rejects_dimensions_beyond_the_decoder_limit() {
    let path = std::env::temp_dir().join(format!(
        "obs-rs-image-source-large-{}.ppm",
        std::process::id()
    ));
    std::fs::write(&path, b"P6\n20000 1\n255\n\x00\x00\x00").expect("write oversized image header");

    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == IMAGE_SOURCE_KIND)
        .expect("image factory");
    assert!(matches!(
        factory.create(
            "large",
            &image_settings(path.to_str().expect("fixture path is UTF-8"), "2", "1"),
        ),
        Err(SourceError::InvalidSetting { key, .. }) if key == "path"
    ));
    std::fs::remove_file(path).expect("remove oversized image fixture");
}

#[test]
fn image_slideshow_advances_by_timestamp_and_updates_atomically() {
    let red_path = std::env::temp_dir().join(format!(
        "obs-rs-image-slideshow-red-{}.ppm",
        std::process::id()
    ));
    let blue_path = std::env::temp_dir().join(format!(
        "obs-rs-image-slideshow-blue-{}.ppm",
        std::process::id()
    ));
    std::fs::write(&red_path, b"P6\n1 1\n255\n\xFF\x00\x00").expect("write red fixture");
    std::fs::write(&blue_path, b"P6\n1 1\n255\n\x00\x00\xFF").expect("write blue fixture");

    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == IMAGE_SLIDESHOW_SOURCE_KIND)
        .expect("slideshow factory");
    let paths = format!(
        "{}\n{}",
        red_path.to_str().expect("red path is UTF-8"),
        blue_path.to_str().expect("blue path is UTF-8")
    );
    let format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let mut source = factory
        .create("slideshow", &slideshow_settings(&paths, "1", "1", "100"))
        .expect("valid slideshow source");
    let first = source
        .render(&VideoRequest::new(Timestamp::ZERO, format))
        .expect("first render")
        .expect("first slide");
    let second = source
        .render(&VideoRequest::new(Timestamp::from_millis(100), format))
        .expect("second render")
        .expect("second slide");
    let wrapped = source
        .render(&VideoRequest::new(Timestamp::from_millis(200), format))
        .expect("wrapped render")
        .expect("wrapped slide");
    assert_eq!(first.pixel(0, 0), Some([0xFF, 0, 0, 0xFF]));
    assert_eq!(second.pixel(0, 0), Some([0, 0, 0xFF, 0xFF]));
    assert_eq!(wrapped.pixel(0, 0), first.pixel(0, 0));

    let error = source
        .update(&slideshow_settings(
            "/definitely/missing/image.png",
            "1",
            "1",
            "100",
        ))
        .expect_err("missing slideshow image is rejected");
    assert!(
        matches!(error, SourceError::InvalidSetting { key, .. } if key == "path" || key == "paths")
    );
    let retained = source
        .render(&VideoRequest::new(Timestamp::ZERO, format))
        .expect("render after failed update")
        .expect("old slideshow remains");
    assert_eq!(retained.pixel(0, 0), first.pixel(0, 0));

    std::fs::remove_file(red_path).expect("remove red fixture");
    std::fs::remove_file(blue_path).expect("remove blue fixture");
}

#[test]
fn image_slideshow_expands_directories_and_randomizes_bounded_order() {
    let directory = std::env::temp_dir().join(format!(
        "obs-rs-image-slideshow-directory-{}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).expect("create slideshow directory");
    std::fs::write(directory.join("a.ppm"), b"P6\n1 1\n255\n\xFF\x00\x00")
        .expect("write first directory fixture");
    std::fs::write(directory.join("b.ppm"), b"P6\n1 1\n255\n\x00\xFF\x00")
        .expect("write second directory fixture");
    std::fs::write(directory.join("c.ppm"), b"P6\n1 1\n255\n\x00\x00\xFF")
        .expect("write third directory fixture");
    std::fs::write(directory.join("ignored.txt"), b"not an image")
        .expect("write ignored directory fixture");

    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == IMAGE_SLIDESHOW_SOURCE_KIND)
        .expect("slideshow factory");
    let path = directory.to_str().expect("slideshow directory is UTF-8");
    let format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let mut sequential = factory
        .create("sequential", &slideshow_settings(path, "1", "1", "100"))
        .expect("directory slideshow source");
    let sequential_order = (0_u64..3)
        .map(|index| {
            sequential
                .render(&VideoRequest::new(
                    Timestamp::from_millis(index * 100),
                    format,
                ))
                .expect("sequential render")
                .expect("sequential frame")
                .pixel(0, 0)
                .expect("sequential pixel")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sequential_order,
        vec![[0xFF, 0, 0, 0xFF], [0, 0xFF, 0, 0xFF], [0, 0, 0xFF, 0xFF]]
    );

    let mut random_settings = slideshow_settings(path, "1", "1", "100");
    random_settings
        .set("randomize", "true")
        .expect("enable slideshow randomization");
    let mut randomized = factory
        .create("randomized", &random_settings)
        .expect("randomized directory slideshow source");
    let randomized_order = (0_u64..3)
        .map(|index| {
            randomized
                .render(&VideoRequest::new(
                    Timestamp::from_millis(index * 100),
                    format,
                ))
                .expect("randomized render")
                .expect("randomized frame")
                .pixel(0, 0)
                .expect("randomized pixel")
        })
        .collect::<Vec<_>>();
    let mut expected_permutation = sequential_order.clone();
    let mut observed_permutation = randomized_order.clone();
    expected_permutation.sort_unstable();
    observed_permutation.sort_unstable();
    assert_eq!(observed_permutation, expected_permutation);
    assert_ne!(randomized_order, sequential_order);

    std::fs::remove_dir_all(directory).expect("remove slideshow directory fixture");
}

#[test]
fn image_slideshow_rejects_unbounded_interval_and_file_count() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == IMAGE_SLIDESHOW_SOURCE_KIND)
        .expect("slideshow factory");
    assert!(matches!(
        factory.create(
            "slideshow",
            &slideshow_settings("", "2", "2", "49")
        ),
        Err(SourceError::InvalidSetting { key, .. }) if key == "slide_time_ms"
    ));

    let path = std::env::temp_dir().join(format!(
        "obs-rs-image-slideshow-count-{}.ppm",
        std::process::id()
    ));
    std::fs::write(&path, b"P6\n1 1\n255\n\x00\xFF\x00").expect("write count fixture");
    let path = path.to_str().expect("count path is UTF-8");
    let paths = (0..65).map(|_| path).collect::<Vec<_>>().join("\n");
    assert!(matches!(
        factory.create("slideshow", &slideshow_settings(&paths, "2", "2", "100")),
        Err(SourceError::InvalidSetting { key, .. }) if key == "paths"
    ));
    std::fs::remove_file(path).expect("remove count fixture");
}

#[test]
fn image_slideshow_render_timing_report() {
    let path = std::env::temp_dir().join(format!(
        "obs-rs-image-slideshow-timing-{}.ppm",
        std::process::id()
    ));
    std::fs::write(
        &path,
        b"P6\n2 2\n255\n\x80\x40\x20\x80\x40\x20\x80\x40\x20\x80\x40\x20",
    )
    .expect("write timing fixture");
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == IMAGE_SLIDESHOW_SOURCE_KIND)
        .expect("slideshow factory");
    let format =
        VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let mut source = factory
        .create(
            "slideshow",
            &slideshow_settings(
                path.to_str().expect("timing path is UTF-8"),
                "640",
                "360",
                "100",
            ),
        )
        .expect("valid slideshow source");
    let started = Instant::now();
    let mut checksum = 0_u64;
    for index in 0..100 {
        let frame = source
            .render(&VideoRequest::new(Timestamp::from_millis(index), format))
            .expect("slideshow render")
            .expect("slideshow frame");
        checksum = checksum.saturating_add(u64::from(frame.pixels()[0]));
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum > 0);
    std::hint::black_box(checksum);
    println!(
        "image slideshow: 100 x 640x360 renders = {:?} ({:?}/render)",
        elapsed,
        elapsed / 100
    );
    std::fs::remove_file(path).expect("remove timing fixture");
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
    assert!(plugin
        .source_factories()
        .iter()
        .any(|factory| factory.kind().as_str() == IMAGE_SOURCE_KIND));
    assert!(plugin
        .source_factories()
        .iter()
        .any(|factory| factory.kind().as_str() == IMAGE_SLIDESHOW_SOURCE_KIND));
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
