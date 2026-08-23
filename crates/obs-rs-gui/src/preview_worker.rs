use std::{
    error::Error,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use obs_rs_config::Config;
use obs_rs_media::{LatencyMetrics, RawVideoFrame, VideoFormat, VideoFrame};
use obs_rs_project::Project;
use obs_rs_ui::TransitionSnapshot;

use crate::preview::{PreviewRenderer, RuntimeDiagnostics, TransformDraft};

/// Multiview is intentionally capped: a desktop preview must never turn a
/// scene collection into an unbounded render fan-out.
pub(crate) const MAX_MULTIVIEW_SCENES: usize = 16;

/// The scene-item identity captured by a selected-source projector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceProjectorTarget {
    pub(crate) scene: String,
    pub(crate) item: String,
}

/// The stable scene identity captured by a scene projector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SceneProjectorTarget {
    pub(crate) scene: String,
}

struct PreviewRequest {
    project: Option<Project>,
    revision: u64,
    preview_scene: Option<String>,
    preview_format: VideoFormat,
    program_scene: Option<String>,
    program_transition: Option<TransitionSnapshot>,
    program_preview_format: VideoFormat,
    multiview_scenes: Vec<String>,
    multiview_format: VideoFormat,
    source_projector: Option<SourceProjectorTarget>,
    scene_projector: Option<SceneProjectorTarget>,
    render_program: bool,
    prepare_output: bool,
    prepare_output_rgba: bool,
    /// The canvas drag in progress, which is not a project edit yet.
    draft: Option<TransformDraft>,
}

pub(crate) struct PreviewResult {
    pub(crate) preview_scene: Option<String>,
    pub(crate) preview_frame: Option<VideoFrame>,
    pub(crate) program_scene: Option<String>,
    pub(crate) program_frame: Option<VideoFrame>,
    pub(crate) multiview_frame: Option<VideoFrame>,
    pub(crate) source_projector_frame: Option<VideoFrame>,
    pub(crate) scene_projector_frame: Option<VideoFrame>,
    pub(crate) program_output: Option<RawVideoFrame>,
    /// Full-canvas RGBA is present only when the output cannot consume the
    /// accelerated raw path, such as CPU fallback or output scaling.
    pub(crate) program_output_frame: Option<VideoFrame>,
    pub(crate) error: Option<String>,
    pub(crate) metrics: String,
    pub(crate) source_settings_updates: Vec<(String, String, Config)>,
    #[cfg(test)]
    render_thread: thread::ThreadId,
}

struct SharedRequest {
    latest: Mutex<Option<PreviewRequest>>,
    ready: Condvar,
    stopped: AtomicBool,
}

#[derive(Clone, Copy)]
struct PreviewLoopShared<'a> {
    request: &'a SharedRequest,
    result: &'a Mutex<Option<PreviewResult>>,
    applied_revision: &'a AtomicU64,
    queue_depth: &'a AtomicUsize,
    dropped_requests: &'a AtomicU64,
    performance: &'a Mutex<PreviewPerformanceSnapshot>,
    diagnostics: &'a Mutex<RuntimeDiagnostics>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreviewPerformanceSnapshot {
    pub(crate) preview_render: LatencyMetrics,
    pub(crate) program_render: LatencyMetrics,
    pub(crate) multiview_render: LatencyMetrics,
    pub(crate) source_projector_render: LatencyMetrics,
    pub(crate) scene_projector_render: LatencyMetrics,
    pub(crate) worker: LatencyMetrics,
    pub(crate) frame_copy: LatencyMetrics,
    pub(crate) frame_copy_bytes: u64,
    pub(crate) slint_update: LatencyMetrics,
    pub(crate) ui_callback: LatencyMetrics,
}

