#[allow(
    clippy::wildcard_imports,
    reason = "preview implementation modules share the renderer boundary namespace"
)]
use super::*;
use obs_rs_ui::StingerSnapshot;

const SCENE_LAYER_CACHE_CAPACITY: usize = 8;

impl PreviewRenderer {
    /// Captures a scene's source layers once per timestamp and fans that
    /// immutable snapshot out to every render target that needs it.
    ///
    /// Runtime sources are stateful, so this cache intentionally lives on the
    /// preview worker and is cleared when the timestamp advances or the
    /// project/transform changes. Its fixed capacity keeps the sharing cache
    /// from becoming a hidden frame queue.
    fn scene_layers(
        &mut self,
        scene: &str,
        request: &VideoRequest,
    ) -> Result<Vec<RenderedSceneLayer>, Box<dyn Error>> {
        if let Some(cached) = self
            .scene_layer_cache
            .iter()
            .find(|cached| cached.scene == scene && cached.timestamp == request.timestamp())
        {
            return Ok(cached.layers.clone());
        }

        let layers = self.runtime.render_scene_layers(scene, request)?;
        if self.scene_layer_cache.len() >= SCENE_LAYER_CACHE_CAPACITY {
            self.scene_layer_cache.pop_front();
        }
        self.scene_layer_cache.push_back(CachedSceneLayers {
            scene: scene.to_owned(),
            timestamp: request.timestamp(),
            layers: layers.clone(),
        });
        Ok(layers)
    }

    pub(crate) fn is_static_scene(project: &Project, scene: &str) -> bool {
        project
            .active_profile_spec()
            .is_some_and(|profile| static_scenes(profile).contains(scene))
    }

    /// Returns the bounded viewport format used for GUI preview rendering.
    ///
    /// The preview is deliberately independent from the profile's program
    /// format: a 4K canvas should not force a 4K CPU readback for a roughly
    /// 1,050-pixel-wide window.
    #[must_use]
    pub(crate) fn preview_format_for_canvas(canvas: VideoFormat) -> VideoFormat {
        const MAX_WIDTH: u64 = 1_050;
        const MAX_HEIGHT: u64 = 590;
        let canvas_width = u64::from(canvas.width());
        let canvas_height = u64::from(canvas.height());
        let (width, height) =
            if canvas_width.saturating_mul(MAX_HEIGHT) <= canvas_height.saturating_mul(MAX_WIDTH) {
                (
                    canvas_width.saturating_mul(MAX_HEIGHT) / canvas_height,
                    MAX_HEIGHT,
                )
            } else {
                (
                    MAX_WIDTH,
                    canvas_height.saturating_mul(MAX_WIDTH) / canvas_width,
                )
            };
        let width = width.max(1).min(canvas_width);
        let height = height.max(1).min(canvas_height);
        VideoFormat::new(
            u32::try_from(width).unwrap_or(u32::MAX),
            u32::try_from(height).unwrap_or(u32::MAX),
            canvas.frame_rate(),
        )
        .expect("bounded preview dimensions are valid")
    }

    /// Returns the bounded thumbnail format used by the multiview compositor.
    /// One tile is deliberately much smaller than the interactive preview so a
    /// collection of scenes cannot turn the GUI into a full-resolution fan-out.
    #[must_use]
    pub(crate) fn multiview_tile_format(canvas: VideoFormat) -> VideoFormat {
        const MAX_TILE_WIDTH: u64 = 256;
        const MAX_TILE_HEIGHT: u64 = 256;
        let canvas_width = u64::from(canvas.width()).max(1);
        let canvas_height = u64::from(canvas.height()).max(1);
        let width = u64::from(canvas.width()).clamp(1, MAX_TILE_WIDTH);
        let height = canvas_height
            .saturating_mul(width)
            .checked_div(canvas_width)
            .unwrap_or(1)
            .clamp(1, MAX_TILE_HEIGHT);
        VideoFormat::new(
            u32::try_from(width).unwrap_or(u32::MAX),
            u32::try_from(height).unwrap_or(u32::MAX),
            canvas.frame_rate(),
        )
        .expect("bounded multiview tile dimensions are valid")
    }

