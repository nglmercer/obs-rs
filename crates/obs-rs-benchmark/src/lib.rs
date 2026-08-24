//! Small, repeatable local benchmarks shared by the CLI and first-run setup.
//!
//! The benchmark renders an in-memory scene made from built-in sources. It
//! does not open a stream, write a recording, or contact a service, which
//! makes it safe to run before the user's output configuration is known.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    cmp::Ordering,
    fmt::{Display, Write},
    fs,
    time::Instant,
};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::{CompositorMetrics, Runtime};
use obs_rs_media::{
    frame_memory_metrics, reset_frame_memory_metrics, FrameFilter, FrameMemoryMetrics, FrameRate,
    FrameTransform, Timestamp, VideoFormat,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_video::{
    run_multi_worker_soak, CancellationToken, DropPolicy, FrameDeadline, MonotonicClock,
    VideoWorker, VideoWorkerReport,
};

/// Number of samples used by the command-line benchmark.
pub const LEGACY_BENCHMARK_FRAMES: u64 = 120;
/// Number of frames used for one setup candidate.
pub const SETUP_BENCHMARK_FRAMES: u64 = 60;

/// Percentile render timings collected by a benchmark.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderLatency {
    pub samples: usize,
    pub p50_nanos: u128,
    pub p95_nanos: u128,
    pub p99_nanos: u128,
    pub max_nanos: u128,
}

/// The complete report emitted by the existing benchmark command.
#[derive(Clone, Copy, Debug)]
pub struct BenchmarkOutput {
    pub report: VideoWorkerReport,
    pub compositor: CompositorMetrics,
    pub elapsed_millis: u128,
    pub multi_worker: obs_rs_video::MultiWorkerSoakReport,
    pub render_latency: RenderLatency,
    pub frame_memory: FrameMemoryMetrics,
    pub rss_before_kib: Option<u64>,
    pub rss_after_kib: Option<u64>,
}

/// A bounded candidate that the setup wizard compares.
#[derive(Clone, Debug)]
pub struct SetupCandidate {
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub tier: u8,
    pub score: u64,
    pub result: Option<SetupCandidateMetrics>,
    pub error: Option<String>,
}

/// The compact metrics needed to rank a setup candidate.
#[derive(Clone, Copy, Debug)]
pub struct SetupCandidateMetrics {
    pub render_p95_nanos: u128,
    pub render_p99_nanos: u128,
    pub render_max_nanos: u128,
    pub requested_frames: u64,
    pub processed_frames: u64,
    pub missed_deadlines: u64,
    pub dropped_frames: u64,
    pub max_queue_age_nanos: u64,
    pub elapsed_millis: u128,
}

/// Results shown by the first-run wizard and retained in compact form by the
/// settings layer.
#[derive(Clone, Debug)]
pub struct SetupBenchmarkReport {
    pub candidates: Vec<SetupCandidate>,
    pub recommended: Option<usize>,
    pub elapsed_millis: u128,
}

impl SetupBenchmarkReport {
    /// Returns a bounded human-readable summary suitable for diagnostics.
    #[must_use]
    pub fn summary(&self) -> String {
        let recommendation = self
            .recommended
            .and_then(|index| self.candidates.get(index))
            .map_or_else(|| "none".to_owned(), |candidate| candidate.label.clone());
        let mut summary = format!(
            "recommended={recommendation}; candidates={}; elapsed_ms={}",
            self.candidates.len(),
            self.elapsed_millis
        );
        for candidate in &self.candidates {
            let (p95, p99, missed, dropped) = candidate.result.map_or((0, 0, 0, 0), |metrics| {
                (
                    metrics.render_p95_nanos,
                    metrics.render_p99_nanos,
                    metrics.missed_deadlines,
                    metrics.dropped_frames,
                )
            });
            let _ = write!(
                summary,
                " | {}:tier={},score={},p95_ns={},p99_ns={},missed={},dropped={}",
                candidate.label, candidate.tier, candidate.score, p95, p99, missed, dropped
            );
        }
        summary.truncate(4_096);
        summary
    }
}

