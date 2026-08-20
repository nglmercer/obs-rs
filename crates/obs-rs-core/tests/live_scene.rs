//! Regression cover for a scene that mixes a screen capture with a camera.
//!
//! Camera-plus-screen is the ordinary streaming layout and the one that used to
//! break: the two sources were opened twice, every scene-item edit tore both of
//! them down and built them again, and whichever of them failed first took the
//! other's picture with it. These tests pin the behaviour that fixed it —
//! both layers keep arriving, a scene edit never recreates a device, and one
//! broken source leaves the rest of the scene composited.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

use obs_rs_config::Config;
use obs_rs_core::Runtime;
use obs_rs_media::{FrameRate, FrameTransform, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{
    Plugin, PluginManifest, Source, SourceError, SourceFactory, VideoRequest,
};
use obs_rs_util::Identifier;

const CANVAS: (u32, u32) = (192, 108);

fn format() -> VideoFormat {
    VideoFormat::new(CANVAS.0, CANVAS.1, FrameRate::new(30, 1).expect("rate")).expect("format")
}

fn settings() -> Config {
    let mut settings = Config::new();
    settings.set("width", &CANVAS.0.to_string()).expect("width");
    settings
        .set("height", &CANVAS.1.to_string())
        .expect("height");
    settings
}

/// A source that counts how often it is constructed and can be made to fail.
///
/// Construction count is the thing under test: for a real camera it is the
/// moment the driver is opened, which is exactly what a scene edit must not
/// trigger.
struct CountingFactory {
    kind: Identifier,
    color: [u8; 4],
    creations: Arc<AtomicU64>,
    broken: Arc<AtomicBool>,
}

impl SourceFactory for CountingFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, _settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        self.creations.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(CountingSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            color: self.color,
            broken: Arc::clone(&self.broken),
            frames: Arc::new(AtomicU64::new(0)),
        }))
    }
}

struct CountingSource {
    kind: Identifier,
    name: String,
    color: [u8; 4],
    broken: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
}

impl Source for CountingSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, _settings: &Config) -> Result<(), SourceError> {
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if self.broken.load(Ordering::Relaxed) {
            return Err(SourceError::Unavailable("device is gone".to_owned()));
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
        Ok(Some(VideoFrame::solid(
            request.format(),
            request.timestamp(),
            self.color,
        )))
    }
}

struct TestPlugin {
    manifest: PluginManifest,
    factories: Vec<Arc<dyn SourceFactory>>,
}

impl Plugin for TestPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source_factories(&self) -> &[Arc<dyn SourceFactory>] {
        &self.factories
    }
}

/// What a test can observe about the two devices in the scene.
struct Fixture {
    runtime: Runtime,
    screen_creations: Arc<AtomicU64>,
    camera_creations: Arc<AtomicU64>,
    camera_broken: Arc<AtomicBool>,
    screen_broken: Arc<AtomicBool>,
}

/// Builds the ordinary streaming layout: a full-canvas screen capture with a
/// camera scaled to 30% in the bottom-right corner.
fn fixture() -> Fixture {
    let screen_creations = Arc::new(AtomicU64::new(0));
    let camera_creations = Arc::new(AtomicU64::new(0));
    let screen_broken = Arc::new(AtomicBool::new(false));
    let camera_broken = Arc::new(AtomicBool::new(false));
    let plugin = TestPlugin {
        manifest: PluginManifest::new("test_devices", "Test devices", "1.0.0")
            .expect("manifest"),
        factories: vec![
            Arc::new(CountingFactory {
                kind: Identifier::new("test_screen").expect("kind"),
                color: [0x20, 0x40, 0x60, 0xff],
                creations: Arc::clone(&screen_creations),
                broken: Arc::clone(&screen_broken),
            }),
            Arc::new(CountingFactory {
                kind: Identifier::new("test_camera").expect("kind"),
                color: [0xc0, 0x80, 0x40, 0xff],
                creations: Arc::clone(&camera_creations),
                broken: Arc::clone(&camera_broken),
            }),
        ],
    };

    let mut runtime = Runtime::new();
    runtime.register_plugin(&plugin).expect("register");
    let screen = runtime
        .create_source("test_screen", "Screen", &settings())
        .expect("screen source");
    let camera = runtime
        .create_source("test_camera", "Camera", &settings())
        .expect("camera source");
    runtime.create_scene("live").expect("scene");
    runtime.attach_source("live", screen).expect("attach screen");
    runtime.attach_source("live", camera).expect("attach camera");
    runtime
        .set_source_transform("live", screen, FrameTransform::IDENTITY)
        .expect("screen transform");
    runtime
        .set_source_transform("live", camera, corner_transform(300))
        .expect("camera transform");

    Fixture {
        runtime,
        screen_creations,
        camera_creations,
        camera_broken,
        screen_broken,
    }
}