    /// Returns the composite dimensions for a bounded scene count.
    #[must_use]
    pub(crate) fn multiview_format_for_canvas(
        canvas: VideoFormat,
        scene_count: usize,
    ) -> VideoFormat {
        let tile = Self::multiview_tile_format(canvas);
        let (columns, rows) = crate::preview_worker::multiview_grid_dimensions(scene_count);
        VideoFormat::new(
            tile.width()
                .saturating_mul(u32::try_from(columns).unwrap_or(u32::MAX)),
            tile.height()
                .saturating_mul(u32::try_from(rows).unwrap_or(u32::MAX)),
            canvas.frame_rate(),
        )
        .expect("bounded multiview composite dimensions are valid")
    }

    /// Returns the timestamp shared by all targets in the current render tick.
    #[must_use]
    pub(crate) const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    pub(super) fn invalidate_static_scene_cache(&mut self, scene: &str) {
        self.static_frames.remove(scene);
        self.static_preview_frames
            .retain(|(cached_scene, _), _| cached_scene != scene);
        self.scene_layer_cache
            .retain(|cached| cached.scene != scene);
        self.gpu_program_scene = None;
    }

    #[allow(
        dead_code,
        reason = "kept as the single-frame program render API for diagnostics"
    )]
    pub(crate) fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let frame = self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
        )?;
        self.advance_timestamp();
        Ok(frame)
    }

    /// Renders the current scene into the viewport-sized preview target.
    /// The caller can render a matching program target at the same timestamp
    /// and advance the clock once after the complete request.
    pub(crate) fn render_preview(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(scene, RenderTarget::new(RenderTargetRole::Preview, format))
    }

    pub(crate) fn render_program(
        &mut self,
        scene: &str,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
        )
    }

    /// Renders the program feed at the bounded size used by the desktop
    /// program view. The full [`RenderTargetRole::Program`] target remains
    /// reserved for output and encoder consumers.
    pub(crate) fn render_program_preview(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::ProgramPreview, format),
        )
    }

    pub(crate) fn render_multiview_tile(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::MultiviewTile, format),
        )
    }

    /// Renders one selected source item without applying its scene-item
    /// transform or compositing the rest of the scene.
    ///
    /// A source projector follows the source's own output and source-level
    /// filters. Its target is separate from the preview/program targets so a
    /// projector cannot overwrite a GUI feed or force a full-canvas readback.
    pub(crate) fn render_source(
        &mut self,
        scene: &str,
        item: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let target = RenderTarget::new(RenderTargetRole::Projector, format);
        if matches!(self.compositor, PreviewCompositor::Wgpu(_)) {
            if !self.submit_source_layer(scene, item, target)? {
                return Ok(None);
            }
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(None);
            };
            let texture = compositor.target(target)?;
            return compositor.readback_async(texture);
        }

        let request = VideoRequest::new(self.timestamp, self.format);
        let layers = self.scene_layers(scene, &request)?;
        let Some(layer) = layers.iter().find(|layer| layer.item_id() == item) else {
            return Ok(None);
        };
        let mut frame = layer.frame().clone();
        frame.apply_filters(layer.filters());
        if frame.format() != target.format() {
            frame = self.scale_frame(&frame, target.format())?;
        }
        Ok(Some(frame))
    }

    /// Renders one complete scene for a scene projector without opening a
    /// second runtime or changing the scene's persisted geometry.
    pub(crate) fn render_scene_projector(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::Projector, format),
        )
    }

    /// Renders one transition into the full program target.
    pub(crate) fn render_transition(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_transition_target(
            source_scene,
            destination_scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
            transition,
        )
    }

    /// Renders one transition into a bounded GUI program target.
    pub(crate) fn render_transition_preview(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        format: VideoFormat,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_transition_target(
            source_scene,
            destination_scene,
            RenderTarget::new(RenderTargetRole::ProgramPreview, format),
            transition,
        )
    }

    /// Renders a preloaded Stinger clip over the shared source/destination
    /// scene pair without opening a second media runtime.
    pub(crate) fn render_stinger_transition(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        stinger: &StingerSnapshot,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_stinger_transition_target(
            source_scene,
            destination_scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
            stinger,
        )
    }

    /// Renders a preloaded Stinger clip into the bounded GUI program target.
    pub(crate) fn render_stinger_transition_preview(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        format: VideoFormat,
        stinger: &StingerSnapshot,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_stinger_transition_target(
            source_scene,
            destination_scene,
            RenderTarget::new(RenderTargetRole::ProgramPreview, format),
            stinger,
        )
    }

    fn render_stinger_transition_target(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        target: RenderTarget,
        stinger: &StingerSnapshot,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let source = self.render_target(source_scene, target)?;
        let destination = self.render_target(destination_scene, target)?;
        if self.deferred_readback() && (source.is_none() || destination.is_none()) {
            return Ok(None);
        }
        match (source, destination) {
            (None, None) => Ok(None),
            (source, destination) => {
                let source = source.unwrap_or_else(|| {
                    VideoFrame::solid(target.format(), self.timestamp, [0, 0, 0, 0])
                });
                let destination = destination.unwrap_or_else(|| {
                    VideoFrame::solid(target.format(), self.timestamp, [0, 0, 0, 0])
                });
                let overlay = stinger
                    .clip()
                    .frame_at_progress(stinger.progress_milli(), destination.timestamp())?;
                let overlay = if overlay.format() == target.format() {
                    overlay
                } else {
                    self.scale_frame(&overlay, target.format())?
                };
                stinger
                    .clip()
                    .render_with_overlay(&source, destination, &overlay, stinger.progress_milli())
                    .map(Some)
                    .map_err(Into::into)
            }
        }
    }

    fn render_transition_target(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        target: RenderTarget,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let source = self.render_target(source_scene, target)?;
        let destination = self.render_target(destination_scene, target)?;
        if self.deferred_readback() && (source.is_none() || destination.is_none()) {
            return Ok(None);
        }
        match (source, destination) {
            (None, None) => Ok(None),
            (source, destination) => {
                let source = source.unwrap_or_else(|| {
                    VideoFrame::solid(target.format(), self.timestamp, [0, 0, 0, 0])
                });
                let destination = destination.unwrap_or_else(|| {
                    VideoFrame::solid(target.format(), self.timestamp, [0, 0, 0, 0])
                });
                VideoFrame::transitioned(&source, destination, transition)
                    .map(Some)
                    .map_err(Into::into)
            }
        }
    }

    fn render_target(
        &mut self,
        scene: &str,
        target: RenderTarget,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let cached = match target.role() {
            RenderTargetRole::Program => self
                .static_frames
                .get(scene)
                .map(|pixels| (self.format, Arc::clone(pixels))),
            RenderTargetRole::Preview
            | RenderTargetRole::ProgramPreview
            | RenderTargetRole::MultiviewTile => self
                .static_preview_frames
                .get(&(scene.to_owned(), target.format()))
                .map(|pixels| (target.format(), Arc::clone(pixels))),
            RenderTargetRole::Projector | RenderTargetRole::Encoder => None,
        };
        let frame = if let Some((cached_format, pixels)) = cached.as_ref() {
            Some(VideoFrame::from_shared(
                *cached_format,
                self.timestamp,
                Arc::clone(pixels),
            )?)
        } else {
            self.render_live_scene(scene, target)?
        };
        let frame = match frame {
            Some(frame) if frame.format() != target.format() => {
                self.scale_frame(&frame, target.format())?
            }
            Some(frame) => frame,
            None => return Ok(None),
        };
        if self.static_scenes.contains(scene) && cached.is_none() {
            match target.role() {
                RenderTargetRole::Program => {
                    self.static_frames
                        .insert(scene.to_owned(), Arc::new(frame.pixels().to_vec()));
                }
                RenderTargetRole::Preview
                | RenderTargetRole::ProgramPreview
                | RenderTargetRole::MultiviewTile => {
                    self.static_preview_frames.insert(
                        (scene.to_owned(), target.format()),
                        Arc::new(frame.pixels().to_vec()),
                    );
                }
                RenderTargetRole::Projector | RenderTargetRole::Encoder => {}
            }
        }
        Ok(Some(frame))
    }

    fn render_live_scene(
        &mut self,
        scene: &str,
        target: RenderTarget,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        if matches!(self.compositor, PreviewCompositor::Wgpu(_)) {
            if !self.submit_live_scene(scene, target)? {
                return Ok(None);
            }
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(None);
            };
            let texture = compositor.target(target)?;
            return compositor.readback_async(texture);
        }
        if target.role() == RenderTargetRole::Program {
            self.gpu_program_scene = None;
        }
        let request = VideoRequest::new(self.timestamp, self.format);
        self.runtime
            .render_scene(scene, &request)
            .map_err(Into::into)
    }

    /// Submits one scene to a WGPU target without reading its pixels back.
    ///
    /// Keeping submission separate lets the encoder consume the full program
    /// texture as NV12 while the GUI consumes a different, viewport-sized
    /// target. A `false` result means the scene has no visible layers.
    fn submit_live_scene(
        &mut self,
        scene: &str,
        target: RenderTarget,
    ) -> Result<bool, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let layers = self.scene_layers(scene, &request)?;
        if layers.is_empty() {
            return Ok(false);
        }
        let submitted = layers
            .iter()
            .map(|layer| SceneLayer::frame(layer.frame(), layer.transform(), layer.filters()))
            .collect::<Vec<_>>();
        let submit_result = {
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(false);
            };
            let texture = compositor.target(target)?;
            compositor.backend.submit_layers(texture, &submitted)
        };
        if let Err(error) = submit_result {
            self.compositor = PreviewCompositor::Cpu {
                reason: Some(format!("GPU composition failed: {error}")),
            };
            self.gpu_program_scene = None;
            return Err(error.into());
        }
        if target.role() == RenderTargetRole::Program {
            self.gpu_program_scene = Some(scene.to_owned());
        }
        Ok(true)
    }

    /// Submits one selected source item to a projector target without opening
    /// another runtime or applying scene-item geometry.
    fn submit_source_layer(
        &mut self,
        scene: &str,
        item: &str,
        target: RenderTarget,
    ) -> Result<bool, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let layers = self.scene_layers(scene, &request)?;
        let Some(layer) = layers.iter().find(|layer| layer.item_id() == item) else {
            return Ok(false);
        };
        let submitted = SceneLayer::frame(layer.frame(), FrameTransform::IDENTITY, layer.filters());
        let submit_result = {
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(false);
            };
            let texture = compositor.target(target)?;
            compositor
                .backend
                .submit_layers(texture, std::slice::from_ref(&submitted))
        };
        if let Err(error) = submit_result {
            self.compositor = PreviewCompositor::Cpu {
                reason: Some(format!("GPU composition failed: {error}")),
            };
            self.gpu_program_scene = None;
            return Err(error.into());
        }
        Ok(true)
    }

    fn scale_frame(
        &mut self,
        frame: &VideoFrame,
        target: VideoFormat,
    ) -> Result<VideoFrame, Box<dyn Error>> {
        let scaler = self
            .preview_scaler
            .get_or_insert_with(|| FrameScaler::new(frame.format(), target, ScaleFilter::Bilinear));
        scaler.reconfigure(frame.format(), target, ScaleFilter::Bilinear);
        scaler.scale(frame).map_err(Into::into)
    }

    /// Produces an encoder-oriented NV12 frame from the full program target.
    /// The target is submitted directly for each live scene tick, then the
    /// bounded encoder bridge presents the newest completed conversion. A
    /// missing completion is a dropped/stale output tick, never a GPU wait.
    pub(crate) fn encoder_frame(
        &mut self,
        scene: &str,
    ) -> Result<Option<RawVideoFrame>, Box<dyn Error>> {
        if !matches!(self.compositor, PreviewCompositor::Wgpu(_)) {
            return Ok(None);
        }
        let needs_submission =
            !self.static_scenes.contains(scene) || self.gpu_program_scene.as_deref() != Some(scene);
        if needs_submission
            && !self.submit_live_scene(
                scene,
                RenderTarget::new(RenderTargetRole::Program, self.format),
            )?
        {
            return Ok(None);
        }
        let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
            return Ok(None);
        };
        let target =
            compositor.target(RenderTarget::new(RenderTargetRole::Program, self.format))?;
        compositor.readback_nv12_async(target)
    }

    pub(crate) fn deferred_readback(&self) -> bool {
        self.compositor.deferred_readback()
    }

    pub(crate) fn poll_deferred_readbacks(&mut self) -> Result<(), Box<dyn Error>> {
        let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
            return Ok(());
        };
        compositor.poll_async_readbacks()
    }

    pub(crate) fn take_deferred_frame(
        &mut self,
        role: RenderTargetRole,
        format: VideoFormat,
    ) -> Option<VideoFrame> {
        let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
            return None;
        };
        let target = RenderTarget::new(role, format);
        let texture = compositor.existing_target(target)?;
        compositor.take_async_readback(texture)
    }

    pub(crate) fn advance_timestamp(&mut self) {
        let period = self
            .format
            .frame_rate()
            .period_nanos()
            .unwrap_or(33_333_333);
        self.timestamp = self
            .timestamp
            .checked_add(period)
            .unwrap_or(Timestamp::ZERO);
        self.scene_layer_cache.clear();
    }

    pub(crate) fn metrics_summary(&self, preview_format: VideoFormat) -> String {
        let metrics = self.runtime.compositor_metrics();
        let capture = metrics.capture_latency();
        let (backend, presenter, adapter, output_conversion, gpu_readbacks, gpu_to_cpu, cpu_to_gpu) =
            match &self.compositor {
                PreviewCompositor::Wgpu(compositor) => {
                    let backend = &compositor.backend;
                    let render = backend.metrics();
                    let nv12 = backend.nv12_metrics();
                    (
                        format!(
                            "WGPU uploads={} compositions={} conversions={} gpu={} MiB NV12 conversions={} readbacks={} bytes={} staging_waits={} drops={}",
                            render.uploads(),
                            render.compositions(),
                            render.color_conversions(),
                            backend.estimated_gpu_bytes() / (1024 * 1024),
                            nv12.conversions(),
                            nv12.readbacks(),
                            nv12.bytes_transferred(),
                            nv12.staging_waits(),
                            nv12.frames_dropped(),
                        ),
                        "Slint RGBA compatibility bridge".to_owned(),
                        backend.adapter_capabilities().name().to_owned(),
                        "GPU NV12 compatibility conversion".to_owned(),
                        render.readbacks(),
                        render.readback_bytes(),
                        render.uploaded_bytes(),
                    )
                }
                PreviewCompositor::Cpu { reason } => (
                    reason.as_ref().map_or_else(
                        || "CPU fallback".to_owned(),
                        |reason| format!("CPU fallback ({reason})"),
                    ),
                    "Slint RGBA CPU presenter".to_owned(),
                    "unavailable".to_owned(),
                    "CPU fallback".to_owned(),
                    0,
                    0,
                    0,
                ),
            };
        format!(
            "Video: {}x{}@{} · Preview: {}x{} · Renderer: {backend} · Presenter: {presenter} · Adapter: {adapter} · Render: {} calls · GPU readbacks: {} ({:.1} MiB) · GPU→CPU: {:.1} MiB · CPU→GPU: {:.1} MiB · Output: {output_conversion} · source requests={} · frames={} · empty={} · failed={} · contract={} · transforms={} · filters={} · blends={} · capture p50/p95/p99/max={}/{}/{}/{} µs",
            self.format.width(),
            self.format.height(),
            rate_label(self.format),
            preview_format.width(),
            preview_format.height(),
            metrics.render_calls(),
            gpu_readbacks,
            bytes_to_mib(gpu_to_cpu),
            bytes_to_mib(gpu_to_cpu),
            bytes_to_mib(cpu_to_gpu),
            metrics.source_requests(),
            metrics.source_frames(),
            metrics.empty_sources(),
            metrics.failed_sources(),
            metrics.contract_violations(),
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

fn rate_label(format: VideoFormat) -> String {
    if format.frame_rate().denominator() == 1 {
        format.frame_rate().numerator().to_string()
    } else {
        format!(
            "{}/{}",
            format.frame_rate().numerator(),
            format.frame_rate().denominator()
        )
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "diagnostics intentionally present bounded byte totals as MiB"
)]
fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
