use std::time::Duration;

use obs_rs_config::Config;
use obs_rs_project::{ProjectCommand, SourceFilterSpec};

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
        program_transition: None,
        program_preview_format: VideoFormat::new(
            16,
            16,
            obs_rs_media::FrameRate::new(30, 1).expect("rate"),
        )
        .expect("program preview format"),
        multiview_scenes: Vec::new(),
        multiview_format: VideoFormat::new(
            256,
            144,
            obs_rs_media::FrameRate::new(30, 1).expect("rate"),
        )
        .expect("multiview format"),
        source_projector: None,
        scene_projector: None,
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
fn preview_diagnostics_report_an_unavailable_persisted_filter() {
    let mut project = crate::initial_project().expect("project");
    let profile = project.active_profile_spec().expect("profile");
    let profile_id = profile.id().as_str().to_owned();
    let source_id = profile
        .sources()
        .next()
        .expect("source")
        .id()
        .as_str()
        .to_owned();
    let filter = SourceFilterSpec::new(
        "future-preview-filter",
        "Future preview filter",
        "future_effect",
        Config::new(),
    )
    .expect("filter");
    project
        .apply(ProjectCommand::AddSourceFilter {
            profile: profile_id,
            source: source_id,
            filter,
        })
        .expect("add filter");

    let renderer = PreviewRenderer::new(&project, 0).expect("renderer");
    assert_eq!(
            renderer.diagnostics().filter_diagnostics,
            vec![
                "source 'Background' filter 'Future preview filter': filter 'future_effect' (effect) unavailable: unsupported kind"
                    .to_owned()
            ]
        );
}

#[test]
fn multiview_grid_and_tile_format_stay_bounded() {
    assert_eq!(multiview_grid_dimensions(1), (1, 1));
    assert_eq!(multiview_grid_dimensions(4), (2, 2));
    assert_eq!(multiview_grid_dimensions(5), (3, 2));
    assert_eq!(multiview_grid_dimensions(32), (4, 4));

    let canvas = VideoFormat::new(
        3_840,
        2_160,
        obs_rs_media::FrameRate::new(60, 1).expect("rate"),
    )
    .expect("canvas format");
    let tile = PreviewRenderer::multiview_tile_format(canvas);
    assert_eq!((tile.width(), tile.height()), (256, 144));
    let composite = PreviewRenderer::multiview_format_for_canvas(canvas, 16);
    assert_eq!((composite.width(), composite.height()), (1_024, 576));
}

#[test]
fn program_transition_uses_the_same_worker_renderer_for_preview_and_output() {
    let project = crate::initial_project().expect("project");
    let canvas = project
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let format = PreviewRenderer::preview_format_for_canvas(canvas);
    let snapshot = TransitionSnapshot::new(
        "preview",
        "program",
        obs_rs_media::FrameTransition::CrossFade {
            progress_milli: 500,
        },
    );
    let mut renderer = PreviewRenderer::new(&project, 0).expect("renderer");
    let preview = render_program_scene(&mut renderer, Some("program"), format, Some(&snapshot))
        .expect("preview transition")
        .expect("preview frame");
    assert_eq!(preview.pixel(0, 0), Some([0x18, 0x28, 0x38, 0xff]));

    let output = render_program_transition(&mut renderer, Some(&snapshot))
        .expect("output transition")
        .expect("output frame");
    assert_eq!(output.pixel(0, 0), Some([0x18, 0x28, 0x38, 0xff]));
}

#[test]
fn source_projector_renders_the_selected_item_without_scene_geometry() {
    let project = crate::initial_project().expect("initial project");
    let canvas = project
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let format = PreviewRenderer::preview_format_for_canvas(canvas);
    let mut renderer = PreviewRenderer::new(&project, 0).expect("renderer");
    let frame = renderer
        .render_source("preview", "background", format)
        .expect("source projector render")
        .expect("selected source frame");

    assert_eq!(frame.format(), format);
    assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
}

#[test]
fn scene_projector_renders_the_complete_scene() {
    let project = crate::initial_project().expect("initial project");
    let canvas = project
        .active_profile_spec()
        .expect("profile")
        .video_format();
    let format = PreviewRenderer::preview_format_for_canvas(canvas);
    let mut renderer = PreviewRenderer::new(&project, 0).expect("renderer");
    let frame = renderer
        .render_scene_projector("preview", format)
        .expect("scene projector render")
        .expect("scene frame");

    assert_eq!(frame.format(), format);
    assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
}

#[test]
fn multiview_render_timing_report() {
    let project = crate::initial_project().expect("project");
    let profile = project.active_profile_spec().expect("profile");
    let canvas = profile.video_format();
    let scenes = profile
        .scenes()
        .take(MAX_MULTIVIEW_SCENES)
        .map(|scene| scene.id().as_str().to_owned())
        .collect::<Vec<_>>();
    let format = PreviewRenderer::multiview_format_for_canvas(canvas, scenes.len());
    let mut renderer = PreviewRenderer::new(&project, 0).expect("renderer");
    let started = std::time::Instant::now();
    for _ in 0..20 {
        let frame = render_multiview_scene(&mut renderer, &scenes, format)
            .expect("multiview render")
            .expect("composite frame");
        assert_eq!(frame.format(), format);
        renderer.advance_timestamp();
    }
    let elapsed = started.elapsed();
    println!(
        "multiview: {} scenes x 20 renders = {:?} ({:?}/render)",
        scenes.len(),
        elapsed,
        elapsed / 20,
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
            program_transition: None,
            program_preview_format: PreviewRenderer::preview_format_for_canvas(
                project
                    .active_profile_spec()
                    .expect("profile")
                    .video_format(),
            ),
            multiview_scenes: vec![scene.clone()],
            multiview_format: PreviewRenderer::multiview_format_for_canvas(
                project
                    .active_profile_spec()
                    .expect("profile")
                    .video_format(),
                1,
            ),
            source_projector: None,
            scene_projector: None,
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
    assert_eq!(
        result.multiview_frame.expect("multiview frame").format(),
        PreviewRenderer::multiview_format_for_canvas(canvas_format, 1)
    );
    assert_eq!(worker.queue_depth(), 0);
    let performance = worker.performance();
    assert_eq!(performance.preview_render.samples(), 1);
    assert_eq!(performance.worker.samples(), 1);
}
