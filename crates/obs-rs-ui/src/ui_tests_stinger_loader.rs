use super::*;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use obs_rs_media::{
    FrameRate, MediaError, StingerClip, StingerLoadCancellation, StingerLoadQueueError,
    StingerLoadRequest, StingerResourceFailure, StingerSpec, Timestamp, VideoFormat, VideoFrame,
};

fn loader_format() -> VideoFormat {
    VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format")
}

fn loader_spec(path: &str) -> StingerSpec {
    StingerSpec::new(path, 500, true, false).expect("spec")
}

fn loader_clip(format: VideoFormat, color: [u8; 4]) -> StingerClip {
    StingerClip::new(
        vec![VideoFrame::solid(format, Timestamp::ZERO, color)],
        vec![100_000_000],
        500,
    )
    .expect("clip")
}

fn wait_for_poll(session: &mut StingerLoadSession) {
    for _ in 0..200 {
        if session.poll() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("stinger session did not publish a result");
}

#[test]
fn stinger_session_exposes_ready_clip_without_project_state_duplication() {
    let format = loader_format();
    let expected = loader_clip(format, [0, 255, 0, 255]);
    let expected_for_loader = expected.clone();
    let mut session = StingerLoadSession::spawn(
        move |request: &StingerLoadRequest, cancellation: &StingerLoadCancellation| {
            assert_eq!(request.target_format(), format);
            assert!(!cancellation.is_cancelled());
            Ok(expected_for_loader.clone())
        },
        format,
    )
    .expect("stinger session");

    let request_id = session
        .try_request(loader_spec("assets/intro.webm"))
        .expect("request");
    assert_eq!(request_id, 1);
    assert!(matches!(session.state(), StingerLoadState::Loading { .. }));

    wait_for_poll(&mut session);
    let StingerLoadState::Ready {
        request_id: ready_id,
        spec,
        clip,
    } = session.state()
    else {
        panic!("session should be ready")
    };
    assert_eq!(*ready_id, request_id);
    assert_eq!(spec.resource_path(), "assets/intro.webm");
    assert_eq!(clip.as_ref(), &expected);
    assert_eq!(session.ready_clip().as_deref(), Some(&expected));
}

#[test]
fn stinger_session_keeps_typed_loader_failures() {
    let format = loader_format();
    let mut session = StingerLoadSession::spawn(
        |_request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
            Err(MediaError::StingerResource {
                failure: StingerResourceFailure::DecoderUnavailable,
            })
        },
        format,
    )
    .expect("stinger session");
    session
        .try_request(loader_spec("assets/missing.webm"))
        .expect("request");

    wait_for_poll(&mut session);
    assert_eq!(
        session.state(),
        &StingerLoadState::Failed {
            request_id: 1,
            spec: loader_spec("assets/missing.webm"),
            error: MediaError::StingerResource {
                failure: StingerResourceFailure::DecoderUnavailable,
            },
        }
    );
}

#[test]
fn stinger_session_rejects_a_loader_clip_with_the_wrong_format() {
    let format = loader_format();
    let wrong_format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("rate")).expect("wrong format");
    let mut session = StingerLoadSession::spawn(
        move |_request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
            Ok(loader_clip(wrong_format, [255, 0, 0, 255]))
        },
        format,
    )
    .expect("stinger session");
    session
        .try_request(loader_spec("assets/wrong-format.webm"))
        .expect("request");

    wait_for_poll(&mut session);
    assert_eq!(
        session.state(),
        &StingerLoadState::Failed {
            request_id: 1,
            spec: loader_spec("assets/wrong-format.webm"),
            error: MediaError::FormatMismatch {
                expected: format,
                actual: wrong_format,
            },
        }
    );
    assert!(session.ready_clip().is_none());
}

#[test]
fn stinger_session_keeps_current_request_when_the_worker_queue_is_full() {
    let format = loader_format();
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let thread_started = Arc::clone(&started);
    let thread_release = Arc::clone(&release);
    let mut session = StingerLoadSession::spawn(
        move |_request: &StingerLoadRequest, cancellation: &StingerLoadCancellation| {
            thread_started.store(true, Ordering::Release);
            while !thread_release.load(Ordering::Acquire) && !cancellation.is_cancelled() {
                thread::yield_now();
            }
            Ok(loader_clip(format, [0, 255, 0, 255]))
        },
        format,
    )
    .expect("stinger session");

    session
        .try_request(loader_spec("assets/first.webm"))
        .expect("first request");
    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    session
        .try_request(loader_spec("assets/second.webm"))
        .expect("second request");
    assert_eq!(
        session.try_request(loader_spec("assets/third.webm")),
        Err(StingerLoadQueueError::Full)
    );
    assert_eq!(
        session.state(),
        &StingerLoadState::Loading {
            request_id: 2,
            spec: loader_spec("assets/second.webm"),
        }
    );

    release.store(true, Ordering::Release);
    session.cancel();
    assert_eq!(session.state(), &StingerLoadState::Stopped);
    assert!(session.is_cancelled());
}

#[test]
fn stinger_session_format_change_invalidates_an_old_completion() {
    let old_format = loader_format();
    let new_format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("rate")).expect("new format");
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let thread_started = Arc::clone(&started);
    let thread_release = Arc::clone(&release);
    let mut session = StingerLoadSession::spawn(
        move |_request: &StingerLoadRequest, cancellation: &StingerLoadCancellation| {
            thread_started.store(true, Ordering::Release);
            while !thread_release.load(Ordering::Acquire) && !cancellation.is_cancelled() {
                thread::yield_now();
            }
            Ok(loader_clip(old_format, [0, 255, 0, 255]))
        },
        old_format,
    )
    .expect("stinger session");
    session
        .try_request(loader_spec("assets/old-format.webm"))
        .expect("request");
    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));

    session.set_target_format(new_format);
    release.store(true, Ordering::Release);
    for _ in 0..200 {
        if session.poll() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(session.state(), &StingerLoadState::Idle);
    assert!(session.ready_clip().is_none());
}
