//! Repeatable headless sustained-render fixture for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{env, error::Error, fs, time::Instant};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::{CompositorMetrics, Runtime};
use obs_rs_media::{
    frame_memory_metrics, reset_frame_memory_metrics, FrameFilter, FrameMemoryMetrics, FrameRate,
    FrameTransform, Timestamp, VideoFormat,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_video::{
    run_multi_worker_soak, CancellationToken, DropPolicy, MonotonicClock, VideoWorker,
    VideoWorkerReport,
};

const BENCHMARK_FRAMES: u64 = 120;

fn main() -> Result<(), Box<dyn Error>> {
    let json = env::args().skip(1).any(|argument| argument == "--json");
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
    runtime.add_source_filter(foreground, FrameFilter::Grayscale)?;

    let rss_before_kib = resident_memory_kib();
    let render_latency = measure_unpaced_render_latency(&mut runtime, format)?;
    let frame_memory = frame_memory_metrics();

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
    let rss_after_kib = resident_memory_kib();

    let output = BenchmarkOutput {
        report,
        compositor: runtime.compositor_metrics(),
        elapsed_millis: elapsed.as_millis(),
        multi_worker,
        render_latency,
        frame_memory,
        rss_before_kib,
        rss_after_kib,
    };
    if json {
        print_json(&output);
    } else {
        print_report(&output);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RenderLatency {
    samples: usize,
    p50_nanos: u128,
    p95_nanos: u128,
    max_nanos: u128,
}

#[derive(Clone, Copy)]
struct BenchmarkOutput {
    report: VideoWorkerReport,
    compositor: CompositorMetrics,
    elapsed_millis: u128,
    multi_worker: obs_rs_video::MultiWorkerSoakReport,
    render_latency: RenderLatency,
    frame_memory: FrameMemoryMetrics,
    rss_before_kib: Option<u64>,
    rss_after_kib: Option<u64>,
}

fn measure_unpaced_render_latency(
    runtime: &mut Runtime,
    format: VideoFormat,
) -> Result<RenderLatency, Box<dyn Error>> {
    for index in 0..10_u64 {
        let request = VideoRequest::new(Timestamp::from_millis(index), format);
        drop(runtime.render_scene("benchmark", &request)?);
    }

    reset_frame_memory_metrics();
    let mut samples = Vec::with_capacity(usize::try_from(BENCHMARK_FRAMES)?);
    for index in 0..BENCHMARK_FRAMES {
        let request = VideoRequest::new(Timestamp::from_millis(index + 10), format);
        let started = Instant::now();
        drop(runtime.render_scene("benchmark", &request)?);
        samples.push(started.elapsed().as_nanos());
    }
    samples.sort_unstable();
    Ok(RenderLatency {
        samples: samples.len(),
        p50_nanos: percentile(&samples, 50),
        p95_nanos: percentile(&samples, 95),
        max_nanos: samples.last().copied().unwrap_or(0),
    })
}

fn percentile(sorted_samples: &[u128], percentile: usize) -> u128 {
    if sorted_samples.is_empty() {
        return 0;
    }
    let index = (sorted_samples.len() - 1) * percentile / 100;
    sorted_samples[index]
}

fn resident_memory_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?.trim();
        value.split_ascii_whitespace().next()?.parse().ok()
    })
}