/// A transform that scales a source to `scale_milli` and parks it bottom-right.
fn corner_transform(scale_milli: u32) -> FrameTransform {
    let width = i32::try_from(CANVAS.0).expect("canvas width");
    let height = i32::try_from(CANVAS.1).expect("canvas height");
    let scale = i32::try_from(scale_milli).expect("scale");
    FrameTransform::new(
        scale_milli,
        scale_milli,
        width - width * scale / 1_000,
        height - height * scale / 1_000,
        false,
        false,
        255,
    )
    .expect("transform")
}

fn render(runtime: &mut Runtime, frame: u64) -> usize {
    let request = VideoRequest::new(
        Timestamp::from_nanos(frame.saturating_mul(33_333_333)),
        format(),
    );
    runtime
        .render_scene_layers("live", &request)
        .expect("scene layers")
        .len()
}

#[test]
fn a_camera_over_a_screen_capture_keeps_delivering_both_layers() {
    let mut fixture = fixture();

    for frame in 0..300 {
        assert_eq!(
            render(&mut fixture.runtime, frame),
            2,
            "both layers must be composited on frame {frame}"
        );
    }

    let metrics = fixture.runtime.compositor_metrics();
    assert_eq!(metrics.source_frames(), 600);
    assert_eq!(metrics.failed_sources(), 0);
    // One device each, for the whole run.
    assert_eq!(fixture.screen_creations.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.camera_creations.load(Ordering::Relaxed), 1);
}

#[test]
fn moving_a_source_never_reopens_a_capture_device() {
    let mut fixture = fixture();
    let camera = *fixture
        .runtime
        .scene_sources("live")
        .expect("scene sources")
        .last()
        .expect("camera is the top layer");

    // A drag is hundreds of transform writes. Not one of them may reach a
    // device: reopening a camera mid-drag is what made the preview stutter and
    // the screen share drop.
    for step in 0..500_u32 {
        fixture
            .runtime
            .set_source_transform("live", camera, corner_transform(200 + step % 600))
            .expect("transform");
        render(&mut fixture.runtime, u64::from(step));
    }

    assert_eq!(fixture.screen_creations.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.camera_creations.load(Ordering::Relaxed), 1);
}

#[test]
fn a_failing_camera_leaves_the_screen_capture_composited() {
    let mut fixture = fixture();
    render(&mut fixture.runtime, 0);

    fixture.camera_broken.store(true, Ordering::Relaxed);
    // The camera's last good frame stands in while it is failing, so the layer
    // stays in the scene rather than blinking out of it.
    assert_eq!(render(&mut fixture.runtime, 1), 2);
    assert!(fixture.runtime.compositor_metrics().failed_sources() > 0);
    assert_eq!(fixture.runtime.source_failures().len(), 1);

    fixture.camera_broken.store(false, Ordering::Relaxed);
    assert_eq!(render(&mut fixture.runtime, 2), 2);
    assert!(fixture.runtime.source_failures().is_empty());
}

#[test]
fn a_failing_screen_capture_leaves_the_camera_composited() {
    let mut fixture = fixture();
    // Nothing has rendered yet, so there is no frame to fall back to: the
    // screen layer drops out and the camera must still be composited.
    fixture.screen_broken.store(true, Ordering::Relaxed);

    assert_eq!(render(&mut fixture.runtime, 0), 1);
    assert_eq!(fixture.runtime.compositor_metrics().failed_sources(), 1);

    fixture.screen_broken.store(false, Ordering::Relaxed);
    assert_eq!(render(&mut fixture.runtime, 1), 2);
}
