use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_core::Runtime;
use obs_rs_media::{FrameTransition, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::Project;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub(crate) struct PreviewRenderer {
    pub(crate) format: VideoFormat,
    pub(crate) runtime: Runtime,
    timestamp: Timestamp,
    project_document: String,
}

impl PreviewRenderer {
    pub(crate) fn new(project: &Project) -> Result<Self, Box<dyn Error>> {
        let active_profile = project.active_profile();
        let profile = project
            .profiles()
            .find(|profile| profile.id() == active_profile)
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        let format = profile.video_format();
        let mut runtime = Runtime::new();
        let plugin = BuiltinPlugin::new()?;
        runtime.register_plugin(&plugin)?;

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
                    runtime.add_source_filter(scene_id, source_id, *filter)?;
                }
            }
        }

        Ok(Self {
            format,
            runtime,
            timestamp: Timestamp::ZERO,
            project_document: project.serialize(),
        })
    }

    pub(crate) fn sync_project(&mut self, project: &Project) -> Result<(), Box<dyn Error>> {
        let document = project.serialize();
        if document != self.project_document {
            *self = Self::new(project)?;
        }
        Ok(())
    }

    pub(crate) fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let frame = self.runtime.render_scene(scene, &request)?;
        self.advance_timestamp();
        Ok(frame)
    }

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
        format!(
            "Preview work: renders={} · source requests={} · frames={} · empty={} · transforms={} · filters={} · blends={}",
            metrics.render_calls(),
            metrics.source_requests(),
            metrics.source_frames(),
            metrics.empty_sources(),
            metrics.transformed_frames(),
            metrics.filtered_frames(),
            metrics.blended_layers()
        )
    }
}

pub(crate) fn scene_image(
    renderer: &Rc<RefCell<PreviewRenderer>>,
    scene: Option<&str>,
) -> (Image, Option<String>) {
    let Some(scene) = scene else {
        return (Image::default(), None);
    };
    match renderer.borrow_mut().render(scene) {
        Ok(Some(frame)) => (frame_to_image(&frame), None),
        Ok(None) => (
            Image::default(),
            Some(format!("Scene {scene} has no frame")),
        ),
        Err(error) => (Image::default(), Some(format!("Preview renderer: {error}"))),
    }
}

pub(crate) fn frame_to_image(frame: &VideoFrame) -> Image {
    let format = frame.format();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(format.width(), format.height());
    for (pixel, channels) in buffer
        .make_mut_slice()
        .iter_mut()
        .zip(frame.pixels().chunks_exact(4))
    {
        *pixel = Rgba8Pixel::new(channels[0], channels[1], channels[2], channels[3]);
    }
    Image::from_rgba8(buffer)
}
