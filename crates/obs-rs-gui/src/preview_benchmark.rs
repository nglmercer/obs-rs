//! End-to-end setup benchmark for the desktop preview path.
//!
//! The portable benchmark crate intentionally has no dependency on the GUI,
//! so it cannot exercise the compatibility presenter. The setup wizard uses
//! this module instead: it drives the same `PreviewRenderer`, bounded WGPU
//! readback bridge, and Slint image conversion that the studio uses.

use std::{thread, time::Instant};

use obs_rs_benchmark::{
    compare_setup_candidates, rank_setup_candidate, SetupBenchmarkReport, SetupCandidate,
    SetupCandidateMetrics,
};
use obs_rs_media::{FrameRate, FrameTransition, VideoFormat, VideoFrame};
use obs_rs_project::ProjectCommand;

use crate::{frame_to_image, initial_project, PreviewRenderer};

const BENCHMARK_FRAMES: u64 = 60;
const WARMUP_FRAMES: usize = 4;

/// Runs the actual desktop preview/presenter benchmark matrix.
pub(crate) fn run_gui_setup_benchmark() -> Result<SetupBenchmarkReport, String> {
    let started = Instant::now();
    let mut candidates = Vec::new();
    for (label, format) in candidate_formats()? {
        match run_candidate(format) {
            Ok(metrics) => {
                let (tier, score) = rank_setup_candidate(format, metrics);
                candidates.push(SetupCandidate {
                    label,
                    width: format.width(),
                    height: format.height(),
                    fps: format.frame_rate().numerator() / format.frame_rate().denominator(),
                    tier,
                    score,
                    result: Some(metrics),
                    error: None,
                });
            }
            Err(error) => candidates.push(SetupCandidate {
                label,
                width: format.width(),
                height: format.height(),
                fps: format.frame_rate().numerator() / format.frame_rate().denominator(),
                tier: 0,
                score: 0,
                result: None,
                error: Some(error),
            }),
        }
    }
    let recommended = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.tier > 0 && candidate.result.is_some())
        .max_by(|(_, left), (_, right)| compare_setup_candidates(left, right))
        .map(|(index, _)| index);
    if recommended.is_none() {
        return Err("the GUI benchmark found no stable candidate".to_owned());
    }
    Ok(SetupBenchmarkReport {
        candidates,
        recommended,
        elapsed_millis: started.elapsed().as_millis(),
    })
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
        let rate = FrameRate::new(fps, 1).map_err(|error| error.to_string())?;
        let format = VideoFormat::new(width, height, rate).map_err(|error| error.to_string())?;
        Ok((label.to_owned(), format))
    })
    .collect()
}