/// What one render request asks the worker to produce.
#[derive(Clone)]
pub(crate) struct RenderTargets<'a> {
    pub(crate) preview_scene: Option<&'a str>,
    pub(crate) preview_format: VideoFormat,
    pub(crate) program_scene: Option<&'a str>,
    pub(crate) program_transition: Option<TransitionSnapshot>,
    pub(crate) program_preview_format: VideoFormat,
    /// Ordered scene IDs for the bounded multiview grid.
    pub(crate) multiview_scenes: Vec<String>,
    pub(crate) multiview_format: VideoFormat,
    /// Selected scene-item source to render in the bounded source projector.
    pub(crate) source_projector: Option<&'a SourceProjectorTarget>,
    /// Stable scene to render in the bounded scene projector.
    pub(crate) scene_projector: Option<&'a SceneProjectorTarget>,
    /// Whether the bounded program view is wanted as well as the preview one.
    pub(crate) render_program: bool,
    /// Whether the program frame should also be converted for the encoder.
    pub(crate) prepare_output: bool,
    /// Requests a full-canvas RGBA frame for output scaling. The normal GPU
    /// output path does not need this CPU-compatible frame.
    pub(crate) prepare_output_rgba: bool,
    /// The canvas drag in progress, which is not a project edit yet.
    pub(crate) draft: Option<&'a TransformDraft>,
}

/// Background scene compositor with capacity-one latest-request/result slots.
pub(crate) struct PreviewWorker {
    request: Arc<SharedRequest>,
    result: Arc<Mutex<Option<PreviewResult>>>,
    applied_revision: Arc<AtomicU64>,
    queue_depth: Arc<AtomicUsize>,
    dropped_requests: Arc<AtomicU64>,
    performance: Arc<Mutex<PreviewPerformanceSnapshot>>,
    join: Option<JoinHandle<()>>,
}

