use super::*;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

struct FakeClock {
    now: Timestamp,
    requested_deadlines: Vec<Timestamp>,
}

impl VideoClock for FakeClock {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.requested_deadlines.push(deadline);
        if deadline > self.now {
            self.now = deadline;
        }
    }
}

fn format() -> VideoFormat {
    VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
}

fn frame(timestamp: u64, color: [u8; 4]) -> VideoFrame {
    VideoFrame::solid(format(), Timestamp::from_nanos(timestamp), color)
}

#[test]
fn scheduler_has_exact_rational_timestamps() {
    let rate = FrameRate::new(30_000, 1_001).expect("valid rate");
    let mut scheduler = VideoScheduler::new(rate);

    assert_eq!(
        scheduler.next_deadline().expect("first deadline"),
        FrameDeadline {
            index: 0,
            timestamp: Timestamp::ZERO
        }
    );
    assert_eq!(
        scheduler
            .next_deadline()
            .expect("second deadline")
            .timestamp(),
        Timestamp::from_nanos(33_366_666)
    );
    assert_eq!(
        scheduler
            .next_deadline()
            .expect("third deadline")
            .timestamp(),
        Timestamp::from_nanos(66_733_333)
    );
    scheduler.reset();
    assert_eq!(
        scheduler.next_deadline().expect("reset deadline").index(),
        0
    );
}

#[test]
fn queue_drops_oldest_when_configured() {
    let mut queue = FrameQueue::new(format(), 2, DropPolicy::DropOldest).expect("capacity");
    queue.push(frame(1, [1, 0, 0, 255])).expect("first push");
    queue.push(frame(2, [2, 0, 0, 255])).expect("second push");

    assert_eq!(
        queue.push(frame(3, [3, 0, 0, 255])).expect("third push"),
        PushOutcome::DroppedOldest(Timestamp::from_nanos(1))
    );
    assert_eq!(
        queue.pop().expect("first remaining").timestamp(),
        Timestamp::from_nanos(2)
    );
    assert_eq!(
        queue.pop().expect("second remaining").timestamp(),
        Timestamp::from_nanos(3)
    );
    assert!(queue.is_empty());
}

#[test]
fn queue_can_drop_newest_and_reject_wrong_formats() {
    let mut queue = FrameQueue::new(format(), 1, DropPolicy::DropNewest).expect("capacity");
    queue.push(frame(1, [1, 0, 0, 255])).expect("first push");
    assert_eq!(
        queue.push(frame(2, [2, 0, 0, 255])).expect("second push"),
        PushOutcome::DroppedNewest(Timestamp::from_nanos(2))
    );
    let other_format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format");
    assert!(matches!(
        queue.push(VideoFrame::solid(
            other_format,
            Timestamp::ZERO,
            [0, 0, 0, 255]
        )),
        Err(VideoError::FormatMismatch { .. })
    ));
}

#[test]
fn pipeline_combines_schedule_and_queue() {
    let mut pipeline = VideoPipeline::new(format(), 2, DropPolicy::DropOldest).expect("pipeline");
    assert_eq!(pipeline.next_deadline().expect("deadline").index(), 0);
    pipeline.submit(frame(0, [0, 0, 0, 255])).expect("submit");
    assert_eq!(pipeline.queued(), 1);
    assert_eq!(
        pipeline.take_next().expect("output").timestamp(),
        Timestamp::ZERO
    );
}

#[test]
fn render_loop_tracks_empty_frames_and_queue_drops() {
    let mut pipeline = VideoPipeline::new(format(), 1, DropPolicy::DropOldest).expect("pipeline");
    let first = pipeline
        .render_next(|deadline, format| {
            Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                format,
                deadline.timestamp(),
                [1, 0, 0, 255],
            )))
        })
        .expect("first render");
    let second = pipeline
        .render_next(|deadline, format| {
            Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                format,
                deadline.timestamp(),
                [2, 0, 0, 255],
            )))
        })
        .expect("second render");
    let empty = pipeline
        .render_next(|_, _| Ok::<_, std::convert::Infallible>(None))
        .expect("empty render");

    assert!(matches!(first, RenderOutcome::Enqueued { .. }));
    assert!(matches!(
        second,
        RenderOutcome::DroppedOldest {
            dropped: Timestamp::ZERO,
            ..
        }
    ));
    assert!(matches!(empty, RenderOutcome::Empty { .. }));
    assert_eq!(pipeline.metrics().render_calls(), 3);
    assert_eq!(pipeline.metrics().produced_frames(), 2);
    assert_eq!(pipeline.metrics().produced_bytes(), 16);
    assert_eq!(pipeline.metrics().peak_queued_bytes(), 8);
    assert_eq!(pipeline.metrics().empty_frames(), 1);
    assert_eq!(pipeline.metrics().dropped_oldest(), 1);
    let observation = pipeline.observe_deadline(
        FrameDeadline {
            index: 1,
            timestamp: Timestamp::from_nanos(10),
        },
        Timestamp::from_nanos(25),
    );
    assert!(observation.missed());
    assert_eq!(pipeline.metrics().missed_deadlines(), 1);
    assert_eq!(pipeline.metrics().total_lateness_nanos(), 15);
}

