use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use obs_rs_capture::NokhwaCaptureFactory;
use obs_rs_capture::TestPatternFactory;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use obs_rs_capture::CAMERA_CAPTURE_SOURCE_KIND;
#[cfg(not(target_os = "windows"))]
use obs_rs_capture::{CaptureKind, SimulatedCaptureFactory};
#[cfg(not(target_os = "windows"))]
use obs_rs_capture::{SCREEN_CAPTURE_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND};
use obs_rs_plugin_api::{PluginError, SourceFactory};

use crate::image::{ImageSlideshowSourceFactory, ImageSourceFactory};
use crate::media::MediaSourceFactory;
use crate::portable::ColorSourceFactory;
use crate::text::TextSourceFactory;
#[cfg(target_os = "linux")]
use crate::wayland::WaylandCaptureFactory;
#[cfg(target_os = "windows")]
use crate::windows::WindowsCaptureFactory;
#[cfg(target_os = "linux")]
use crate::x11::X11CaptureFactory;

pub(crate) fn build() -> Result<Vec<Arc<dyn SourceFactory>>, PluginError> {
    let color_factory = ColorSourceFactory::new()?;
    let image_factory = ImageSourceFactory::new()?;
    let slideshow_factory = ImageSlideshowSourceFactory::new()?;
    let media_factory = MediaSourceFactory::new()?;
    let text_factory = TextSourceFactory::new()?;
    let test_pattern_factory = TestPatternFactory::new()?;
    #[cfg(not(target_os = "windows"))]
    let screen_factory: Arc<dyn SourceFactory> = Arc::new(SimulatedCaptureFactory::new(
        SCREEN_CAPTURE_SOURCE_KIND,
        CaptureKind::Screen,
    )?);
    #[cfg(target_os = "windows")]
    let screen_factory: Arc<dyn SourceFactory> = Arc::new(WindowsCaptureFactory::screen()?);
    #[cfg(not(target_os = "windows"))]
    let window_factory: Arc<dyn SourceFactory> = Arc::new(SimulatedCaptureFactory::new(
        WINDOW_CAPTURE_SOURCE_KIND,
        CaptureKind::Window,
    )?);
    #[cfg(target_os = "windows")]
    let window_factory: Arc<dyn SourceFactory> = Arc::new(WindowsCaptureFactory::window()?);
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let camera_factory = NokhwaCaptureFactory::new()?;
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let camera_factory =
        SimulatedCaptureFactory::new(CAMERA_CAPTURE_SOURCE_KIND, CaptureKind::Camera)?;
    #[allow(unused_mut)]
    let mut factories: Vec<Arc<dyn SourceFactory>> = vec![
        Arc::new(color_factory),
        Arc::new(image_factory),
        Arc::new(slideshow_factory),
        Arc::new(media_factory),
        Arc::new(text_factory),
        Arc::new(test_pattern_factory),
        screen_factory,
        window_factory,
        Arc::new(camera_factory),
    ];
    #[cfg(target_os = "linux")]
    factories.push(Arc::new(X11CaptureFactory::new()?));
    #[cfg(target_os = "linux")]
    factories.push(Arc::new(X11CaptureFactory::for_windows()?));
    #[cfg(target_os = "linux")]
    factories.push(Arc::new(WaylandCaptureFactory::new()?));
    Ok(factories)
}