impl PreviewWorker {
    /// Starts the one thread that owns live capture devices.
    ///
    /// `diagnostics` is the slot the studio window reads engine counters from;
    /// it exists so the window never needs a runtime of its own.
    pub(crate) fn spawn(
        project: Project,
        revision: u64,
        diagnostics: &Arc<Mutex<RuntimeDiagnostics>>,
    ) -> Result<Self, Box<dyn Error>> {
        let request = Arc::new(SharedRequest {
            latest: Mutex::new(None),
            ready: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        let result = Arc::new(Mutex::new(None));
        let applied_revision = Arc::new(AtomicU64::new(u64::MAX));
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let dropped_requests = Arc::new(AtomicU64::new(0));
        let performance = Arc::new(Mutex::new(PreviewPerformanceSnapshot::default()));
        let thread_request = Arc::clone(&request);
        let thread_result = Arc::clone(&result);
        let thread_revision = Arc::clone(&applied_revision);
        let thread_depth = Arc::clone(&queue_depth);
        let thread_drops = Arc::clone(&dropped_requests);
        let thread_performance = Arc::clone(&performance);
        let thread_diagnostics = Arc::clone(diagnostics);
        let join = thread::Builder::new()
            .name("obs-rs-preview".to_owned())
            .spawn(move || {
                preview_loop(
                    &project,
                    revision,
                    PreviewLoopShared {
                        request: &thread_request,
                        result: &thread_result,
                        applied_revision: &thread_revision,
                        queue_depth: &thread_depth,
                        dropped_requests: &thread_drops,
                        performance: &thread_performance,
                        diagnostics: &thread_diagnostics,
                    },
                );
            })?;
        Ok(Self {
            request,
            result,
            applied_revision,
            queue_depth,
            dropped_requests,
            performance,
            join: Some(join),
        })
    }

    pub(crate) fn request_render(
        &self,
        project: &Project,
        revision: u64,
        targets: RenderTargets<'_>,
    ) {
        let project =
            (self.applied_revision.load(Ordering::Acquire) != revision).then(|| project.clone());
        let mut multiview_scenes = targets.multiview_scenes;
        multiview_scenes.truncate(MAX_MULTIVIEW_SCENES);
        let request = PreviewRequest {
            project,
            revision,
            preview_scene: targets.preview_scene.map(str::to_owned),
            preview_format: targets.preview_format,
            program_scene: targets.program_scene.map(str::to_owned),
            program_transition: targets.program_transition,
            program_preview_format: targets.program_preview_format,
            multiview_scenes,
            multiview_format: targets.multiview_format,
            source_projector: targets.source_projector.cloned(),
            scene_projector: targets.scene_projector.cloned(),
            render_program: targets.render_program,
            prepare_output: targets.prepare_output,
            prepare_output_rgba: targets.prepare_output_rgba,
            draft: targets.draft.cloned(),
        };
        enqueue_request(
            &self.request,
            &self.queue_depth,
            &self.dropped_requests,
            request,
        );
    }

    pub(crate) fn try_take_latest(&self) -> Option<PreviewResult> {
        self.result.lock().ok()?.take()
    }

    pub(crate) fn queue_depth(&self) -> usize {
        self.queue_depth.load(Ordering::Acquire)
    }

    pub(crate) fn dropped_requests(&self) -> u64 {
        self.dropped_requests.load(Ordering::Relaxed)
    }

    pub(crate) fn record_frame_copy(&self, duration: Duration, bytes: usize) {
        if let Ok(mut performance) = self.performance.lock() {
            performance.frame_copy.record(duration);
            performance.frame_copy_bytes = performance
                .frame_copy_bytes
                .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        }
    }

    pub(crate) fn record_slint_update(&self, duration: Duration) {
        if let Ok(mut performance) = self.performance.lock() {
            performance.slint_update.record(duration);
        }
    }

    pub(crate) fn record_ui_callback(&self, duration: Duration) {
        if let Ok(mut performance) = self.performance.lock() {
            performance.ui_callback.record(duration);
        }
    }

    pub(crate) fn performance(&self) -> PreviewPerformanceSnapshot {
        self.performance.lock().map_or_else(
            |_| PreviewPerformanceSnapshot::default(),
            |performance| *performance,
        )
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        self.request.stopped.store(true, Ordering::Release);
        self.request.ready.notify_one();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn preview_loop(project: &Project, revision: u64, shared: PreviewLoopShared<'_>) {
    let mut renderer = PreviewRenderer::new(project, revision).map_err(|error| error.to_string());
    if renderer.is_ok() {
        shared.applied_revision.store(revision, Ordering::Release);
    }
    while let Some(next) = wait_for_request(shared.request, shared.queue_depth) {
        let worker_started = std::time::Instant::now();
        if let (Ok(renderer), Some(project)) = (&mut renderer, next.project.as_ref()) {
            if let Err(error) = renderer.sync_project(project, next.revision) {
                renderer_error(shared.result, shared.dropped_requests, error.to_string());
                continue;
            }
            shared
                .applied_revision
                .store(next.revision, Ordering::Release);
        } else if renderer.is_err() {
            let Some(project) = next.project.as_ref() else {
                continue;
            };
            renderer =
                PreviewRenderer::new(project, next.revision).map_err(|error| error.to_string());
            if renderer.is_ok() {
                shared
                    .applied_revision
                    .store(next.revision, Ordering::Release);
            }
        }

        if let Ok(renderer) = &mut renderer {
            renderer.set_transform_draft(next.draft.as_ref());
            if let Ok(mut diagnostics) = shared.diagnostics.lock() {
                *diagnostics = renderer.diagnostics();
            }
        }
        let completed = match &mut renderer {
            Ok(renderer) => render_request(renderer, next, shared.performance),
            Err(error) => PreviewResult {
                preview_scene: None,
                preview_frame: None,
                program_scene: None,
                program_frame: None,
                multiview_frame: None,
                source_projector_frame: None,
                scene_projector_frame: None,
                program_output: None,
                program_output_frame: None,
                error: Some(error.clone()),
                metrics: "Preview worker unavailable".to_owned(),
                source_settings_updates: Vec::new(),
                #[cfg(test)]
                render_thread: thread::current().id(),
            },
        };
        if let Ok(mut performance) = shared.performance.lock() {
            performance.worker.record(worker_started.elapsed());
        }
        publish_result(shared.result, shared.dropped_requests, completed);
    }
}

fn wait_for_request(request: &SharedRequest, queue_depth: &AtomicUsize) -> Option<PreviewRequest> {
    let mut pending = request.latest.lock().ok()?;
    while pending.is_none() && !request.stopped.load(Ordering::Acquire) {
        pending = request.ready.wait(pending).ok()?;
    }
    if request.stopped.load(Ordering::Acquire) {
        return None;
    }
    let request = pending.take();
    queue_depth.store(0, Ordering::Release);
    request
}

#[allow(
    clippy::too_many_lines,
    reason = "one worker request keeps preview, program, multiview, and output consumers aligned"
)]
fn render_request(
    renderer: &mut PreviewRenderer,
    request: PreviewRequest,
    performance: &Mutex<PreviewPerformanceSnapshot>,
) -> PreviewResult {
    let preview_started = std::time::Instant::now();
    let preview = render_preview_scene(
        renderer,
        request.preview_scene.as_deref(),
        request.preview_format,
    );
    if let Ok(mut performance) = performance.lock() {
        performance.preview_render.record(preview_started.elapsed());
    }
    let program = if request.render_program {
        let program_started = std::time::Instant::now();
        let program = render_program_scene(
            renderer,
            request.program_scene.as_deref(),
            request.program_preview_format,
            request.program_transition.as_ref(),
        );
        if let Ok(mut performance) = performance.lock() {
            performance.program_render.record(program_started.elapsed());
        }
        program
    } else {
        Ok(None)
    };
    let multiview = if request.multiview_scenes.is_empty() {
        Ok(None)
    } else {
        let multiview_started = std::time::Instant::now();
        let multiview = render_multiview_scene(
            renderer,
            &request.multiview_scenes,
            request.multiview_format,
        );
        if let Ok(mut performance) = performance.lock() {
            performance
                .multiview_render
                .record(multiview_started.elapsed());
        }
        multiview
    };
    let source_projector = if let Some(target) = request.source_projector.as_ref() {
        let source_started = std::time::Instant::now();
        let format = PreviewRenderer::preview_format_for_canvas(renderer.format);
        let source_projector = render_source_projector(renderer, target, format);
        if let Ok(mut performance) = performance.lock() {
            performance
                .source_projector_render
                .record(source_started.elapsed());
        }
        source_projector
    } else {
        Ok(None)
    };
    let scene_projector = if let Some(target) = request.scene_projector.as_ref() {
        let scene_started = std::time::Instant::now();
        let format = PreviewRenderer::preview_format_for_canvas(renderer.format);
        let scene_projector = render_scene_projector(renderer, target, format);
        if let Ok(mut performance) = performance.lock() {
            performance
                .scene_projector_render
                .record(scene_started.elapsed());
        }
        scene_projector
    } else {
        Ok(None)
    };
    let error = preview
        .as_ref()
        .err()
        .cloned()
        .or_else(|| program.as_ref().err().cloned())
        .or_else(|| multiview.as_ref().err().cloned())
        .or_else(|| source_projector.as_ref().err().cloned())
        .or_else(|| scene_projector.as_ref().err().cloned())
        .or_else(|| {
            (request.preview_scene.is_some() && matches!(preview, Ok(None)))
                .then(|| "preview scene produced no frame".to_owned())
        })
        .or_else(|| {
            (request.render_program
                && request.program_scene.is_some()
                && matches!(program, Ok(None)))
            .then(|| "program scene produced no frame".to_owned())
        })
        .or_else(|| {
            (!request.multiview_scenes.is_empty() && matches!(multiview, Ok(None)))
                .then(|| "multiview scenes produced no frame".to_owned())
        })
        .or_else(|| {
            (request.source_projector.is_some() && matches!(source_projector, Ok(None)))
                .then(|| "selected source produced no frame".to_owned())
        })
        .or_else(|| {
            (request.scene_projector.is_some() && matches!(scene_projector, Ok(None)))
                .then(|| "scene projector target produced no frame".to_owned())
        });
    let (output, output_frame) = if request.prepare_output {
        match request.program_scene.as_deref() {
            Some(_) if request.prepare_output_rgba && request.program_transition.is_some() => {
                match render_program_transition(renderer, request.program_transition.as_ref()) {
                    Ok(frame) => (Ok(None), frame),
                    Err(error) => (Err(error), None),
                }
            }
            Some(scene) if request.prepare_output_rgba => match renderer.render_program(scene) {
                Ok(frame) => (Ok(None), frame),
                Err(error) => (Err(error), None),
            },
            Some(_) if request.program_transition.is_some() => {
                match render_program_transition(renderer, request.program_transition.as_ref()) {
                    Ok(frame) => (Ok(None), frame),
                    Err(error) => (Err(error), None),
                }
            }
            Some(scene) => match renderer.encoder_frame(scene) {
                Ok(Some(frame)) => (Ok(Some(frame)), None),
                Ok(None) => match renderer.render_program(scene) {
                    Ok(frame) => (Ok(None), frame),
                    Err(error) => (Err(error), None),
                },
                Err(error) => (Err(error), None),
            },
            None => (Ok(None), None),
        }
    } else {
        (Ok(None), None)
    };
    if request.preview_scene.is_some()
        || (request.render_program && request.program_scene.is_some())
        || !request.multiview_scenes.is_empty()
        || request.source_projector.is_some()
        || request.scene_projector.is_some()
        || (request.prepare_output && request.program_scene.is_some())
    {
        renderer.advance_timestamp();
    }
    let error = error.or_else(|| output.as_ref().err().map(ToString::to_string));
    let source_settings_updates = renderer.take_source_settings_updates();
    PreviewResult {
        preview_scene: request.preview_scene,
        preview_frame: preview.as_ref().ok().cloned().flatten(),
        program_scene: request.program_scene,
        program_frame: program.as_ref().ok().cloned().flatten(),
        multiview_frame: multiview.as_ref().ok().cloned().flatten(),
        source_projector_frame: source_projector.as_ref().ok().cloned().flatten(),
        scene_projector_frame: scene_projector.as_ref().ok().cloned().flatten(),
        program_output: output.ok().flatten(),
        program_output_frame: output_frame,
        error,
        metrics: renderer.metrics_summary(),
        source_settings_updates,
        #[cfg(test)]
        render_thread: thread::current().id(),
    }
}

fn render_source_projector(
    renderer: &mut PreviewRenderer,
    target: &SourceProjectorTarget,
    format: VideoFormat,
) -> Result<Option<VideoFrame>, String> {
    renderer
        .render_source(&target.scene, &target.item, format)
        .map_err(|error| error.to_string())
}

fn render_scene_projector(
    renderer: &mut PreviewRenderer,
    target: &SceneProjectorTarget,
    format: VideoFormat,
) -> Result<Option<VideoFrame>, String> {
    renderer
        .render_scene_projector(&target.scene, format)
        .map_err(|error| error.to_string())
}

fn render_multiview_scene(
    renderer: &mut PreviewRenderer,
    scenes: &[String],
    format: VideoFormat,
) -> Result<Option<VideoFrame>, String> {
    if scenes.is_empty() {
        return Ok(None);
    }
    let scenes = &scenes[..scenes.len().min(MAX_MULTIVIEW_SCENES)];
    let (columns, rows) = multiview_grid_dimensions(scenes.len());
    let tile_format = PreviewRenderer::multiview_tile_format(renderer.format);
    let tile_width = usize::try_from(tile_format.width()).unwrap_or(usize::MAX);
    let tile_height = usize::try_from(tile_format.height()).unwrap_or(usize::MAX);
    let composite_width = tile_width.saturating_mul(columns);
    let composite_height = tile_height.saturating_mul(rows);
    let expected_format = VideoFormat::new(
        u32::try_from(composite_width).map_err(|_| "multiview width is too large".to_owned())?,
        u32::try_from(composite_height).map_err(|_| "multiview height is too large".to_owned())?,
        renderer.format.frame_rate(),
    )
    .map_err(|error| error.to_string())?;
    if format != expected_format {
        return Err("multiview target format does not match the bounded grid".to_owned());
    }
    let composite_format = format;
    let mut pixels = vec![0_u8; composite_format.rgba_bytes()];
    for (index, scene) in scenes.iter().enumerate() {
        let Some(frame) = renderer
            .render_preview(scene, tile_format)
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let column = index % columns;
        let row = index / columns;
        let source_row_bytes = tile_width.saturating_mul(4);
        for tile_row in 0..tile_height {
            let source_start = tile_row.saturating_mul(source_row_bytes);
            let destination_start = (row.saturating_mul(tile_height).saturating_add(tile_row))
                .saturating_mul(composite_width)
                .saturating_mul(4)
                .saturating_add(column.saturating_mul(source_row_bytes));
            let destination_end = destination_start.saturating_add(source_row_bytes);
            let source_end = source_start.saturating_add(source_row_bytes);
            if source_end <= frame.pixels().len() && destination_end <= pixels.len() {
                pixels[destination_start..destination_end]
                    .copy_from_slice(&frame.pixels()[source_start..source_end]);
            }
        }
    }
    VideoFrame::new(composite_format, renderer.timestamp(), pixels)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Returns a stable near-square grid for a bounded scene count.
pub(crate) fn multiview_grid_dimensions(scene_count: usize) -> (usize, usize) {
    let scene_count = scene_count.clamp(1, MAX_MULTIVIEW_SCENES);
    let mut columns = 1_usize;
    while columns.saturating_mul(columns) < scene_count {
        columns = columns.saturating_add(1);
    }
    (columns, scene_count.div_ceil(columns))
}

fn render_preview_scene(
    renderer: &mut PreviewRenderer,
    scene: Option<&str>,
    format: VideoFormat,
) -> Result<Option<VideoFrame>, String> {
    let Some(scene) = scene else {
        return Ok(None);
    };
    renderer
        .render_preview(scene, format)
        .map_err(|error| error.to_string())
}

fn render_program_scene(
    renderer: &mut PreviewRenderer,
    scene: Option<&str>,
    format: VideoFormat,
    transition: Option<&TransitionSnapshot>,
) -> Result<Option<VideoFrame>, String> {
    let Some(scene) = scene else {
        return Ok(None);
    };
    if let Some(transition) = transition {
        if transition.destination_scene() != scene {
            return Err("program transition destination does not match program scene".to_owned());
        }
        return renderer
            .render_transition_preview(
                transition.source_scene(),
                transition.destination_scene(),
                format,
                transition.transition(),
            )
            .map_err(|error| error.to_string());
    }
    renderer
        .render_program_preview(scene, format)
        .map_err(|error| error.to_string())
}

fn render_program_transition(
    renderer: &mut PreviewRenderer,
    transition: Option<&TransitionSnapshot>,
) -> Result<Option<VideoFrame>, Box<dyn Error>> {
    let Some(transition) = transition else {
        return Ok(None);
    };
    renderer.render_transition(
        transition.source_scene(),
        transition.destination_scene(),
        transition.transition(),
    )
}

fn renderer_error(
    result: &Mutex<Option<PreviewResult>>,
    dropped_requests: &AtomicU64,
    error: String,
) {
    publish_result(
        result,
        dropped_requests,
        PreviewResult {
            preview_scene: None,
            preview_frame: None,
            program_scene: None,
            program_frame: None,
            multiview_frame: None,
            source_projector_frame: None,
            scene_projector_frame: None,
            program_output: None,
            program_output_frame: None,
            error: Some(error),
            metrics: "Preview project sync failed".to_owned(),
            source_settings_updates: Vec::new(),
            #[cfg(test)]
            render_thread: thread::current().id(),
        },
    );
}

fn publish_result(
    slot: &Mutex<Option<PreviewResult>>,
    dropped_requests: &AtomicU64,
    result: PreviewResult,
) {
    if let Ok(mut latest) = slot.lock() {
        if latest.replace(result).is_some() {
            dropped_requests.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn enqueue_request(
    shared: &SharedRequest,
    queue_depth: &AtomicUsize,
    dropped_requests: &AtomicU64,
    request: PreviewRequest,
) {
    if let Ok(mut pending) = shared.latest.lock() {
        if pending.replace(request).is_some() {
            dropped_requests.fetch_add(1, Ordering::Relaxed);
        }
        queue_depth.store(1, Ordering::Release);
        shared.ready.notify_one();
    }
}
#[cfg(test)]
#[path = "preview_worker_tests.rs"]
mod tests;
