//! Repeatable headless sustained-render fixture for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{error::Error, time::Instant};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::{CompositorMetrics, Runtime};
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_video::{
    run_multi_worker_soak, CancellationToken, DropPolicy, MonotonicClock, VideoWorker,
    VideoWorkerReport,
};

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
    let multi_worker = run_multi_worker_soak(format, 2, 30, 3, DropPolicy::DropOldest)?;

    print_report(
        report,
        runtime.compositor_metrics(),
        elapsed.as_millis(),
        multi_worker,
    );
    Ok(())
}

fn print_report(
    report: VideoWorkerReport,
    compositor: CompositorMetrics,
    elapsed_millis: u128,
    multi_worker: obs_rs_video::MultiWorkerSoakReport,
) {
    println!(
        "obs-rs benchmark: requested={} processed={} cancelled={} empty={} dropped_oldest={} dropped_newest={} missed={} lateness_ns={} max_lateness_ns={} wait_ns={} render_ns={} produced_bytes={} peak_queued_bytes={} remaining={} renders={} source_requests={} source_frames={} empty_sources={} transformed={} filtered={} blends={} elapsed_ms={elapsed_millis} multi_workers={} multi_requested={} multi_processed={} multi_missed={} multi_lateness_ns={} multi_produced_bytes={} multi_peak_queued_bytes={} multi_elapsed_ns={}",
        report.requested_frames(),
        report.processed_frames(),
        report.cancelled(),
        report.empty_frames(),
        report.dropped_oldest(),
        report.dropped_newest(),
        report.missed_deadlines(),
        report.total_lateness_nanos(),
        report.max_lateness_nanos(),
        report.total_wait_nanos(),
        report.total_render_nanos(),
        report.produced_bytes(),
        report.peak_queued_bytes(),
        report.remaining_queue(),
        compositor.render_calls(),
        compositor.source_requests(),
        compositor.source_frames(),
        compositor.empty_sources(),
        compositor.transformed_frames(),
        compositor.filtered_frames(),
        compositor.blended_layers(),
        multi_worker.workers(),
        multi_worker.requested_frames(),
        multi_worker.processed_frames(),
        multi_worker.missed_deadlines(),
        multi_worker.total_lateness_nanos(),
        multi_worker.produced_bytes(),
        multi_worker.peak_queued_bytes(),
        multi_worker.elapsed_nanos(),
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