fn run_candidate(format: VideoFormat) -> Result<SetupCandidateMetrics, String> {
    let mut project = initial_project().map_err(|error| error.to_string())?;
    project
        .apply(ProjectCommand::SetProfileVideoFormat {
            profile: "live".to_owned(),
            format,
        })
        .map_err(|error| error.to_string())?;
    let mut renderer = PreviewRenderer::new(&project, 0).map_err(|error| error.to_string())?;
    let preview_format = PreviewRenderer::preview_format_for_canvas(format);
    let tile_format = PreviewRenderer::multiview_tile_format(format);
    let scenes = ["preview", "program", "intermission"];

    // Let the bounded asynchronous presenter fill its first staging slots
    // before measuring deadlines. Warmup is deliberately finite and bounded.
    for index in 0..WARMUP_FRAMES {
        let _ = render_workload(&mut renderer, &scenes, preview_format, tile_format, index)?;
        renderer
            .poll_deferred_readbacks()
            .map_err(|error| error.to_string())?;
        renderer.advance_timestamp();
        thread::yield_now();
    }

    let period_nanos = u128::from(format.frame_rate().period_nanos().unwrap_or(1));
    let started = Instant::now();
    let mut samples = Vec::with_capacity(
        usize::try_from(BENCHMARK_FRAMES).expect("benchmark frame count fits in usize"),
    );
    let mut processed_frames = 0_u64;
    let mut missed_deadlines = 0_u64;
    let mut max_queue_age_nanos = 0_u64;
    for index in 0..BENCHMARK_FRAMES {
        let frame_started = Instant::now();
        if render_workload(
            &mut renderer,
            &scenes,
            preview_format,
            tile_format,
            usize::try_from(index).unwrap_or(usize::MAX),
        )? {
            processed_frames = processed_frames.saturating_add(1);
        }
        renderer
            .poll_deferred_readbacks()
            .map_err(|error| error.to_string())?;
        renderer.advance_timestamp();
        let elapsed_nanos = started.elapsed().as_nanos();
        let deadline_nanos = period_nanos.saturating_mul(u128::from(index + 1));
        let lateness = elapsed_nanos.saturating_sub(deadline_nanos);
        if lateness > 0 {
            missed_deadlines = missed_deadlines.saturating_add(1);
            max_queue_age_nanos =
                max_queue_age_nanos.max(u64::try_from(lateness).unwrap_or(u64::MAX));
        }
        samples.push(frame_started.elapsed().as_nanos());
    }

    Ok(SetupCandidateMetrics {
        render_p95_nanos: percentile(&samples, 95),
        render_p99_nanos: percentile(&samples, 99),
        render_max_nanos: samples.iter().copied().max().unwrap_or(0),
        requested_frames: BENCHMARK_FRAMES,
        processed_frames,
        missed_deadlines,
        dropped_frames: BENCHMARK_FRAMES.saturating_sub(processed_frames),
        max_queue_age_nanos,
        elapsed_millis: started.elapsed().as_millis(),
    })
}

fn render_workload(
    renderer: &mut PreviewRenderer,
    scenes: &[&str],
    preview_format: VideoFormat,
    tile_format: VideoFormat,
    index: usize,
) -> Result<bool, String> {
    match index % 5 {
        // Static scene: measures invalidation/cache behavior and both desktop
        // feeds without continuously changing source pixels.
        0 => {
            let preview = renderer
                .render_preview(scenes[0], preview_format)
                .map_err(|error| error.to_string())?;
            let program = renderer
                .render_program_preview(scenes[1], preview_format)
                .map_err(|error| error.to_string())?;
            let preview_present = present(preview);
            let program_present = present(program);
            Ok(preview_present && program_present)
        }
        // Live capture fixture: the animated built-in source exercises a
        // source frame changing every request without opening a real device.
        1 => renderer
            .render_preview(scenes[2], preview_format)
            .map(present)
            .map_err(|error| error.to_string()),
        // Studio workload: transition the two desktop feeds through the same
        // renderer boundary used by the program preview.
        2 => renderer
            .render_transition_preview(
                scenes[0],
                scenes[1],
                preview_format,
                FrameTransition::CrossFade {
                    progress_milli: 500,
                },
            )
            .map(present)
            .map_err(|error| error.to_string()),
        // Bounded multiview fan-out: source content is rendered once per tile
        // and each tile stays at thumbnail resolution.
        3 => {
            let mut any = false;
            for scene in scenes {
                any |= present(
                    renderer
                        .render_multiview_tile(scene, tile_format)
                        .map_err(|error| error.to_string())?,
                );
            }
            Ok(any)
        }
        // Output-active workload: exercise the encoder-oriented GPU path when
        // available, while retaining the RGBA presenter fallback.
        _ => match renderer.encoder_frame(scenes[1]) {
            Ok(Some(_)) => Ok(true),
            Ok(None) if renderer.deferred_readback() => Ok(true),
            Ok(None) => renderer
                .render_program(scenes[1])
                .map(present)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        },
    }
}

fn present(frame: Option<VideoFrame>) -> bool {
    if let Some(frame) = frame {
        let _ = frame_to_image(&frame);
        true
    } else {
        false
    }
}

fn percentile(samples: &[u128], percentile: u8) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = usize::from(percentile.clamp(1, 100))
        .saturating_mul(sorted.len())
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}
