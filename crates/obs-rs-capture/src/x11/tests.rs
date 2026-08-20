use std::path::PathBuf;

use obs_rs_media::{Timestamp, VideoFormat};

use super::super::{CaptureError, VideoCaptureDevice};
use super::{
    connection::display_socket,
    image::{
        decode_pixels, decode_pixels_scaled, packed_row_bytes, padded_row_bytes, scale_channel,
    },
    protocol::{
        parse_setup, setup_request, Authorization, ImageByteOrder, VisualMasks, X11_TRUE_COLOR,
    },
    screen::{resize_letterbox, X11CaptureDevice},
};

fn setup_fixture() -> Vec<u8> {
    let mut bytes = vec![0_u8; 40 + 8 + 40 + 8 + 24];
    bytes[0] = 1;
    bytes[28] = 1;
    bytes[29] = 1;
    bytes[30] = 0;
    bytes[32] = 32;
    bytes[33] = 32;
    bytes[40] = 24;
    bytes[41] = 32;
    bytes[42] = 32;
    let root = 48;
    bytes[root..root + 4].copy_from_slice(&7_u32.to_le_bytes());
    bytes[root + 20..root + 22].copy_from_slice(&640_u16.to_le_bytes());
    bytes[root + 22..root + 24].copy_from_slice(&480_u16.to_le_bytes());
    bytes[root + 32..root + 36].copy_from_slice(&7_u32.to_le_bytes());
    bytes[root + 38] = 24;
    bytes[root + 39] = 1;
    let depth = root + 40;
    bytes[depth] = 24;
    bytes[depth + 2..depth + 4].copy_from_slice(&1_u16.to_le_bytes());
    let visual = depth + 8;
    bytes[visual..visual + 4].copy_from_slice(&7_u32.to_le_bytes());
    bytes[visual + 4] = X11_TRUE_COLOR;
    bytes[visual + 8..visual + 12].copy_from_slice(&0x00ff_0000_u32.to_le_bytes());
    bytes[visual + 12..visual + 16].copy_from_slice(&0x0000_ff00_u32.to_le_bytes());
    bytes[visual + 16..visual + 20].copy_from_slice(&0x0000_00ff_u32.to_le_bytes());
    bytes
}

#[test]
fn setup_parser_extracts_root_and_visual_layout() {
    let server = parse_setup(&setup_fixture()).expect("valid setup");
    assert_eq!(server.root, 7);
    assert_eq!((server.width, server.height), (640, 480));
    assert_eq!(server.depth, 24);
    assert_eq!(server.visual, 7);
    assert_eq!(server.bits_per_pixel, 32);
    assert_eq!(server.scanline_pad, 32);
    assert_eq!(
        server.image_byte_order,
        ImageByteOrder::LeastSignificantFirst
    );
    assert_eq!(server.masks.red, 0x00ff_0000);
}

#[test]
fn pixel_decoder_converts_masked_little_endian_true_color() {
    let pixels = decode_pixels(
        2,
        1,
        8,
        32,
        ImageByteOrder::LeastSignificantFirst,
        VisualMasks {
            red: 0x00ff_0000,
            green: 0x0000_ff00,
            blue: 0x0000_00ff,
        },
        &[0, 0, 255, 0, 0, 255, 0, 0],
    )
    .expect("decode pixels");
    assert_eq!(pixels, vec![255, 0, 0, 255, 0, 255, 0, 255]);
}

#[test]
fn scaled_pixel_decoder_matches_letterboxed_reference() {
    let source = [0_u8, 0, 255, 0, 0, 255, 0, 0];
    let scaled = decode_pixels_scaled(
        2,
        1,
        3,
        3,
        8,
        32,
        ImageByteOrder::LeastSignificantFirst,
        VisualMasks {
            red: 0x00ff_0000,
            green: 0x0000_ff00,
            blue: 0x0000_00ff,
        },
        &source,
    )
    .expect("scaled pixels");
    let decoded = decode_pixels(
        2,
        1,
        8,
        32,
        ImageByteOrder::LeastSignificantFirst,
        VisualMasks {
            red: 0x00ff_0000,
            green: 0x0000_ff00,
            blue: 0x0000_00ff,
        },
        &source,
    )
    .expect("decoded pixels");
    let reference = resize_letterbox(&decoded, 2, 1, 3, 3).expect("reference resize");
    assert_eq!(scaled, reference);
}

