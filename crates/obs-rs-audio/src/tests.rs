use super::*;
use obs_rs_media::Timestamp;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

struct FakeClock {
    now: Timestamp,
    requested_deadlines: Vec<Timestamp>,
}

impl AudioClock for FakeClock {
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

struct LateClock {
    now: Timestamp,
    delay_nanos: u64,
    requested_deadlines: Vec<Timestamp>,
}

impl AudioClock for LateClock {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.requested_deadlines.push(deadline);
        self.now = Timestamp::from_nanos(deadline.as_nanos().saturating_add(self.delay_nanos));
    }
}

fn format() -> AudioFormat {
    AudioFormat::new(48_000, 2).expect("valid audio format")
}

fn buffer(values: &[f32]) -> AudioBuffer {
    AudioBuffer::new(format(), Timestamp::ZERO, values.to_vec()).expect("valid buffer")
}

#[path = "audio_tests_basics.rs"]
mod basics;
#[path = "audio_tests_filters.rs"]
mod filters;
#[path = "audio_tests_mixer.rs"]
mod mixer;
#[path = "audio_tests_monitor.rs"]
mod monitor;
#[path = "audio_tests_pipeline.rs"]
mod pipeline;
