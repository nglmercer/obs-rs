use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

fn loader_format() -> VideoFormat {
    VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format")
}

fn loader_spec() -> StingerSpec {
    StingerSpec::new("assets/intro.webm", 500, true, false).expect("spec")
}

fn loader_clip(format: VideoFormat) -> StingerClip {
    StingerClip::new(
        vec![VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 255])],
        vec![100_000_000],
        500,
    )
    .expect("clip")
}

fn wait_for_result(worker: &StingerLoadWorker) -> StingerLoadResult {
    for _ in 0..200 {
        if let Some(result) = worker.try_take_result() {
            return result;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("stinger loader did not publish a result");
}

#[test]
fn stinger_loader_runs_off_caller_and_publishes_typed_result() {
    let format = loader_format();
    let expected = loader_clip(format);
    let expected_for_loader = expected.clone();
    let worker = StingerLoadWorker::spawn(
        move |request: &StingerLoadRequest, cancellation: &StingerLoadCancellation| {
            assert!(!cancellation.is_cancelled());
            assert_eq!(request.spec(), &loader_spec());
            assert_eq!(request.target_format(), format);
            Ok(expected_for_loader.clone())
        },
    )
    .expect("loader worker");
    worker
        .try_submit(StingerLoadRequest::new(42, loader_spec(), format))
        .expect("bounded request");

    let result = wait_for_result(&worker);
    assert_eq!(result.request_id(), 42);
    assert_eq!(result.into_result().expect("loaded clip"), expected);
}

#[test]
fn stinger_loader_keeps_request_queue_bounded_and_cancels_cooperatively() {
    let format = loader_format();
    let started = Arc::new(AtomicBool::new(false));
    let thread_started = Arc::clone(&started);
    let worker = StingerLoadWorker::spawn(
        move |_request: &StingerLoadRequest, cancellation: &StingerLoadCancellation| {
            thread_started.store(true, Ordering::Release);
            while !cancellation.is_cancelled() {
                thread::yield_now();
            }
            Err(MediaError::InvalidTransition { progress_milli: 0 })
        },
    )
    .expect("loader worker");
    worker
        .try_submit(StingerLoadRequest::new(1, loader_spec(), format))
        .expect("first request");
    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    worker
        .try_submit(StingerLoadRequest::new(2, loader_spec(), format))
        .expect("one pending request");
    assert_eq!(
        worker.try_submit(StingerLoadRequest::new(3, loader_spec(), format)),
        Err(StingerLoadQueueError::Full)
    );

    worker.cancel();
    assert!(worker.is_cancelled());
    assert_eq!(
        worker.try_submit(StingerLoadRequest::new(4, loader_spec(), format)),
        Err(StingerLoadQueueError::Stopped)
    );
}

#[test]
fn stinger_loader_keeps_media_failures_typed() {
    let format = loader_format();
    let worker = StingerLoadWorker::spawn(
        |_request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
            Err(MediaError::InvalidTransition {
                progress_milli: 1_001,
            })
        },
    )
    .expect("loader worker");
    worker
        .try_submit(StingerLoadRequest::new(7, loader_spec(), format))
        .expect("request");

    let result = wait_for_result(&worker);
    assert_eq!(result.request_id(), 7);
    assert_eq!(
        result.into_result(),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001,
        })
    );
}
