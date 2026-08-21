//! Optional `wgpu` backend isolated from the portable render contracts.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

/// Build/runtime availability reported without attempting to create a device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WgpuBuildCapabilities {
    compiled: bool,
}

impl WgpuBuildCapabilities {
    #[must_use]
    pub const fn compiled(self) -> bool {
        self.compiled
    }
}

/// Reports whether the optional GPU implementation was compiled.
#[must_use]
pub const fn build_capabilities() -> WgpuBuildCapabilities {
    WgpuBuildCapabilities {
        compiled: cfg!(feature = "gpu"),
    }
}

#[cfg(feature = "gpu")]
mod gpu;

#[cfg(feature = "gpu")]
pub use gpu::{WgpuAdapterCapabilities, WgpuBackendError, WgpuRenderBackend};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_build_state_is_explicit() {
        assert_eq!(build_capabilities().compiled(), cfg!(feature = "gpu"));
    }

    #[cfg(feature = "gpu")]
    #[allow(
        clippy::too_many_lines,
        reason = "the GPU integration fixture keeps CPU-oracle, viewport, rotation, and recovery assertions together"
    )]
    #[test]
    fn gpu_upload_layer_submission_readback_and_recovery_are_explicit() {
        use obs_rs_media::{
            FrameFilter, FrameRate, FrameTransform, Timestamp, VideoFormat, VideoFrame,
        };
        use obs_rs_render::{CpuRenderBackend, RenderBackend, RenderError, SceneLayer};

        let Ok(mut backend) = WgpuRenderBackend::new(8, 16 * 1_024 * 1_024) else {
            return;
        };
        let format = VideoFormat::new(8, 8, FrameRate::new(30, 1).expect("rate")).expect("format");
        let pixels = (0_u8..64)
            .flat_map(|value| [value, value.wrapping_add(20), 200, 255])
            .collect();
        let frame = VideoFrame::new(format, Timestamp::from_millis(7), pixels).expect("frame");
        let target = backend.create_texture(format).expect("target");
        let filters = [
            FrameFilter::Grayscale,
            FrameFilter::Brightness { milli: -250 },
            FrameFilter::Opacity(200),
            FrameFilter::CropPad {
                left: 1,
                top: 1,
                right: 1,
                bottom: 1,
            },
        ];
        let transform = FrameTransform::new(1_500, 750, -1, 2, true, false, 210)
            .expect("transform")
            .with_crop(1, 1, 2, 1)
            .expect("crop");
        backend
            .submit_layers(target, &[SceneLayer::frame(&frame, transform, &filters)])
            .expect("GPU layer submission");
        assert_eq!(backend.metrics().readbacks(), 0);
        assert_eq!(backend.metrics().compositions(), 1);
        let mut expected = frame.transformed(transform).expect("CPU transform oracle");
        expected.apply_filters(&filters);
        assert_eq!(
            backend.readback(target).expect("explicit readback"),
            expected
        );
        assert_eq!(backend.metrics().readbacks(), 1);

        let rotated_transform = FrameTransform::IDENTITY
            .with_rotation_degrees(90)
            .expect("rotation");
        backend
            .submit_layers(target, &[SceneLayer::frame(&frame, rotated_transform, &[])])
            .expect("GPU rotation");
        let rotated_expected = frame
            .transformed(rotated_transform)
            .expect("CPU rotation oracle");
        assert_eq!(
            backend.readback(target).expect("rotated readback"),
            rotated_expected
        );

        // A desktop preview is allowed to be smaller than the program
        // canvas. The compositor must scale the canvas-space layer into the
        // target without first producing a full-size CPU frame.
        let preview_format = VideoFormat::new(4, 4, format.frame_rate()).expect("preview format");
        let preview_target = backend
            .create_texture(preview_format)
            .expect("preview target");
        let solid = VideoFrame::solid(format, Timestamp::ZERO, [32, 96, 160, 255]);
        backend
            .submit_layers(
                preview_target,
                &[SceneLayer::frame(&solid, FrameTransform::IDENTITY, &[])],
            )
            .expect("viewport-sized GPU composition");
        let preview = backend.readback(preview_target).expect("preview readback");
        assert_eq!(preview.format(), preview_format);
        assert!(preview
            .pixels()
            .chunks_exact(4)
            .all(|pixel| pixel == [32, 96, 160, 255]));

        let background_pixels = (0_u8..64)
            .flat_map(|value| [10, 200, 30, value.saturating_mul(4)])
            .collect();
        let foreground_pixels = (0_u8..64)
            .flat_map(|value| [250, 20, 90, 255_u8.saturating_sub(value.saturating_mul(4))])
            .collect();
        let background =
            VideoFrame::new(format, Timestamp::ZERO, background_pixels).expect("background");
        let foreground =
            VideoFrame::new(format, Timestamp::ZERO, foreground_pixels).expect("foreground");
        let layers = [
            SceneLayer::frame(&background, FrameTransform::IDENTITY, &[]),
            SceneLayer::frame(&foreground, FrameTransform::IDENTITY, &[]),
        ];
        let mut cpu = CpuRenderBackend::new(1).expect("CPU oracle");
        let cpu_target = cpu.create_texture(format).expect("CPU target");
        cpu.submit_layers(cpu_target, &layers)
            .expect("CPU composition");
        let expected = cpu.readback(cpu_target).expect("CPU result");
        backend
            .submit_layers(target, &layers)
            .expect("GPU alpha composition");
        assert_eq!(backend.readback(target).expect("GPU result"), expected);

        backend
            .upload(
                target,
                &VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255]),
            )
            .expect("black frame");
        let nv12 = backend.readback_nv12(target).expect("GPU NV12 conversion");
        assert_eq!(nv12.pixel_format(), obs_rs_media::PixelFormat::Nv12);
        assert!(nv12.bytes()[..64].iter().all(|value| *value == 16));
        assert!(nv12.bytes()[64..].iter().all(|value| *value == 128));
        assert_eq!(backend.metrics().color_conversions(), 1);

        backend.lose_device();
        assert_eq!(backend.state(), obs_rs_render::RenderState::Lost);
        backend.recover().expect("device recovery");
        assert!(matches!(
            backend.readback(target),
            Err(RenderError::TextureNotReady(id)) if id == target
        ));
        assert_eq!(backend.metrics().context_losses(), 1);
        assert_eq!(backend.metrics().recoveries(), 1);
    }

    #[cfg(feature = "gpu")]
    #[test]
    #[ignore = "10,000-frame reference-hardware acceptance run"]
    fn gpu_1080p60_acceptance_has_no_implicit_readback() {
        use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
        use obs_rs_render::RenderBackend;
        use std::time::Instant;

        let Ok(mut backend) = WgpuRenderBackend::new(4, 64 * 1_024 * 1_024) else {
            return;
        };
        let format =
            VideoFormat::new(1_920, 1_080, FrameRate::new(60, 1).expect("rate")).expect("format");
        let source = backend.create_texture(format).expect("source");
        let target = backend.create_texture(format).expect("target");
        backend
            .upload(
                source,
                &VideoFrame::solid(format, Timestamp::ZERO, [24, 96, 180, 255]),
            )
            .expect("upload");
        let mut samples = Vec::with_capacity(10_000);
        let mut dropped = 0_u64;
        for _ in 0..10_000 {
            let started = Instant::now();
            if backend.composite(target, &[source]).is_err() {
                dropped = dropped.saturating_add(1);
            }
            backend.wait_idle();
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let p95 = samples[samples.len() * 95 / 100];
        println!(
            "gpu_frames=10000 p95_ns={p95} dropped={dropped} readbacks={} gpu_bytes={} pooled_textures={}",
            backend.metrics().readbacks(),
            backend.estimated_gpu_bytes(),
            backend.pooled_texture_count(),
        );
        assert!(p95 < 16_700_000, "p95 was {p95}ns");
        assert!(dropped <= 100, "dropped {dropped} frames");
        assert_eq!(backend.metrics().readbacks(), 0);
    }
}
