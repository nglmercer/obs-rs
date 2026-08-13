use std::{
    error::Error,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
};

use obs_rs_media::VideoFrame;
use obs_rs_project::Project;

use crate::PreviewRenderer;

struct PreviewRequest {
    project: Option<Project>,
    revision: u64,
    preview_scene: Option<String>,
    program_scene: Option<String>,
    render_program: bool,
}

pub(crate) struct PreviewResult {
    pub(crate) preview_scene: Option<String>,
    pub(crate) preview_frame: Option<VideoFrame>,
    pub(crate) program_scene: Option<String>,
    pub(crate) program_frame: Option<VideoFrame>,
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

/// Background scene compositor with capacity-one latest-request/result slots.
pub(crate) struct PreviewWorker {
    request: Arc<SharedRequest>,
    result: Arc<Mutex<Option<PreviewResult>>>,
    applied_revision: Arc<AtomicU64>,
    queue_depth: Arc<AtomicUsize>,
    dropped_requests: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
}

impl PreviewWorker {
    pub(crate) fn spawn(project: Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let request = Arc::new(SharedRequest {
            latest: Mutex::new(None),
            ready: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        let result = Arc::new(Mutex::new(None));
        let applied_revision = Arc::new(AtomicU64::new(u64::MAX));
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let dropped_requests = Arc::new(AtomicU64::new(0));
        let thread_request = Arc::clone(&request);
        let thread_result = Arc::clone(&result);
        let thread_revision = Arc::clone(&applied_revision);
        let thread_depth = Arc::clone(&queue_depth);
        let thread_drops = Arc::clone(&dropped_requests);
        let join = thread::Builder::new()
            .name("obs-rs-preview".to_owned())
            .spawn(move || {
                preview_loop(
                    &project,
                    revision,
                    &thread_request,
                    &thread_result,
                    &thread_revision,
                    &thread_depth,
                    &thread_drops,
                );
            })?;
        Ok(Self {
            request,
            result,
            applied_revision,
            queue_depth,
            dropped_requests,
            join: Some(join),
        })
    }

    pub(crate) fn request_render(
        &self,
        project: &Project,
        revision: u64,
        preview_scene: Option<&str>,
        program_scene: Option<&str>,
        render_program: bool,
    ) {
        let project =
            (self.applied_revision.load(Ordering::Acquire) != revision).then(|| project.clone());
        let request = PreviewRequest {
            project,
            revision,
            preview_scene: preview_scene.map(str::to_owned),
            program_scene: program_scene.map(str::to_owned),
            render_program,
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

fn preview_loop(
    project: &Project,
    revision: u64,
    request: &SharedRequest,
    result: &Mutex<Option<PreviewResult>>,
    applied_revision: &AtomicU64,
    queue_depth: &AtomicUsize,
    dropped_requests: &AtomicU64,
) {
    let mut renderer = PreviewRenderer::new(project, revision).map_err(|error| error.to_string());
    if renderer.is_ok() {
        applied_revision.store(revision, Ordering::Release);
    }
    while let Some(next) = wait_for_request(request, queue_depth) {
        if let (Ok(renderer), Some(project)) = (&mut renderer, next.project.as_ref()) {
            if let Err(error) = renderer.sync_project(project, next.revision) {
                renderer_error(result, dropped_requests, error.to_string());
                continue;
            }
            applied_revision.store(next.revision, Ordering::Release);
        } else if renderer.is_err() {
            let Some(project) = next.project.as_ref() else {
                continue;
            };
            renderer =
                PreviewRenderer::new(project, next.revision).map_err(|error| error.to_string());
            if renderer.is_ok() {
                applied_revision.store(next.revision, Ordering::Release);
            }
        }

        let completed = match &mut renderer {
            Ok(renderer) => render_request(renderer, next),
            Err(error) => PreviewResult {
                preview_scene: None,
                preview_frame: None,
                program_scene: None,
                program_frame: None,
                error: Some(error.clone()),
                metrics: "Preview worker unavailable".to_owned(),
                #[cfg(test)]
                render_thread: thread::current().id(),
            },
        };
        publish_result(result, dropped_requests, completed);
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

fn render_request(renderer: &mut PreviewRenderer, request: PreviewRequest) -> PreviewResult {
    let preview = render_scene(renderer, request.preview_scene.as_deref());
    let program = if !request.render_program {
        Ok(None)
    } else if request.preview_scene == request.program_scene {
        preview.clone()
    } else {
        render_scene(renderer, request.program_scene.as_deref())
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
    PreviewResult {
        preview_scene: request.preview_scene,
        preview_frame: preview.as_ref().ok().cloned().flatten(),
        program_scene: request.program_scene,
        program_frame: program.as_ref().ok().cloned().flatten(),
        error,
        metrics: renderer.metrics_summary(),
        #[cfg(test)]
        render_thread: thread::current().id(),
    }
}

fn render_scene(
    renderer: &mut PreviewRenderer,
    scene: Option<&str>,
) -> Result<Option<VideoFrame>, String> {
    let Some(scene) = scene else {
        return Ok(None);
    };
    renderer.render(scene).map_err(|error| error.to_string())
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
            program_scene: None,
            render_program: false,
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
        let worker = PreviewWorker::spawn(project.clone(), 0).expect("preview worker");
        let caller = thread::current().id();
        worker.request_render(&project, 0, Some(&scene), Some(&scene), true);

        let mut result = None;
        for _ in 0..100 {
            result = worker.try_take_latest();
            if result.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let result = result.expect("render result");
        assert_ne!(result.render_thread, caller);
        assert!(result.preview_frame.is_some());
        assert!(result.program_frame.is_some());
        assert_eq!(worker.queue_depth(), 0);
    }
}