/// Runs the historical 640x360@30 benchmark used by `obs-rs-benchmark`.
///
/// # Errors
///
/// Returns a bounded diagnostic when the fixture cannot be built or the
/// worker/compositor reports an error.
pub fn run_legacy_benchmark() -> Result<BenchmarkOutput, String> {
    let rate = FrameRate::new(30, 1).map_err(error_text)?;
    let format = VideoFormat::new(640, 360, rate).map_err(error_text)?;
    run_benchmark(format, LEGACY_BENCHMARK_FRAMES, true)
}

/// Runs the short local matrix used by the first-run setup wizard.
///
/// # Errors
///
/// Returns a diagnostic when no candidate can be validated or when the
/// benchmark inputs cannot be constructed.
pub fn run_setup_benchmark() -> Result<SetupBenchmarkReport, String> {
    let started = Instant::now();
    let candidates = candidate_formats()?;
    let mut results = Vec::with_capacity(candidates.len());

    for (label, format) in candidates {
        let candidate = match run_benchmark(format, SETUP_BENCHMARK_FRAMES, false) {
            Ok(output) => {
                let metrics = setup_metrics(format, &output);
                let (tier, score) = rank_setup_candidate(format, metrics);
                SetupCandidate {
                    label,
                    width: format.width(),
                    height: format.height(),
                    fps: format.frame_rate().numerator() / format.frame_rate().denominator(),
                    tier,
                    score,
                    result: Some(metrics),
                    error: None,
                }
            }
            Err(error) => SetupCandidate {
                label,
                width: format.width(),
                height: format.height(),
                fps: format.frame_rate().numerator() / format.frame_rate().denominator(),
                tier: 0,
                score: 0,
                result: None,
                error: Some(error),
            },
        };
        results.push(candidate);
    }

    let recommended = results
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.tier > 0 && candidate.result.is_some())
        .max_by(|(_, left), (_, right)| compare_setup_candidates(left, right))
        .map(|(index, _)| index);
    if recommended.is_none() {
        return Err("the local benchmark found no stable candidate".to_owned());
    }
    Ok(SetupBenchmarkReport {
        candidates: results,
        recommended,
        elapsed_millis: started.elapsed().as_millis(),
    })
}

/// Formats the legacy report using the original one-line text schema.
#[must_use]
pub fn legacy_text(output: &BenchmarkOutput) -> String {
    let report = output.report;
    let compositor = output.compositor;
    let multi_worker = output.multi_worker;
    format!(
        "obs-rs benchmark: render_samples={} render_p50_ns={} render_p95_ns={} render_p99_ns={} render_max_ns={} frame_owned_buffers={} frame_owned_bytes={} frame_shared_clones={} frame_cow_buffers={} frame_copied_bytes={} rss_before_kib={} rss_after_kib={} requested={} processed={} cancelled={} empty={} dropped_oldest={} dropped_newest={} missed={} lateness_ns={} max_lateness_ns={} wait_ns={} paced_render_ns={} produced_bytes={} peak_queued_bytes={} remaining={} renders={} source_requests={} source_frames={} empty_sources={} transformed={} filtered={} blends={} elapsed_ms={} multi_workers={} multi_requested={} multi_processed={} multi_missed={} multi_lateness_ns={} multi_produced_bytes={} multi_peak_queued_bytes={} multi_elapsed_ns={}",
        output.render_latency.samples,
        output.render_latency.p50_nanos,
        output.render_latency.p95_nanos,
        output.render_latency.p99_nanos,
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
    )
}

