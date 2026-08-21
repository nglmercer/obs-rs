use std::sync::Arc;

use obs_rs_capture::{
    CaptureKind, SimulatedCaptureFactory, TestPatternFactory, CAMERA_CAPTURE_SOURCE_KIND,
    SCREEN_CAPTURE_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND,
};
use obs_rs_plugin_api::{PluginError, SourceFactory};

use crate::image::{ImageSlideshowSourceFactory, ImageSourceFactory};
use crate::portable::ColorSourceFactory;
use crate::text::TextSourceFactory;
#[cfg(target_os = "linux")]
use crate::wayland::WaylandCaptureFactory;
#[cfg(target_os = "linux")]
use crate::x11::X11CaptureFactory;

pub(crate) fn build() -> Result<Vec<Arc<dyn SourceFactory>>, PluginError> {
    let color_factory = ColorSourceFactory::new()?;
    let image_factory = ImageSourceFactory::new()?;
    let slideshow_factory = ImageSlideshowSourceFactory::new()?;
    let text_factory = TextSourceFactory::new()?;
    let test_pattern_factory = TestPatternFactory::new()?;
    let screen_factory =
        SimulatedCaptureFactory::new(SCREEN_CAPTURE_SOURCE_KIND, CaptureKind::Screen)?;
    let window_factory =
        SimulatedCaptureFactory::new(WINDOW_CAPTURE_SOURCE_KIND, CaptureKind::Window)?;
    let camera_factory =
        SimulatedCaptureFactory::new(CAMERA_CAPTURE_SOURCE_KIND, CaptureKind::Camera)?;
    let mut factories: Vec<Arc<dyn SourceFactory>> = vec![
        Arc::new(color_factory),
        Arc::new(image_factory),
        Arc::new(slideshow_factory),
        Arc::new(text_factory),
        Arc::new(test_pattern_factory),
        Arc::new(screen_factory),
        Arc::new(window_factory),
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
