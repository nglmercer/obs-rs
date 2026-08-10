//! Repeatable headless sustained-render fixture for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{error::Error, time::Instant};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::Runtime;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_video::{CancellationToken, DropPolicy, MonotonicClock, VideoWorker, VideoWorkerReport};

const BENCHMARK_FRAMES: u64 = 120;

fn main() -> Result<(), Box<dyn Error>> {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut runtime = Runtime::new();
    let plugin = BuiltinPlugin::new()?;
    runtime.register_plugin(&plugin)?;
    runtime.create_scene("benchmark")?;

    let background = runtime.create_source(
        "color_source",
        "background",
        &color_settings("640", "360", "#102030FF"),
    )?;
    let foreground = runtime.create_source(
        "screen_capture",
        "foreground",
        &video_settings("640", "360"),
    )?;
    runtime.attach_source("benchmark", background)?;
    runtime.attach_source("benchmark", foreground)?;
    runtime.set_source_transform(
        "benchmark",
        foreground,
        FrameTransform::new(1_000, 1_000, 0, 0, true, false, 220)?,
    )?;
    runtime.add_source_filter("benchmark", foreground, FrameFilter::Grayscale)?;

    let mut worker = VideoWorker::new(format, 3, DropPolicy::DropOldest)?;
    let cancellation = CancellationToken::new();
    let mut clock = MonotonicClock::start();
    let started = Instant::now();
    let report = worker.run(
        &mut clock,
        BENCHMARK_FRAMES,
        &cancellation,
        |deadline, output_format| {
            runtime
                .render_scene(
                    "benchmark",
                    &VideoRequest::new(deadline.timestamp(), output_format),
                )
                .map_err(|error| error.to_string())
        },
    )?;
    let elapsed = started.elapsed();

    print_report(report, elapsed.as_millis());
    Ok(())
}

fn print_report(report: VideoWorkerReport, elapsed_millis: u128) {
    println!(
        "obs-rs benchmark: requested={} processed={} cancelled={} empty={} dropped_oldest={} dropped_newest={} missed={} lateness_ns={} remaining={} elapsed_ms={elapsed_millis}",
        report.requested_frames(),
        report.processed_frames(),
        report.cancelled(),
        report.empty_frames(),
        report.dropped_oldest(),
        report.dropped_newest(),
        report.missed_deadlines(),
        report.total_lateness_nanos(),
        report.remaining_queue(),
    );
}

fn color_settings(width: &str, height: &str, color: &str) -> Config {
    let mut settings = Config::new();
    settings
        .set("width", width)
        .expect("benchmark width is valid");
    settings
        .set("height", height)
        .expect("benchmark height is valid");
    settings
        .set("color", color)
        .expect("benchmark color is valid");
    settings
}

fn video_settings(width: &str, height: &str) -> Config {
    let mut settings = Config::new();
    settings
        .set("width", width)
        .expect("benchmark width is valid");
    settings
        .set("height", height)
        .expect("benchmark height is valid");
    settings
}
