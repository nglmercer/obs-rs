use super::{
    layer_parameters, FrameFilter, FrameTransform, RenderError, TextureId, Timestamp, VideoFormat,
    VideoFrame, WgpuRenderBackend,
};
use wgpu::util::DeviceExt;

pub(super) fn write_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, frame: &VideoFrame) {
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        frame.pixels(),
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(frame.format().width() * 4),
            rows_per_image: Some(frame.format().height()),
        },
        wgpu::Extent3d {
            width: frame.format().width(),
            height: frame.format().height(),
            depth_or_array_layers: 1,
        },
    );
}

impl WgpuRenderBackend {
    #[allow(
        clippy::too_many_lines,
        reason = "one encoder owns both ping-pong textures and every ordered layer pass"
    )]
    pub(super) fn composite_textures<'a, I>(
        &self,
        target: TextureId,
        sources: I,
    ) -> Result<(), RenderError>
    where
        I: IntoIterator<
            Item = (
                &'a wgpu::Texture,
                VideoFormat,
                Timestamp,
                FrameTransform,
                &'a [FrameFilter],
            ),
        >,
        I::IntoIter: ExactSizeIterator,
    {
        let sources = sources.into_iter();
        if sources.len() == 0 {
            return Err(RenderError::EmptyComposition);
        }
        let source_count = sources.len();
        let target_texture = self
            .textures
            .get(&target)
            .ok_or(RenderError::UnknownTexture(target))?;
        let scratch_a = self.acquire_texture(target_texture.format);
        let scratch_b = self.acquire_texture(target_texture.format);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("obs-rs-gpu-composite"),
            });
        for (index, (source, source_format, timestamp, transform, filters)) in sources.enumerate() {
            let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
            let background = if index.is_multiple_of(2) {
                if index == 0 {
                    source
                } else {
                    &scratch_b
                }
            } else {
                &scratch_a
            };
            let background_view = background.create_view(&wgpu::TextureViewDescriptor::default());
            let destination = if index.is_multiple_of(2) {
                &scratch_a
            } else {
                &scratch_b
            };
            let destination_view = destination.create_view(&wgpu::TextureViewDescriptor::default());
            let parameters = layer_parameters(
                source_format,
                target_texture.format,
                timestamp,
                transform,
                filters,
            );
            let parameter_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("obs-rs-layer-parameters"),
                        contents: &parameters,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("obs-rs-layer"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&background_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: parameter_buffer.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("obs-rs-layer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &destination_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(if index == 0 {
                &self.replace_pipeline
            } else {
                &self.composite_pipeline
            });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let completed = if source_count.is_multiple_of(2) {
            &scratch_b
        } else {
            &scratch_a
        };
        encoder.copy_texture_to_texture(
            wgpu::ImageCopyTexture {
                texture: completed,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyTexture {
                texture: &target_texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: target_texture.format.width(),
                height: target_texture.format.height(),
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        self.recycle_texture(target_texture.format, scratch_a);
        self.recycle_texture(target_texture.format, scratch_b);
        Ok(())
    }
}