/// Formats the legacy report using its stable JSON schema.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "keeps the dependency-free JSON schema explicit"
)]
pub fn legacy_json(output: &BenchmarkOutput) -> String {
    let report = output.report;
    let compositor = output.compositor;
    let multi = output.multi_worker;
    let rss_before = output
        .rss_before_kib
        .map_or_else(|| "null".to_owned(), |value| value.to_string());
    let rss_after = output
        .rss_after_kib
        .map_or_else(|| "null".to_owned(), |value| value.to_string());
    format!(
        concat!(
            "{{\"schema_version\":1,",
            "\"render_latency\":{{\"samples\":{},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{}}},",
            "\"frame_memory\":{{\"owned_buffers\":{},\"owned_bytes\":{},\"shared_clones\":{},\"copy_on_write_buffers\":{},\"copied_bytes\":{}}},",
            "\"process_memory\":{{\"rss_before_kib\":{},\"rss_after_kib\":{}}},",
            "\"paced_worker\":{{\"requested\":{},\"processed\":{},\"cancelled\":{},\"empty\":{},\"dropped_oldest\":{},\"dropped_newest\":{},\"missed_deadlines\":{},\"total_lateness_ns\":{},\"max_lateness_ns\":{},\"wait_ns\":{},\"render_ns\":{},\"produced_bytes\":{},\"peak_queued_bytes\":{},\"remaining_queue\":{},\"elapsed_ms\":{}}},",
            "\"compositor\":{{\"renders\":{},\"source_requests\":{},\"source_frames\":{},\"empty_sources\":{},\"transformed\":{},\"filtered\":{},\"blends\":{}}},",
            "\"multi_worker\":{{\"workers\":{},\"requested\":{},\"processed\":{},\"missed_deadlines\":{},\"total_lateness_ns\":{},\"produced_bytes\":{},\"peak_queued_bytes\":{},\"elapsed_ns\":{}}}}}"
        ),
        output.render_latency.samples,
        output.render_latency.p50_nanos,
        output.render_latency.p95_nanos,
        output.render_latency.p99_nanos,
        output.render_latency.max_nanos,
        output.frame_memory.owned_buffers(),
        output.frame_memory.owned_bytes(),
        output.frame_memory.shared_clones(),
        output.frame_memory.copy_on_write_buffers(),
        output.frame_memory.copy_on_write_bytes(),
        rss_before,
        rss_after,
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
        output.elapsed_millis,
        compositor.render_calls(),
        compositor.source_requests(),
        compositor.source_frames(),
        compositor.empty_sources(),
        compositor.transformed_frames(),
        compositor.filtered_frames(),
        compositor.blended_layers(),
        multi.workers(),
        multi.requested_frames(),
        multi.processed_frames(),
        multi.missed_deadlines(),
        multi.total_lateness_nanos(),
        multi.produced_bytes(),
        multi.peak_queued_bytes(),
        multi.elapsed_nanos(),
    )
}

fn run_benchmark(
    format: VideoFormat,
    frames: u64,
    include_soak: bool,
) -> Result<BenchmarkOutput, String> {
    let mut runtime = build_runtime(format)?;
    let rss_before_kib = resident_memory_kib();
    let render_latency = measure_unpaced_render_latency(&mut runtime, format, frames)?;
    let frame_memory = frame_memory_metrics();
    let mut worker = VideoWorker::new(format, 3, DropPolicy::DropOldest).map_err(error_text)?;
    let cancellation = CancellationToken::new();
    let mut clock = MonotonicClock::start();
    let started = Instant::now();
    // The scheduler's first deadline is media timestamp zero. Prime that
    // startup deadline outside the measured run; otherwise a real monotonic
    // clock necessarily observes the first callback after timestamp zero and
    // every candidate is reported as having an initial missed deadline even
    // when its steady-state frame work is comfortably within budget.
    let mut render = |deadline: FrameDeadline, output_format: VideoFormat| {
        runtime
            .render_scene(
                "benchmark",
                &VideoRequest::new(deadline.timestamp(), output_format),
            )
            .map_err(error_text)
    };
    let _ = worker
        .run(&mut clock, 1, &cancellation, &mut render)
        .map_err(error_text)?;
    let report = worker
        .run(&mut clock, frames, &cancellation, &mut render)
        .map_err(error_text)?;
    let elapsed_millis = started.elapsed().as_millis();
    let multi_worker = if include_soak {
        run_multi_worker_soak(format, 2, 30, 3, DropPolicy::DropOldest).map_err(error_text)?
    } else {
        // Setup only needs a single-worker stability signal. The legacy report
        // retains the full soak; this path avoids doubling the first-run wait.
        run_multi_worker_soak(format, 1, 1, 1, DropPolicy::DropOldest).map_err(error_text)?
    };
    let rss_after_kib = resident_memory_kib();
    Ok(BenchmarkOutput {
        report,
        compositor: runtime.compositor_metrics(),
        elapsed_millis,
        multi_worker,
        render_latency,
        frame_memory,
        rss_before_kib,
        rss_after_kib,
    })
}

