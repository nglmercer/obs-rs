use std::{
    error::Error,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use obs_rs_media::{LatencyMetrics, RawVideoFrame, VideoFormat, VideoFrame};
use obs_rs_project::Project;

use crate::preview::{PreviewRenderer, RuntimeDiagnostics, TransformDraft};

struct PreviewRequest {
    project: Option<Project>,
    revision: u64,
    preview_scene: Option<String>,
    preview_format: VideoFormat,
    program_scene: Option<String>,
    program_preview_format: VideoFormat,
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
    pub(crate) program_output: Option<RawVideoFrame>,
    /// Full-canvas RGBA is present only when the output cannot consume the
    /// accelerated raw path, such as CPU fallback or output scaling.
    pub(crate) program_output_frame: Option<VideoFrame>,
    pub(crate) error: Option<String>,
    pub(crate) metrics: String,
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
    pub(crate) worker: LatencyMetrics,
    pub(crate) frame_copy: LatencyMetrics,
    pub(crate) frame_copy_bytes: u64,
    pub(crate) slint_update: LatencyMetrics,
    pub(crate) ui_callback: LatencyMetrics,
}

/// What one render request asks the worker to produce.
#[derive(Clone, Copy)]
pub(crate) struct RenderTargets<'a> {
    pub(crate) preview_scene: Option<&'a str>,
    pub(crate) preview_format: VideoFormat,
    pub(crate) program_scene: Option<&'a str>,
    pub(crate) program_preview_format: VideoFormat,
    /// Whether the program canvas is wanted as well as the preview one.
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
        let request = PreviewRequest {
            project,
            revision,
            preview_scene: targets.preview_scene.map(str::to_owned),
            preview_format: targets.preview_format,
            program_scene: targets.program_scene.map(str::to_owned),
            program_preview_format: targets.program_preview_format,
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
                program_output: None,
                program_output_frame: None,
                error: Some(error.clone()),
                metrics: "Preview worker unavailable".to_owned(),
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
        );
        if let Ok(mut performance) = performance.lock() {
            performance.program_render.record(program_started.elapsed());
        }
        program
    } else {
        Ok(None)
    };
    let error = preview
        .as_ref()
        .err()
        .cloned()
        .or_else(|| program.as_ref().err().cloned())
        .or_else(|| {
            (request.preview_scene.is_some() && matches!(preview, Ok(None)))
                .then(|| "preview scene produced no frame".to_owned())
        })
        .or_else(|| {
            (request.render_program
                && request.program_scene.is_some()
                && matches!(program, Ok(None)))
            .then(|| "program scene produced no frame".to_owned())
        });
    let (output, output_frame) = if request.prepare_output {
        match request.program_scene.as_deref() {
            Some(scene) if request.prepare_output_rgba => match renderer.render_program(scene) {
                Ok(frame) => (Ok(None), frame),
                Err(error) => (Err(error), None),
            },
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
        || (request.prepare_output && request.program_scene.is_some())
    {
        renderer.advance_timestamp();
    }
    let error = error.or_else(|| output.as_ref().err().map(ToString::to_string));
    PreviewResult {
        preview_scene: request.preview_scene,
        preview_frame: preview.as_ref().ok().cloned().flatten(),
        program_scene: request.program_scene,
        program_frame: program.as_ref().ok().cloned().flatten(),
        program_output: output.ok().flatten(),
        program_output_frame: output_frame,
        error,
        metrics: renderer.metrics_summary(),
        #[cfg(test)]
        render_thread: thread::current().id(),
    }
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
) -> Result<Option<VideoFrame>, String> {
    let Some(scene) = scene else {
        return Ok(None);
    };
    renderer
        .render_program_preview(scene, format)
        .map_err(|error| error.to_string())
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
            program_output: None,
            program_output_frame: None,
            error: Some(error),
            metrics: "Preview project sync failed".to_owned(),
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
mod tests {
    use std::time::Duration;

    use super::*;

    fn request(scene: &str) -> PreviewRequest {
        PreviewRequest {
            project: None,
            revision: 0,
            preview_scene: Some(scene.to_owned()),
            preview_format: VideoFormat::new(
                16,
                16,
                obs_rs_media::FrameRate::new(30, 1).expect("rate"),
            )
            .expect("preview format"),
            program_scene: None,
            program_preview_format: VideoFormat::new(
                16,
                16,
                obs_rs_media::FrameRate::new(30, 1).expect("rate"),
            )
            .expect("program preview format"),
            render_program: false,
            prepare_output: false,
            prepare_output_rgba: false,
            draft: None,
        }
    }

    #[test]
    fn pending_preview_requests_keep_only_the_newest() {
        let shared = SharedRequest {
            latest: Mutex::new(None),
            ready: Condvar::new(),
            stopped: AtomicBool::new(false),
        };
        let depth = AtomicUsize::new(0);
        let drops = AtomicU64::new(0);

        enqueue_request(&shared, &depth, &drops, request("old"));
        enqueue_request(&shared, &depth, &drops, request("new"));

        assert_eq!(depth.load(Ordering::Relaxed), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(
            shared
                .latest
                .lock()
                .expect("pending request")
                .as_ref()
                .and_then(|request| request.preview_scene.as_deref()),
            Some("new")
        );
    }

    #[test]
    fn preview_format_is_bounded_and_preserves_canvas_aspect() {
        let canvas = VideoFormat::new(
            1_920,
            1_080,
            obs_rs_media::FrameRate::new(60, 1).expect("rate"),
        )
        .expect("canvas format");
        let preview = PreviewRenderer::preview_format_for_canvas(canvas);
        assert_eq!((preview.width(), preview.height()), (1_048, 590));
        assert_eq!(preview.frame_rate(), canvas.frame_rate());

        let small = VideoFormat::new(640, 360, obs_rs_media::FrameRate::new(30, 1).expect("rate"))
            .expect("small canvas");
        let preview = PreviewRenderer::preview_format_for_canvas(small);
        assert_eq!((preview.width(), preview.height()), (640, 360));
    }

    #[test]
    fn scene_composition_runs_on_the_preview_thread() {
        let project = crate::initial_project().expect("project");
        let scene = project
            .active_profile_spec()
            .expect("profile")
            .scenes()
            .next()
            .expect("scene")
            .id()
            .as_str()
            .to_owned();
        let worker = PreviewWorker::spawn(
            project.clone(),
            0,
            &Arc::new(Mutex::new(RuntimeDiagnostics::default())),
        )
        .expect("preview worker");
        let caller = thread::current().id();
        worker.request_render(
            &project,
            0,
            RenderTargets {
                preview_scene: Some(&scene),
                preview_format: PreviewRenderer::preview_format_for_canvas(
                    project
                        .active_profile_spec()
                        .expect("profile")
                        .video_format(),
                ),
                program_scene: Some(&scene),
                program_preview_format: PreviewRenderer::preview_format_for_canvas(
                    project
                        .active_profile_spec()
                        .expect("profile")
                        .video_format(),
                ),
                render_program: true,
                prepare_output: true,
                prepare_output_rgba: true,
                draft: None,
            },
        );

        let mut result = None;
        // The workspace runs many GUI/runtime tests in parallel. Keep the
        // wait bounded, but leave enough scheduler slack for a loaded managed
        // runner before declaring the worker missing its result.
        for _ in 0..500 {
            result = worker.try_take_latest();
            if result.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let result = result.expect("render result");
        assert_ne!(result.render_thread, caller);
        let canvas_format = project
            .active_profile_spec()
            .expect("profile")
            .video_format();
        assert_eq!(
            result.preview_frame.expect("preview frame").format(),
            PreviewRenderer::preview_format_for_canvas(canvas_format)
        );
        assert_eq!(
            result.program_frame.expect("program frame").format(),
            PreviewRenderer::preview_format_for_canvas(canvas_format)
        );
        assert!(result.program_output.is_none());
        assert_eq!(
            result
                .program_output_frame
                .expect("full output fallback frame")
                .format(),
            canvas_format
        );
        assert_eq!(worker.queue_depth(), 0);
        let performance = worker.performance();
        assert_eq!(performance.preview_render.samples(), 1);
        assert_eq!(performance.worker.samples(), 1);
    }
}
