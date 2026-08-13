use std::{
    collections::{HashMap, HashSet},
    error::Error,
    rc::Rc,
};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_core::Runtime;
#[cfg(test)]
use obs_rs_media::FrameTransition;
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::Project;
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
                    runtime.add_source_filter(scene_id, source_id, *filter)?;
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
            Some(VideoFrame::new(
                self.format,
                self.timestamp,
                pixels.clone(),
            )?)
        } else {
            let request = VideoRequest::new(self.timestamp, self.format);
            let frame = self.runtime.render_scene(scene, &request)?;
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
