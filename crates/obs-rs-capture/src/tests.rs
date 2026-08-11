use obs_rs_config::Config;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

use super::*;

fn format() -> VideoFormat {
    VideoFormat::new(32, 16, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
}

#[test]
fn catalog_is_deterministic_and_rejects_duplicates() {
    let first = CaptureDeviceInfo::new("screen_b", "B", CaptureKind::Screen).expect("valid info");
    let second = CaptureDeviceInfo::new("screen_a", "A", CaptureKind::Screen).expect("valid info");
    let mut catalog = CaptureCatalog::new();
    catalog.register(first.clone()).expect("first device");
    catalog.register(second).expect("second device");
    assert_eq!(
        catalog.devices().next().expect("first device"),
        &catalog.get("screen_a").cloned().expect("lookup")
    );
    assert_eq!(
        catalog.register(first),
        Err(CaptureError::DuplicateDevice(
            Identifier::new("screen_b").expect("valid id")
        ))
    );
}

#[test]
fn catalog_applies_hotplug_and_permission_events() {
    let info =
        CaptureDeviceInfo::new("camera", "Camera", CaptureKind::Camera).expect("device info");
    let id = info.id().clone();
    let mut catalog = CaptureCatalog::new();
    catalog.apply(CaptureEvent::Added(info)).expect("add event");
    catalog
        .apply(CaptureEvent::PermissionChanged {
            id: id.clone(),
            permission: CapturePermission::PromptRequired,
        })
        .expect("permission event");
    assert_eq!(
        catalog.get("camera").expect("camera").permission(),
        CapturePermission::PromptRequired
    );
    catalog
        .apply(CaptureEvent::Removed(id.clone()))
        .expect("remove event");
    assert!(catalog.get("camera").is_none());
    assert_eq!(
        catalog.apply(CaptureEvent::Removed(id.clone())),
        Err(CaptureError::UnknownDevice(id))
    );
}

#[test]
fn provider_refreshes_catalog_atomically_and_deterministically() {
    let provider = SimulatedCaptureProvider::new();
    let mut catalog = CaptureCatalog::new();
    catalog
        .register(
            CaptureDeviceInfo::new("old", "Old device", CaptureKind::Screen).expect("old device"),
        )
        .expect("register old device");
    provider.refresh(&mut catalog).expect("refresh catalog");

    let devices: Vec<&str> = catalog
        .devices()
        .map(|device| device.id().as_str())
        .collect();
    assert_eq!(
        devices,
        vec!["camera-0", "screen-0", "test-pattern", "window-0"]
    );
    assert!(catalog.get("old").is_none());
}

#[test]
fn catalog_snapshot_rejects_duplicates_without_partial_replacement() {
    let mut catalog = CaptureCatalog::new();
    catalog
        .register(
            CaptureDeviceInfo::new("stable", "Stable", CaptureKind::Screen).expect("stable device"),
        )
        .expect("register stable");
    let duplicate =
        CaptureDeviceInfo::new("duplicate", "One", CaptureKind::Screen).expect("first duplicate");
    let duplicate_again =
        CaptureDeviceInfo::new("duplicate", "Two", CaptureKind::Window).expect("second duplicate");

    assert_eq!(
        catalog.replace_all(vec![duplicate, duplicate_again]),
        Err(CaptureError::DuplicateDevice(
            Identifier::new("duplicate").expect("valid ID")
        ))
    );
    assert!(catalog.get("stable").is_some());
    assert!(catalog.get("duplicate").is_none());
}

#[test]
fn test_device_has_start_stop_and_animated_frames() {
    let mut device = TestPatternDevice::new("pattern", "Pattern").expect("device");
    assert_eq!(
        device.next_frame(Timestamp::ZERO),
        Err(CaptureError::NotRunning)
    );
    device.set_permission(CapturePermission::Denied);
    assert_eq!(device.start(format()), Err(CaptureError::PermissionDenied));
    device.set_permission(CapturePermission::Granted);
    device.start(format()).expect("start device");
    assert!(device.is_running());
    assert_eq!(device.start(format()), Err(CaptureError::AlreadyRunning));
    let first = device
        .next_frame(Timestamp::ZERO)
        .expect("first frame")
        .expect("frame exists");
    let second = device
        .next_frame(Timestamp::from_millis(33))
        .expect("second frame")
        .expect("frame exists");
    assert_ne!(first.pixels(), second.pixels());
    assert_eq!(device.frame_index(), 2);
    device.stop();
    assert!(!device.is_running());
}

#[test]
fn stream_device_round_trips_bounded_rgba_packets() {
    let format = format();
    let first = VideoFrame::solid(format, Timestamp::from_millis(10), [1, 2, 3, 255]);
    let second = VideoFrame::solid(format, Timestamp::from_millis(20), [4, 5, 6, 255]);
    let mut bytes = encode_frame_packet(&first).expect("first packet");
    bytes.extend_from_slice(&encode_frame_packet(&second).expect("second packet"));
    let mut device = StreamCaptureDevice::new(
        "stream",
        "Rust frame stream",
        CaptureKind::Screen,
        std::io::Cursor::new(bytes),
    )
    .expect("device");
    device.start(format).expect("start");
    assert_eq!(
        device.next_frame(Timestamp::ZERO).expect("first read"),
        Some(first)
    );
    assert_eq!(
        device
            .next_frame(Timestamp::from_millis(33))
            .expect("second read"),
        Some(second)
    );
    assert_eq!(device.next_frame(Timestamp::from_millis(66)), Ok(None));
    assert_eq!(device.frame_index(), 2);
}

#[test]
fn stream_device_rejects_truncation_and_format_mismatch() {
    let format = format();
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255]);
    let mut truncated = encode_frame_packet(&frame).expect("packet");
    let _ = truncated.pop();
    let mut device = StreamCaptureDevice::new(
        "stream",
        "Rust frame stream",
        CaptureKind::Screen,
        std::io::Cursor::new(truncated),
    )
    .expect("device");
    device.start(format).expect("start");
    assert_eq!(
        device.next_frame(Timestamp::ZERO),
        Err(CaptureError::TruncatedFrame)
    );

    let other_format =
        VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let packet = encode_frame_packet(&VideoFrame::solid(
        other_format,
        Timestamp::ZERO,
        [0, 0, 0, 255],
    ))
    .expect("packet");
    let mut mismatch = StreamCaptureDevice::new(
        "stream-other",
        "Rust frame stream",
        CaptureKind::Screen,
        std::io::Cursor::new(packet),
    )
    .expect("device");
    mismatch.start(format).expect("start");
    assert!(matches!(
        mismatch.next_frame(Timestamp::ZERO),
        Err(CaptureError::FrameFormatMismatch { expected, actual })
            if expected == format && actual == other_format
    ));
}