fn print_report(output: &BenchmarkOutput) {
    let report = output.report;
    let compositor = output.compositor;
    let multi_worker = output.multi_worker;
    println!(
        "obs-rs benchmark: render_samples={} render_p50_ns={} render_p95_ns={} render_max_ns={} frame_owned_buffers={} frame_owned_bytes={} frame_shared_clones={} frame_cow_buffers={} frame_copied_bytes={} rss_before_kib={} rss_after_kib={} requested={} processed={} cancelled={} empty={} dropped_oldest={} dropped_newest={} missed={} lateness_ns={} max_lateness_ns={} wait_ns={} paced_render_ns={} produced_bytes={} peak_queued_bytes={} remaining={} renders={} source_requests={} source_frames={} empty_sources={} transformed={} filtered={} blends={} elapsed_ms={} multi_workers={} multi_requested={} multi_processed={} multi_missed={} multi_lateness_ns={} multi_produced_bytes={} multi_peak_queued_bytes={} multi_elapsed_ns={}",
        output.render_latency.samples,
        output.render_latency.p50_nanos,
        output.render_latency.p95_nanos,
        output.render_latency.max_nanos,
        output.frame_memory.owned_buffers(),
        output.frame_memory.owned_bytes(),
        output.frame_memory.shared_clones(),
        output.frame_memory.copy_on_write_buffers(),
        output.frame_memory.copy_on_write_bytes(),
        display_optional(output.rss_before_kib),
        display_optional(output.rss_after_kib),
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
        output.elapsed_millis,
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

fn display_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps the dependency-free JSON schema explicit"
)]
fn print_json(output: &BenchmarkOutput) {
    let report = output.report;
    let compositor = output.compositor;
    let multi = output.multi_worker;
    let rss_before = output
        .rss_before_kib
        .map_or_else(|| "null".to_owned(), |v| v.to_string());
    let rss_after = output
        .rss_after_kib
        .map_or_else(|| "null".to_owned(), |v| v.to_string());
    println!(
        concat!(
            "{{\"schema_version\":1,",
            "\"render_latency\":{{\"samples\":{},\"p50_ns\":{},\"p95_ns\":{},\"max_ns\":{}}},",
            "\"frame_memory\":{{\"owned_buffers\":{},\"owned_bytes\":{},\"shared_clones\":{},\"copy_on_write_buffers\":{},\"copied_bytes\":{}}},",
            "\"process_memory\":{{\"rss_before_kib\":{},\"rss_after_kib\":{}}},",
            "\"paced_worker\":{{\"requested\":{},\"processed\":{},\"cancelled\":{},\"empty\":{},\"dropped_oldest\":{},\"dropped_newest\":{},\"missed_deadlines\":{},\"total_lateness_ns\":{},\"max_lateness_ns\":{},\"wait_ns\":{},\"render_ns\":{},\"produced_bytes\":{},\"peak_queued_bytes\":{},\"remaining_queue\":{},\"elapsed_ms\":{}}},",
            "\"compositor\":{{\"renders\":{},\"source_requests\":{},\"source_frames\":{},\"empty_sources\":{},\"transformed\":{},\"filtered\":{},\"blends\":{}}},",
            "\"multi_worker\":{{\"workers\":{},\"requested\":{},\"processed\":{},\"missed_deadlines\":{},\"total_lateness_ns\":{},\"produced_bytes\":{},\"peak_queued_bytes\":{},\"elapsed_ns\":{}}}}}"
        ),
        output.render_latency.samples, output.render_latency.p50_nanos,
        output.render_latency.p95_nanos, output.render_latency.max_nanos,
        output.frame_memory.owned_buffers(), output.frame_memory.owned_bytes(),
        output.frame_memory.shared_clones(), output.frame_memory.copy_on_write_buffers(),
        output.frame_memory.copy_on_write_bytes(), rss_before, rss_after,
        report.requested_frames(), report.processed_frames(), report.cancelled(),
        report.empty_frames(), report.dropped_oldest(), report.dropped_newest(),
        report.missed_deadlines(), report.total_lateness_nanos(), report.max_lateness_nanos(),
        report.total_wait_nanos(), report.total_render_nanos(), report.produced_bytes(),
        report.peak_queued_bytes(), report.remaining_queue(), output.elapsed_millis,
        compositor.render_calls(), compositor.source_requests(), compositor.source_frames(),
        compositor.empty_sources(), compositor.transformed_frames(), compositor.filtered_frames(),
        compositor.blended_layers(), multi.workers(), multi.requested_frames(),
        multi.processed_frames(), multi.missed_deadlines(), multi.total_lateness_nanos(),
        multi.produced_bytes(), multi.peak_queued_bytes(), multi.elapsed_nanos(),
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