#[test]
fn sustained_run_reports_counter_deltas_and_drains_output() {
    let mut pipeline = VideoPipeline::new(format(), 2, DropPolicy::DropOldest).expect("pipeline");
    let report = pipeline
        .run_sustained(120, |deadline, format| {
            Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                format,
                deadline.timestamp(),
                [7, 8, 9, 255],
            )))
        })
        .expect("sustained run");

    assert_eq!(report.requested_frames(), 120);
    assert_eq!(report.produced_frames(), 120);
    assert_eq!(report.empty_frames(), 0);
    assert_eq!(report.dropped_oldest(), 0);
    assert_eq!(report.dropped_newest(), 0);
    assert_eq!(report.remaining_queue(), 0);
    assert_eq!(pipeline.metrics().render_calls(), 120);
}

#[test]
fn paced_worker_reports_lateness_and_honors_callback_cancellation() {
    let mut clock = FakeClock {
        now: Timestamp::from_millis(5),
        requested_deadlines: Vec::new(),
    };
    let token = CancellationToken::new();
    let mut worker = VideoWorker::new(format(), 2, DropPolicy::DropOldest).expect("worker");
    let report = worker
        .run(&mut clock, 10, &token, |deadline, output_format| {
            if deadline.index() == 2 {
                token.cancel();
            }
            Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                output_format,
                deadline.timestamp(),
                [1, 2, 3, 255],
            )))
        })
        .expect("worker run");

    assert_eq!(report.requested_frames(), 10);
    assert_eq!(report.processed_frames(), 3);
    assert!(report.cancelled());
    assert_eq!(report.missed_deadlines(), 1);
    assert_eq!(report.total_lateness_nanos(), 5_000_000);
    assert_eq!(report.total_wait_nanos(), 61_666_666);
    assert_eq!(report.total_render_nanos(), 0);
    assert_eq!(report.max_lateness_nanos(), 5_000_000);
    assert_eq!(report.empty_frames(), 0);
    assert_eq!(report.remaining_queue(), 0);
    assert_eq!(clock.requested_deadlines.len(), 3);
}

#[test]
fn monotonic_clock_is_non_decreasing() {
    let clock = MonotonicClock::start();
    let first = clock.now();
    let second = clock.now();

    assert!(second >= first);
}

#[test]
fn multi_worker_soak_reports_wall_clock_and_owned_frame_footprint() {
    let report = run_multi_worker_soak(format(), 2, 2, 1, DropPolicy::DropOldest)
        .expect("multi-worker soak");
    assert_eq!(report.workers(), 2);
    assert_eq!(report.requested_frames(), 4);
    assert_eq!(report.processed_frames(), 4);
    assert_eq!(report.produced_bytes(), 32);
    assert_eq!(report.peak_queued_bytes(), 8);
    assert!(report.elapsed_nanos() > 0);
    assert_eq!(
        run_multi_worker_soak(format(), 0, 1, 1, DropPolicy::DropOldest),
        Err(VideoError::ZeroWorkers)
    );
}

#[test]
fn pacer_waits_with_an_injected_clock_and_reports_lateness() {
    let mut clock = FakeClock {
        now: Timestamp::from_millis(5),
        requested_deadlines: Vec::new(),
    };
    let mut pacer = VideoPacer::new(FrameRate::new(30, 1).expect("valid rate"));

    let first = pacer.next(&mut clock).expect("first paced deadline");
    assert_eq!(first.deadline().index(), 0);
    assert_eq!(first.requested_at(), Timestamp::from_millis(5));
    assert_eq!(first.observed_at(), Timestamp::from_millis(5));
    assert!(first.missed());
    assert_eq!(first.lateness_nanos(), 5_000_000);
    assert_eq!(first.waited_nanos(), 0);

    let second = pacer.next(&mut clock).expect("second paced deadline");
    assert_eq!(second.deadline().index(), 1);
    assert_eq!(second.observed_at(), Timestamp::from_nanos(33_333_333));
    assert_eq!(second.waited_nanos(), 28_333_333);
    assert!(!second.missed());
    assert_eq!(
        clock.requested_deadlines,
        vec![Timestamp::ZERO, Timestamp::from_nanos(33_333_333)]
    );
}