fn build_runtime(format: VideoFormat) -> Result<Runtime, String> {
    let mut runtime = Runtime::new();
    let plugin = BuiltinPlugin::new().map_err(error_text)?;
    runtime.register_plugin(&plugin).map_err(error_text)?;
    runtime.create_scene("benchmark").map_err(error_text)?;
    let width = format.width().to_string();
    let height = format.height().to_string();
    let background = runtime
        .create_source(
            "color_source",
            "background",
            &color_settings(&width, &height),
        )
        .map_err(error_text)?;
    let foreground = runtime
        .create_source(
            "screen_capture",
            "foreground",
            &video_settings(&width, &height),
        )
        .map_err(error_text)?;
    runtime
        .attach_source("benchmark", background)
        .map_err(error_text)?;
    runtime
        .attach_source("benchmark", foreground)
        .map_err(error_text)?;
    runtime
        .set_source_transform(
            "benchmark",
            foreground,
            FrameTransform::new(1_000, 1_000, 0, 0, true, false, 220).map_err(error_text)?,
        )
        .map_err(error_text)?;
    runtime
        .add_source_filter(foreground, FrameFilter::Grayscale)
        .map_err(error_text)?;
    Ok(runtime)
}

fn candidate_formats() -> Result<Vec<(String, VideoFormat)>, String> {
    [
        ("720p30", 1_280, 720, 30),
        ("720p60", 1_280, 720, 60),
        ("1080p30", 1_920, 1_080, 30),
        ("1080p60", 1_920, 1_080, 60),
    ]
    .into_iter()
    .map(|(label, width, height, fps)| {
        let rate = FrameRate::new(fps, 1).map_err(error_text)?;
        let format = VideoFormat::new(width, height, rate).map_err(error_text)?;
        Ok((label.to_owned(), format))
    })
    .collect()
}

fn setup_metrics(format: VideoFormat, output: &BenchmarkOutput) -> SetupCandidateMetrics {
    let report = output.report;
    let frame_budget = u128::from(format.frame_rate().period_nanos().unwrap_or(1));
    // The wall-clock pacer observes unavoidable wake-up jitter at nanosecond
    // precision. Treat that as queue-age telemetry; a setup deadline is
    // considered missed only when the measured render itself reaches the
    // frame budget. This keeps a 1 ms render from being rejected because the
    // OS woke the pacing thread 200 µs after its target.
    let missed_deadlines = if output.render_latency.max_nanos >= frame_budget {
        report.missed_deadlines()
    } else {
        0
    };
    SetupCandidateMetrics {
        render_p95_nanos: output.render_latency.p95_nanos,
        render_p99_nanos: output.render_latency.p99_nanos,
        render_max_nanos: output.render_latency.max_nanos,
        requested_frames: report.requested_frames(),
        processed_frames: report.processed_frames(),
        missed_deadlines,
        dropped_frames: report.dropped_oldest() + report.dropped_newest(),
        max_queue_age_nanos: report.max_lateness_nanos(),
        elapsed_millis: output.elapsed_millis,
    }
}

/// Applies the documented setup acceptance policy to one candidate.
#[must_use]
pub fn rank_setup_candidate(format: VideoFormat, metrics: SetupCandidateMetrics) -> (u8, u64) {
    let frame_budget = u128::from(format.frame_rate().period_nanos().unwrap_or(1));
    let stable = metrics.requested_frames > 0
        && metrics.processed_frames == metrics.requested_frames
        && metrics.missed_deadlines == 0
        && metrics.dropped_frames == 0
        && metrics.render_p95_nanos <= frame_budget.saturating_mul(80) / 100
        && metrics.render_p99_nanos < frame_budget
        && u128::from(metrics.max_queue_age_nanos) <= frame_budget
        && metrics.render_max_nanos <= frame_budget.saturating_mul(2);
    let tier = u8::from(stable).saturating_mul(2);
    let pixels = u64::from(format.width()).saturating_mul(u64::from(format.height()));
    let score = pixels
        .saturating_mul(u64::from(format.frame_rate().numerator()))
        .saturating_div(u64::from(format.frame_rate().denominator()).max(1));
    (tier, score)
}

