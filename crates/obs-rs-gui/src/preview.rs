use std::{
    collections::{HashMap, HashSet},
    error::Error,
    rc::Rc,
};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_core::Runtime;
use obs_rs_engine::compile_filter;
#[cfg(test)]
use obs_rs_media::FrameTransition;
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::Project;
use obs_rs_render::{
    GpuFrameHandle, GpuPlaneHandle, RenderBackend, SceneLayer, TextureId, VideoSurface,
};
use obs_rs_render_wgpu::WgpuRenderBackend;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub(crate) struct PreviewRenderer {
    pub(crate) format: VideoFormat,
    pub(crate) runtime: Runtime,
    timestamp: Timestamp,
    /// Project revision this renderer was built from.
    ///
    /// Change detection compares this integer against the session's current
    /// revision. It used to serialize the whole project on every frame and
    /// compare the resulting strings.
    revision: u64,
    /// Scenes made exclusively from solid color sources do not change between
    /// frames. Their first composed pixel buffer is reused while fresh frame
    /// timestamps are issued to the output encoder.
    static_scenes: HashSet<String>,
    static_frames: HashMap<String, Vec<u8>>,
    compositor: PreviewCompositor,
    gpu_scene: Option<String>,
}

enum PreviewCompositor {
    Wgpu {
        backend: Box<WgpuRenderBackend>,
        target: TextureId,
    },
    Cpu {
        reason: Option<String>,
    },
}

impl PreviewCompositor {
    fn new(format: VideoFormat) -> Self {
        let texture_budget = format.rgba_bytes().saturating_mul(12);
        match WgpuRenderBackend::new(12, texture_budget) {
            Ok(mut backend) => match backend.create_texture(format) {
                Ok(target) => Self::Wgpu {
                    backend: Box::new(backend),
                    target,
                },
                Err(error) => Self::Cpu {
                    reason: Some(format!("target allocation failed: {error}")),
                },
            },
            Err(error) => Self::Cpu {
                reason: Some(error.to_string()),
            },
        }
    }
}

thread_local! {
    /// The builtin plugin, constructed once per thread.
    ///
    /// Rebuilding the renderer used to recreate the plugin and all of its
    /// factory objects; the plugin is immutable, so one instance is shared.
    static BUILTIN_PLUGIN: Rc<BuiltinPlugin> = Rc::new(
        BuiltinPlugin::new().unwrap_or_else(|error| {
            unreachable!("builtin plugin manifest is valid: {error}")
        }),
    );
}

impl PreviewRenderer {
    pub(crate) fn new(project: &Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        let format = profile.video_format();
        let mut runtime = Runtime::new();
        let plugin = BUILTIN_PLUGIN.with(Rc::clone);
        runtime.register_plugin(plugin.as_ref())?;

        for scene in profile.scenes() {
            let scene_id = scene.id().as_str();
            runtime.create_scene(scene_id)?;
            for source in scene.sources() {
                if !source.visible() {
                    continue;
                }
                let source_id = runtime.create_source(
                    source.kind().as_str(),
                    source.name(),
                    source.settings(),
                )?;
                runtime.attach_source(scene_id, source_id)?;
                runtime.set_source_transform(scene_id, source_id, source.transform())?;
                for filter in source.filters() {
                    if let Some(runtime_filter) = compile_filter(filter) {
                        runtime.add_source_filter(scene_id, source_id, runtime_filter)?;
                    }
                }
            }
        }

        let static_scenes = profile
            .scenes()
            .filter(|scene| {
                scene
                    .sources()
                    .iter()
                    .any(obs_rs_project::SourceSpec::visible)
                    && scene
                        .sources()
                        .iter()
                        .filter(|source| source.visible())
                        .all(|source| source.kind().as_str() == "color_source")
            })
            .map(|scene| scene.id().as_str().to_owned())
            .collect();

        Ok(Self {
            format,
            runtime,
            timestamp: Timestamp::ZERO,
            revision,
            static_scenes,
            static_frames: HashMap::new(),
            compositor: PreviewCompositor::new(format),
            gpu_scene: None,
        })
    }

    /// Rebuilds the runtime when the project has changed since the last sync.
    ///
    /// Returns whether a rebuild happened, so the caller can skip the UI work
    /// that depends on project content.
    pub(crate) fn sync_project(
        &mut self,
        project: &Project,
        revision: u64,
    ) -> Result<bool, Box<dyn Error>> {
        if revision == self.revision {
            return Ok(false);
        }
        *self = Self::new(project, revision)?;
        Ok(true)
    }

    pub(crate) const fn is_synced(&self, revision: u64) -> bool {
        self.revision == revision
    }

