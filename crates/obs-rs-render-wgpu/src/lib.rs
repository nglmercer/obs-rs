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
    #[test]
    fn gpu_upload_layer_submission_readback_and_recovery_are_explicit() {
        use obs_rs_media::{
            FrameFilter, FrameRate, FrameTransform, Timestamp, VideoFormat, VideoFrame,
        };
        use obs_rs_render::{RenderBackend, RenderError, SceneLayer};

        let Ok(mut backend) = WgpuRenderBackend::new(8, 16 * 1_024 * 1_024) else {
            return;
        };
        let format = VideoFormat::new(8, 8, FrameRate::new(30, 1).expect("rate")).expect("format");
        let pixels = (0_u8..64)
            .flat_map(|value| [value, value.wrapping_add(20), 200, 255])
            .collect();
        let frame = VideoFrame::new(format, Timestamp::from_millis(7), pixels).expect("frame");
        let target = backend.create_texture(format).expect("target");
        let filters = [FrameFilter::Brightness { milli: 750 }];
        backend
            .submit_layers(
                target,
                &[SceneLayer::frame(
                    &frame,
                    FrameTransform::IDENTITY,
                    &filters,
                )],
            )
            .expect("GPU layer submission");
        assert_eq!(backend.metrics().readbacks(), 0);
        assert_eq!(backend.metrics().compositions(), 1);
        let mut expected = frame;
        expected.apply_filters(&filters);
        assert_eq!(
            backend.readback(target).expect("explicit readback"),
            expected
        );
        assert_eq!(backend.metrics().readbacks(), 1);

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
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let p95 = samples[samples.len() * 95 / 100];
        println!(
            "gpu_frames=10000 p95_ns={p95} dropped={dropped} readbacks={}",
            backend.metrics().readbacks()
        );
        assert!(p95 < 16_700_000, "p95 was {p95}ns");
        assert!(dropped <= 100, "dropped {dropped} frames");
        assert_eq!(backend.metrics().readbacks(), 0);
    }
}