#[test]
fn simulated_platform_kinds_share_the_cpu_lifecycle_contract() {
    for kind in [
        CaptureKind::Screen,
        CaptureKind::Window,
        CaptureKind::Camera,
    ] {
        let id = match kind {
            CaptureKind::Screen => "screen",
            CaptureKind::Window => "window",
            CaptureKind::Camera => "camera",
            CaptureKind::TestPattern => "pattern",
            CaptureKind::External => "external",
        };
        let mut device = SimulatedCaptureDevice::new(id, id, kind).expect("device");
        device.start(format()).expect("start");
        let frame = device
            .next_frame(Timestamp::from_millis(33))
            .expect("frame result")
            .expect("frame");
        assert_eq!(frame.format(), format());
        assert_eq!(frame.timestamp(), Timestamp::from_millis(33));
        assert_eq!(device.info().kind(), kind);
        device.stop();
        assert!(!device.is_running());
    }
}

#[test]
fn source_factory_exposes_test_pattern_frames() {
    let factory = TestPatternFactory::new().expect("factory");
    let mut settings = Config::new();
    settings.set("width", "32").expect("width");
    settings.set("height", "16").expect("height");
    let mut source = factory.create("capture", &settings).expect("source");
    let frame = source
        .render(&VideoRequest::new(Timestamp::ZERO, format()))
        .expect("render")
        .expect("frame");
    assert_eq!(frame.format(), format());
}

#[test]
fn simulated_factories_expose_screen_window_and_camera_sources() {
    let settings = {
        let mut settings = Config::new();
        settings.set("width", "32").expect("width");
        settings.set("height", "16").expect("height");
        settings
    };
    for (kind, capture_kind) in [
        (SCREEN_CAPTURE_SOURCE_KIND, CaptureKind::Screen),
        (WINDOW_CAPTURE_SOURCE_KIND, CaptureKind::Window),
        (CAMERA_CAPTURE_SOURCE_KIND, CaptureKind::Camera),
    ] {
        let factory = SimulatedCaptureFactory::new(kind, capture_kind).expect("factory");
        let mut source = factory.create("capture", &settings).expect("source");
        let frame = source
            .render(&VideoRequest::new(Timestamp::ZERO, format()))
            .expect("render")
            .expect("frame");
        assert_eq!(frame.format(), format());
    }
}
