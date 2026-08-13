use super::*;
use obs_rs_media::{
    FrameFilter, FrameRate, FrameTransform, RawVideoFrame, Timestamp, VideoFormat, VideoFrame,
};

fn format() -> VideoFormat {
    VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format")
}

#[test]
fn cpu_backend_uploads_composes_and_reads_back() {
    let format = format();
    let mut backend = CpuRenderBackend::new(3).expect("backend");
    let background = backend.create_texture(format).expect("background");
    let foreground = backend.create_texture(format).expect("foreground");
    let target = backend.create_texture(format).expect("target");
    backend
        .upload(
            background,
            &VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 255, 255]),
        )
        .expect("background upload");
    backend
        .upload(
            foreground,
            &VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 128]),
        )
        .expect("foreground upload");
    backend
        .composite(target, &[background, foreground])
        .expect("composition");

    let frame = backend.readback(target).expect("readback");
    assert_eq!(frame.pixel(0, 0), Some([128, 0, 127, 255]));
    assert!(!backend.capabilities().accelerated());
    assert!(backend.capabilities().readback());
    let metrics = backend.metrics();
    assert_eq!(metrics.textures_created(), 3);
    assert_eq!(metrics.textures_destroyed(), 0);
    assert_eq!(metrics.uploads(), 2);
    assert_eq!(metrics.compositions(), 1);
    assert_eq!(metrics.readbacks(), 1);
    assert_eq!(metrics.allocated_bytes(), format.rgba_bytes() * 3);
    assert_eq!(metrics.peak_allocated_bytes(), format.rgba_bytes() * 3);
}

#[test]
fn context_loss_requires_recovery_and_invalidates_contents() {
    let format = format();
    let mut backend = CpuRenderBackend::new(1).expect("backend");
    let texture = backend.create_texture(format).expect("texture");
    backend
        .upload(
            texture,
            &VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255]),
        )
        .expect("upload");
    backend.lose_context();
    assert_eq!(backend.state(), RenderState::Lost);
    assert_eq!(backend.readback(texture), Err(RenderError::ContextLost));
    backend.recover().expect("recover");
    assert_eq!(backend.state(), RenderState::Ready);
    assert_eq!(
        backend.readback(texture),
        Err(RenderError::TextureNotReady(texture))
    );
    let metrics = backend.metrics();
    assert_eq!(metrics.context_losses(), 1);
    assert_eq!(metrics.recoveries(), 1);
    assert_eq!(metrics.readbacks(), 0);
}

#[test]
fn backend_rejects_limits_formats_and_empty_layers() {
    let format = format();
    assert!(matches!(
        CpuRenderBackend::new(0),
        Err(RenderError::ZeroCapacity)
    ));
    let mut backend = CpuRenderBackend::new(1).expect("backend");
    let texture = backend.create_texture(format).expect("texture");
    assert_eq!(
        backend.create_texture(format),
        Err(RenderError::TextureLimit { limit: 1 })
    );
    assert_eq!(
        backend.composite(texture, &[]),
        Err(RenderError::EmptyComposition)
    );
    let other = VideoFormat::new(1, 1, format.frame_rate()).expect("other format");
    assert!(matches!(
        backend.upload(
            texture,
            &VideoFrame::solid(other, Timestamp::ZERO, [0, 0, 0, 255])
        ),
        Err(RenderError::FormatMismatch { .. })
    ));
}

#[test]
fn backend_accounts_texture_bytes_and_accepts_raw_uploads() {
    let format = format();
    let mut backend = CpuRenderBackend::with_limits(2, format.rgba_bytes()).expect("backend");
    let texture = backend.create_texture(format).expect("texture");
    assert_eq!(backend.allocated_bytes(), format.rgba_bytes());
    assert_eq!(
        backend.create_texture(format),
        Err(RenderError::TextureByteLimit {
            limit: format.rgba_bytes(),
            requested: format.rgba_bytes()
        })
    );

    let raw = RawVideoFrame::new(
        format,
        obs_rs_media::PixelFormat::Bgra8,
        Timestamp::ZERO,
        vec![3, 2, 1, 255, 7, 6, 5, 255],
    )
    .expect("raw frame");
    backend.upload_raw(texture, &raw).expect("raw upload");
    assert_eq!(
        backend.readback(texture).expect("readback").pixels(),
        &[1, 2, 3, 255, 5, 6, 7, 255]
    );
    backend.destroy_texture(texture).expect("destroy");
    assert_eq!(backend.allocated_bytes(), 0);
    let metrics = backend.metrics();
    assert_eq!(metrics.textures_created(), 1);
    assert_eq!(metrics.textures_destroyed(), 1);
    assert_eq!(metrics.uploads(), 1);
    assert_eq!(metrics.readbacks(), 1);
    assert_eq!(metrics.allocated_bytes(), 0);
    assert_eq!(metrics.peak_allocated_bytes(), format.rgba_bytes());
}

#[test]
fn cpu_scene_layer_submission_is_the_pixel_oracle_for_extended_backends() {
    let format = format();
    let background = VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 255, 255]);
    let foreground = VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 128]);
    let filters = [FrameFilter::Grayscale];
    let layers = [
        SceneLayer::frame(&background, FrameTransform::IDENTITY, &[]),
        SceneLayer::frame(&foreground, FrameTransform::IDENTITY, &filters),
    ];
    let mut backend = CpuRenderBackend::new(1).expect("backend");
    let target = backend.create_texture(format).expect("target");
    backend
        .submit_layers(target, &layers)
        .expect("submit layers");
    assert_eq!(
        backend.readback(target).expect("readback").pixel(0, 0),
        Some([38, 38, 165, 255])
    );

    let surface =
        OpaqueFrameSurface::new("linux-dmabuf", 7, format, Timestamp::ZERO).expect("surface");
    assert_eq!(
        backend.surface_import_mode(surface.provider()),
        SurfaceImportMode::CpuFallback
    );
    assert_eq!(
        backend.submit_layers(
            target,
            &[SceneLayer::surface(&surface, FrameTransform::IDENTITY, &[])]
        ),
        Err(RenderError::SurfaceUnsupported {
            provider: "linux-dmabuf".to_owned()
        })
    );
}

#[test]
fn portable_gpu_surface_contains_only_backend_tokens_and_media_metadata() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut backend = CpuRenderBackend::new(2).expect("backend");
    let texture = backend.create_texture(format).expect("texture");
    let handle = GpuFrameHandle::new(
        "test-backend",
        format,
        obs_rs_media::PixelFormat::Nv12,
        Timestamp::from_millis(9),
        vec![
            GpuPlaneHandle::new(texture, format.width(), format.height()),
            GpuPlaneHandle::new(texture, format.width() / 2, format.height() / 2),
        ],
    )
    .expect("portable handle");
    let surface = VideoSurface::Gpu(handle.clone());
    assert_eq!(surface.format(), format);
    assert_eq!(surface.timestamp(), Timestamp::from_millis(9));
    assert_eq!(handle.provider(), "test-backend");
    assert_eq!(handle.planes().len(), 2);
    assert_eq!(handle.pixel_format(), obs_rs_media::PixelFormat::Nv12);
}
