use std::sync::Arc;

use obs_rs_capture::{
    CaptureKind, SimulatedCaptureFactory, TestPatternFactory, CAMERA_CAPTURE_SOURCE_KIND,
};
#[cfg(not(target_os = "windows"))]
use obs_rs_capture::{SCREEN_CAPTURE_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND};
use obs_rs_plugin_api::{PluginError, SourceFactory};

use crate::portable::ColorSourceFactory;
#[cfg(target_os = "linux")]
use crate::wayland::WaylandCaptureFactory;
#[cfg(target_os = "windows")]
use crate::windows::WindowsCaptureFactory;
#[cfg(target_os = "linux")]
use crate::x11::X11CaptureFactory;

pub(crate) fn build() -> Result<Vec<Arc<dyn SourceFactory>>, PluginError> {
    let color_factory = ColorSourceFactory::new()?;
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
    let camera_factory =
        SimulatedCaptureFactory::new(CAMERA_CAPTURE_SOURCE_KIND, CaptureKind::Camera)?;
    #[allow(unused_mut)]
    let mut factories: Vec<Arc<dyn SourceFactory>> = vec![
        Arc::new(color_factory),
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