#[test]
fn display_parser_accepts_local_display_forms_and_rejects_remote_hosts() {
    assert_eq!(
        display_socket(":3.0").expect("display").0,
        PathBuf::from("/tmp/.X11-unix/X3")
    );
    assert_eq!(
        display_socket("unix:4").expect("display").0,
        PathBuf::from("/tmp/.X11-unix/X4")
    );
    assert!(matches!(
        display_socket("remote.example:0"),
        Err(CaptureError::PlatformUnavailable { .. })
    ));
}

#[test]
fn setup_request_contains_optional_auth_and_aligned_fields() {
    let auth = Authorization {
        name: b"MIT-MAGIC-COOKIE-1".to_vec(),
        data: vec![1, 2, 3, 4],
    };
    let request = setup_request(Some(&auth)).expect("request");
    assert_eq!(&request[..2], b"l\0");
    assert_eq!(u16::from_le_bytes([request[6], request[7]]), 18);
    assert_eq!(u16::from_le_bytes([request[8], request[9]]), 4);
    assert_eq!(request.len() % 4, 0);
}

#[test]
fn malformed_setup_is_rejected_before_visual_access() {
    assert!(matches!(
        parse_setup(&[1, 0, 11, 0, 0, 0, 0, 0]),
        Err(CaptureError::Protocol { .. })
    ));
    assert_eq!(scale_channel(0x00ff_0000, 0x00ff_0000), u8::MAX);
    assert_eq!(packed_row_bytes(640, 32).expect("row bytes"), 2_560);
    assert_eq!(padded_row_bytes(2_561, 32).expect("padded row"), 2_564);
}

#[test]
fn root_capture_resize_preserves_aspect_and_fills_canvas() {
    let source = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let output = resize_letterbox(&source, 2, 1, 3, 3).expect("resize");
    assert_eq!(output.len(), 3 * 3 * 4);
    assert_eq!(&output[0..4], &[0, 0, 0, 255]);
    assert_eq!(&output[12..16], &[255, 0, 0, 255]);
    assert_eq!(&output[16..20], &[255, 0, 0, 255]);
    assert_eq!(&output[20..24], &[0, 255, 0, 255]);
    assert_eq!(&output[32..36], &[0, 0, 0, 255]);
}

#[test]
#[ignore = "requires a live local X11 server"]
fn live_x11_monitor_enumeration_reports_usable_rectangles() {
    let display = std::env::var("DISPLAY").expect("DISPLAY");
    let monitors = super::screen::x11_monitors(&display).expect("enumerate monitors");

    assert!(!monitors.is_empty(), "at least the root window is reported");
    for monitor in &monitors {
        assert!(!monitor.name().is_empty());
        assert!(monitor.width() > 0 && monitor.height() > 0);
        assert!(monitor.device_id().starts_with("x11-monitor-"));
    }
}

#[test]
#[ignore = "requires a live local X11 server"]
fn live_x11_capture_crops_to_the_selected_monitor() {
    let display = std::env::var("DISPLAY").expect("DISPLAY");
    let monitors = super::screen::x11_monitors(&display).expect("enumerate monitors");
    let monitor = monitors.first().expect("one monitor").clone();
    let mut device =
        X11CaptureDevice::connect(&display, "x11-root", "X11 root").expect("connect to local X11");

    device
        .select_monitor(Some(monitor.name()))
        .expect("select the first monitor");

    assert_eq!(device.capture_size(), (monitor.width(), monitor.height()));
    let format =
        VideoFormat::new(2, 2, obs_rs_media::FrameRate::new(30, 1).expect("rate")).expect("format");
    device.start(format).expect("start X11 device");
    let frame = device
        .next_frame(Timestamp::ZERO)
        .expect("capture frame")
        .expect("monitor frame");
    assert_eq!(frame.format(), format);
}

#[test]
#[ignore = "requires a live local X11 server"]
fn live_x11_capture_decodes_a_root_screen_frame() {
    let mut device = X11CaptureDevice::connect_from_environment("x11-root", "X11 root")
        .expect("connect to local X11");
    let (width, height) = device.screen_size();
    assert!(width >= 2 && height >= 2);
    let format =
        VideoFormat::new(2, 2, obs_rs_media::FrameRate::new(30, 1).expect("rate")).expect("format");
    device.start(format).expect("start X11 device");
    let frame = device
        .next_frame(Timestamp::ZERO)
        .expect("capture frame")
        .expect("root frame");
    assert_eq!(frame.format(), format);
    assert_eq!(device.frame_index(), 1);
}

