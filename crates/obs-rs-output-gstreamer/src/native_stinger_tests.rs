use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gstreamer as gst;
use gstreamer::prelude::*;
use obs_rs_media::{
    FrameRate, MediaError, StingerLoadCancellation, StingerLoadRequest, StingerResourceFailure,
    StingerResourceLoader, StingerSpec, VideoFormat, MAX_STINGER_FRAMES,
};

use super::GStreamerStingerLoader;

fn target_format() -> VideoFormat {
    VideoFormat::new(32, 24, FrameRate::new(30, 1).expect("frame rate")).expect("format")
}

fn spec(path: &std::path::Path) -> StingerSpec {
    StingerSpec::new(path.to_string_lossy(), 500, true, false).expect("stinger spec")
}

fn fixture_path() -> PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("obs-rs-stinger-loader-{token}.mkv"))
}

#[test]
fn stinger_loader_reports_missing_resource_as_typed_media_error() {
    let path = fixture_path();
    let request = StingerLoadRequest::new(1, spec(&path), target_format());
    let mut loader = GStreamerStingerLoader;
    let error = loader
        .load(&request, &StingerLoadCancellation::default())
        .expect_err("missing resource");
    assert_eq!(
        error,
        MediaError::StingerResource {
            failure: StingerResourceFailure::Unreadable,
        }
    );
}

#[test]
fn stinger_loader_honors_cancellation_before_native_setup() {
    let path = fixture_path();
    let request = StingerLoadRequest::new(2, spec(&path), target_format());
    let cancellation = StingerLoadCancellation::default();
    cancellation.cancel();
    let mut loader = GStreamerStingerLoader;
    let error = loader
        .load(&request, &cancellation)
        .expect_err("cancelled resource");
    assert_eq!(
        error,
        MediaError::StingerResource {
            failure: StingerResourceFailure::Cancelled,
        }
    );
}

#[test]
fn stinger_loader_decodes_a_bounded_matroska_fixture() {
    gst::init().expect("GStreamer runtime");
    if [
        "videotestsrc",
        "videoconvert",
        "openh264enc",
        "h264parse",
        "matroskamux",
        "filesink",
        "filesrc",
        "decodebin",
        "videoscale",
        "videorate",
    ]
    .iter()
    .any(|element| gst::ElementFactory::find(element).is_none())
    {
        return;
    }

    let path = fixture_path();
    let description =
        "videotestsrc num-buffers=3 ! video/x-raw,width=32,height=24,framerate=30/1 ! ".to_owned()
            + "videoconvert ! openh264enc ! h264parse ! matroskamux ! filesink location="
            + path.to_str().expect("fixture path");
    let element = gst::parse::launch_full(&description, None, gst::ParseFlags::FATAL_ERRORS)
        .expect("fixture pipeline");
    let pipeline = element.downcast::<gst::Pipeline>().expect("pipeline");
    pipeline
        .set_state(gst::State::Playing)
        .expect("fixture starts");
    let bus = pipeline.bus().expect("fixture bus");
    let message = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(20),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    if let Some(gst::MessageView::Error(error)) = message.as_ref().map(|message| message.view()) {
        panic!("fixture pipeline failed: {}", error.error());
    }
    assert!(
        matches!(
            message.as_ref().map(|message| message.view()),
            Some(gst::MessageView::Eos(_))
        ),
        "fixture pipeline timed out"
    );
    pipeline.set_state(gst::State::Null).expect("fixture stops");

    let request = StingerLoadRequest::new(3, spec(&path), target_format());
    let mut loader = GStreamerStingerLoader;
    let clip = loader
        .load(&request, &StingerLoadCancellation::default())
        .expect("decode fixture");
    assert_eq!(clip.format(), target_format());
    assert!((3..=MAX_STINGER_FRAMES).contains(&clip.frame_count()));
    assert_eq!(clip.transition_point_milli(), 500);
    assert!(clip.duration_nanos() >= 3 * 30_000_000);
    std::fs::remove_file(path).expect("remove fixture");
}

#[test]
fn stinger_loader_does_not_wait_on_a_cancelled_worker_fixture() {
    let path = fixture_path();
    let request = StingerLoadRequest::new(4, spec(&path), target_format());
    let cancellation = StingerLoadCancellation::default();
    cancellation.cancel();
    let started = std::time::Instant::now();
    let mut loader = GStreamerStingerLoader;
    let _ = loader.load(&request, &cancellation);
    assert!(started.elapsed() < Duration::from_secs(1));
}
