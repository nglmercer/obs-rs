//! Non-live GUI view of preview runtime diagnostics and canvas format.

use std::{
    error::Error,
    sync::{Arc, Mutex},
};

use obs_rs_media::VideoFormat;
use obs_rs_project::Project;

use super::RuntimeDiagnostics;

/// The studio window's non-live view of the engine.
///
/// The window used to hold a second [`PreviewRenderer`], which meant a second
/// [`Runtime`], which meant every camera and screen-cast session in the project
/// was opened twice — once for the window that never rendered a frame from it,
/// and once for the worker that actually composites. Cameras in particular do
/// not survive being opened twice. The window needs the canvas format, the
/// revision it has observed, and engine counters, so that is all this carries;
/// the worker owns the only live runtime.
pub(crate) struct PreviewSurface {
    pub(crate) format: VideoFormat,
    revision: u64,
    diagnostics: Arc<Mutex<RuntimeDiagnostics>>,
}

impl PreviewSurface {
    /// Creates the window's view of `project` without opening any device.
    pub(crate) fn new(project: &Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        Ok(Self {
            format: profile.video_format(),
            revision,
            diagnostics: Arc::new(Mutex::new(RuntimeDiagnostics::default())),
        })
    }

    /// Returns the slot the preview worker publishes engine counters into.
    pub(crate) fn diagnostics_handle(&self) -> Arc<Mutex<RuntimeDiagnostics>> {
        Arc::clone(&self.diagnostics)
    }

    /// Returns the newest engine snapshot the worker published.
    pub(crate) fn diagnostics(&self) -> RuntimeDiagnostics {
        self.diagnostics
            .lock()
            .map_or_else(|_| RuntimeDiagnostics::default(), |value| value.clone())
    }

    pub(crate) const fn is_synced(&self, revision: u64) -> bool {
        self.revision == revision
    }

    /// Records a new project revision and the canvas it renders at.
    ///
    /// Nothing here touches a device: the worker's runtime is the only thing
    /// that opens capture hardware.
    pub(crate) fn sync_project(
        &mut self,
        project: &Project,
        revision: u64,
    ) -> Result<bool, Box<dyn Error>> {
        if revision == self.revision {
            return Ok(false);
        }
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        self.format = profile.video_format();
        self.revision = revision;
        Ok(true)
    }
}