/// Returns the parsed server description the region tests clamp against.
fn server() -> super::protocol::ServerInfo {
    parse_setup(&setup_fixture()).expect("the fixture setup parses")
}

#[test]
fn a_window_inside_the_desktop_is_captured_whole() {
    let server = server();

    let region = X11CaptureDevice::clamp_region(100, 50, 320, 240, &server, || "window".to_owned())
        .expect("a fully visible window needs no clamping");

    assert_eq!(
        (region.x, region.y, region.width, region.height),
        (100, 50, 320, 240)
    );
}

#[test]
fn a_window_hanging_off_the_right_edge_is_clipped_to_what_is_visible() {
    let server = server();

    // The 640x480 root leaves 140 pixels to the right of x=500.
    let region = X11CaptureDevice::clamp_region(500, 0, 320, 240, &server, || "window".to_owned())
        .expect("a partly visible window still captures its visible part");

    assert_eq!(region.x, 500);
    assert_eq!(
        region.width, 140,
        "GetImage rejects a request past the root"
    );
    assert_eq!(region.height, 240);
}

#[test]
fn a_window_hanging_off_the_left_edge_loses_the_hidden_columns() {
    let server = server();

    // The window starts 40 pixels left of the desktop, so 40 columns are gone
    // and capture starts at the root origin.
    let region =
        X11CaptureDevice::clamp_region(-40, -30, 320, 240, &server, || "window".to_owned())
            .expect("a window pushed off the top-left still has a visible part");

    assert_eq!((region.x, region.y), (0, 0));
    assert_eq!(region.width, 280);
    assert_eq!(region.height, 210);
}

#[test]
fn a_window_entirely_off_screen_is_a_typed_error_rather_than_an_empty_frame() {
    let server = server();

    let error = X11CaptureDevice::clamp_region(700, 0, 320, 240, &server, || {
        "X11 window 0x00000007".to_owned()
    })
    .expect_err("a window with no visible pixels cannot be captured");

    assert!(
        error.to_string().contains("X11 window 0x00000007"),
        "the message has to name what could not be captured: {error}"
    );
}

#[test]
fn a_window_larger_than_the_desktop_is_clamped_to_the_desktop() {
    let server = server();

    let region = X11CaptureDevice::clamp_region(0, 0, 4096, 4096, &server, || "window".to_owned())
        .expect("an oversized window captures the desktop it covers");

    assert_eq!((region.width, region.height), (640, 480));
}

#[test]
#[ignore = "requires a live local X11 server"]
fn live_x11_window_enumeration_reports_selectable_windows() {
    let display = std::env::var("DISPLAY").expect("DISPLAY is set");

    let windows = super::window::x11_windows(&display).expect("enumerate X11 windows");

    for window in &windows {
        assert!(window.width() > 0 && window.height() > 0);
        assert!(!window.title().is_empty());
        assert_eq!(
            super::window::parse_window_id(&window.device_id()),
            Some(window.id()),
            "a stored device ID has to resolve back to the same window"
        );
    }
}

#[test]
#[ignore = "requires a live local X11 server with at least one window"]
fn live_x11_window_capture_tracks_the_selected_window() {
    let display = std::env::var("DISPLAY").expect("DISPLAY is set");
    let windows = super::window::x11_windows(&display).expect("enumerate X11 windows");
    let target = windows.first().expect("at least one window is open");

    let mut device =
        X11CaptureDevice::connect(&display, "x11-window", "X11 window").expect("connect to X11");
    device
        .select_window(Some(&target.device_id()))
        .expect("select the window");
    assert_eq!(device.selected_window(), Some(target.id()));

    let format = VideoFormat::new(64, 36, obs_rs_media::FrameRate::new(30, 1).expect("rate"))
        .expect("format");
    device.start(format).expect("start the window device");
    let frame = device
        .next_frame(Timestamp::ZERO)
        .expect("capture the window")
        .expect("window frame");
    assert_eq!(frame.format(), format);
}

#[test]
fn an_unparsable_window_id_is_rejected_before_any_capture_starts() {
    // No X11 server is needed: the ID never reaches one.
    assert!(super::window::parse_window_id("window-please").is_none());
    assert!(super::window::parse_window_id("x11-window-zzzz").is_none());
}