    pub(crate) fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let frame = if let Some(pixels) = self.static_frames.get(scene) {
            self.gpu_scene = None;
            Some(VideoFrame::new(
                self.format,
                self.timestamp,
                pixels.clone(),
            )?)
        } else {
            let frame = self.render_live_scene(scene)?;
            if self.static_scenes.contains(scene) {
                if let Some(frame) = frame.as_ref() {
                    self.static_frames
                        .insert(scene.to_owned(), frame.pixels().to_vec());
                }
            }
            frame
        };
        self.advance_timestamp();
        Ok(frame)
    }

    fn render_live_scene(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        match &mut self.compositor {
            PreviewCompositor::Wgpu { backend, target } => {
                let layers = self.runtime.render_scene_layers(scene, &request)?;
                if layers.is_empty() {
                    return Ok(None);
                }
                let submitted = layers
                    .iter()
                    .map(|layer| {
                        SceneLayer::frame(layer.frame(), layer.transform(), layer.filters())
                    })
                    .collect::<Vec<_>>();
                if let Err(error) = backend.submit_layers(*target, &submitted) {
                    self.compositor = PreviewCompositor::Cpu {
                        reason: Some(format!("GPU composition failed: {error}")),
                    };
                    return Err(error.into());
                }
                self.gpu_scene = Some(scene.to_owned());
                let timestamp = layers
                    .last()
                    .map_or(self.timestamp, |layer| layer.frame().timestamp());
                let handle = GpuFrameHandle::new(
                    "obs-rs-wgpu",
                    self.format,
                    obs_rs_media::PixelFormat::Rgba8,
                    timestamp,
                    vec![GpuPlaneHandle::new(
                        *target,
                        self.format.width(),
                        self.format.height(),
                    )],
                )
                .ok_or_else(|| std::io::Error::other("invalid GPU surface descriptor"))?;
                let surface = VideoSurface::Gpu(handle);
                match surface {
                    VideoSurface::Gpu(handle) => backend
                        .readback(handle.planes()[0].texture())
                        .map(Some)
                        .map_err(Into::into),
                    VideoSurface::Cpu(frame) => Ok(Some(frame)),
                }
            }
            PreviewCompositor::Cpu { .. } => {
                self.gpu_scene = None;
                self.runtime
                    .render_scene(scene, &request)
                    .map_err(Into::into)
            }
        }
    }

    /// Produces an encoder-oriented NV12 frame from the current GPU target.
    /// Static/cached frames are uploaded once when the target no longer names
    /// this scene; live GPU composition proceeds directly to conversion.
    pub(crate) fn encoder_frame(
        &mut self,
        scene: &str,
        frame: &VideoFrame,
    ) -> Result<Option<RawVideoFrame>, Box<dyn Error>> {
        let PreviewCompositor::Wgpu { backend, target } = &mut self.compositor else {
            return Ok(None);
        };
        if self.gpu_scene.as_deref() != Some(scene) {
            backend.upload(*target, frame)?;
            self.gpu_scene = Some(scene.to_owned());
        }
        backend.readback_nv12(*target).map(Some).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn render_transition(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let frame = self.runtime.render_scene_transition(
            source_scene,
            destination_scene,
            &request,
            transition,
        )?;
        self.advance_timestamp();
        Ok(frame)
    }

    fn advance_timestamp(&mut self) {
        let period = self
            .format
            .frame_rate()
            .period_nanos()
            .unwrap_or(33_333_333);
        self.timestamp = self
            .timestamp
            .checked_add(period)
            .unwrap_or(Timestamp::ZERO);
    }

    pub(crate) fn metrics_summary(&self) -> String {
        let metrics = self.runtime.compositor_metrics();
        let capture = metrics.capture_latency();
        let backend = match &self.compositor {
            PreviewCompositor::Wgpu { backend, .. } => {
                let render = backend.metrics();
                format!(
                    "WGPU uploads={} compositions={} conversions={} readbacks={} gpu={} MiB",
                    render.uploads(),
                    render.compositions(),
                    render.color_conversions(),
                    render.readbacks(),
                    backend.estimated_gpu_bytes() / (1024 * 1024),
                )
            }
            PreviewCompositor::Cpu { reason } => reason.as_ref().map_or_else(
                || "CPU fallback".to_owned(),
                |reason| format!("CPU fallback ({reason})"),
            ),
        };
        format!(
            "Preview work: {backend} · renders={} · source requests={} · frames={} · empty={} · transforms={} · filters={} · blends={} · capture p50/p95/p99/max={}/{}/{}/{} µs",
            metrics.render_calls(),
            metrics.source_requests(),
            metrics.source_frames(),
            metrics.empty_sources(),
            metrics.transformed_frames(),
            metrics.filtered_frames(),
            metrics.blended_layers(),
            capture.percentile_nanos(50) / 1_000,
            capture.percentile_nanos(95) / 1_000,
            capture.percentile_nanos(99) / 1_000,
            capture.max_nanos() / 1_000,
        )
    }
}

pub(crate) fn frame_to_image(frame: &VideoFrame) -> Image {
    let format = frame.format();
    // Slint owns its pixel storage, so one copy out of the engine frame is
    // unavoidable here; `clone_from_slice` performs it as a single block copy.
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        frame.pixels(),
        format.width(),
        format.height(),
    );
    Image::from_rgba8(buffer)
}