/// Orders setup candidates by stability/headroom before resolution throughput.
#[must_use]
pub fn compare_setup_candidates(left: &SetupCandidate, right: &SetupCandidate) -> Ordering {
    let Some(left_metrics) = left.result else {
        return Ordering::Less;
    };
    let Some(right_metrics) = right.result else {
        return Ordering::Greater;
    };
    left.tier
        .cmp(&right.tier)
        // Once both candidates are stable, prefer timing headroom before
        // using resolution/FPS as a tie breaker.
        .then_with(|| {
            right_metrics
                .render_p95_nanos
                .cmp(&left_metrics.render_p95_nanos)
        })
        .then_with(|| {
            right_metrics
                .render_p99_nanos
                .cmp(&left_metrics.render_p99_nanos)
        })
        .then_with(|| {
            right_metrics
                .render_max_nanos
                .cmp(&left_metrics.render_max_nanos)
        })
        .then_with(|| {
            right_metrics
                .max_queue_age_nanos
                .cmp(&left_metrics.max_queue_age_nanos)
        })
        .then_with(|| left.score.cmp(&right.score))
}

fn measure_unpaced_render_latency(
    runtime: &mut Runtime,
    format: VideoFormat,
    frames: u64,
) -> Result<RenderLatency, String> {
    for index in 0..10_u64 {
        let request = VideoRequest::new(Timestamp::from_millis(index), format);
        runtime
            .render_scene("benchmark", &request)
            .map_err(error_text)?;
    }
    reset_frame_memory_metrics();
    let sample_count = usize::try_from(frames).map_err(error_text)?;
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..frames {
        let request = VideoRequest::new(Timestamp::from_millis(index + 10), format);
        let started = Instant::now();
        runtime
            .render_scene("benchmark", &request)
            .map_err(error_text)?;
        samples.push(started.elapsed().as_nanos());
    }
    samples.sort_unstable();
    Ok(RenderLatency {
        samples: samples.len(),
        p50_nanos: percentile(&samples, 50),
        p95_nanos: percentile(&samples, 95),
        p99_nanos: percentile(&samples, 99),
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

fn display_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

fn color_settings(width: &str, height: &str) -> Config {
    let mut settings = Config::new();
    settings
        .set("width", width)
        .expect("benchmark width is valid");
    settings
        .set("height", height)
        .expect("benchmark height is valid");
    settings
        .set("color", "#102030FF")
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

fn error_text(error: impl Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::{candidate_formats, percentile, rank_setup_candidate, SetupCandidateMetrics};
    use obs_rs_media::{FrameRate, VideoFormat};

    #[test]
    fn candidate_matrix_is_bounded_and_ordered() {
        let formats = candidate_formats().expect("valid candidate formats");
        assert_eq!(formats.len(), 4);
        assert_eq!(formats[0].0, "720p30");
        assert_eq!(formats[3].0, "1080p60");
    }

    #[test]
    fn percentile_uses_sorted_sample_rank() {
        assert_eq!(percentile(&[10, 20, 30, 40], 50), 20);
        assert_eq!(percentile(&[], 95), 0);
    }

    #[test]
    fn stable_candidate_gets_the_highest_tier() {
        let format = VideoFormat::new(1_280, 720, FrameRate::new(30, 1).unwrap()).unwrap();
        let stable = SetupCandidateMetrics {
            render_p95_nanos: 1_000,
            render_p99_nanos: 1_000,
            render_max_nanos: 1_000,
            requested_frames: 60,
            processed_frames: 60,
            missed_deadlines: 0,
            dropped_frames: 0,
            max_queue_age_nanos: 0,
            elapsed_millis: 1,
        };
        assert_eq!(rank_setup_candidate(format, stable).0, 2);
    }

    #[test]
    fn dropped_or_missed_candidate_is_not_stable_or_recommended() {
        let format = VideoFormat::new(1_280, 720, FrameRate::new(30, 1).unwrap()).unwrap();
        let unstable = SetupCandidateMetrics {
            render_p95_nanos: 1_000,
            render_p99_nanos: 1_000,
            render_max_nanos: 1_000,
            requested_frames: 60,
            processed_frames: 59,
            missed_deadlines: 0,
            dropped_frames: 1,
            max_queue_age_nanos: 0,
            elapsed_millis: 1,
        };
        assert_eq!(rank_setup_candidate(format, unstable).0, 0);
    }
}
