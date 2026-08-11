use super::{commands::ProjectCommand, error::ProjectError, model::Project};
use std::cell::RefCell;
/// Mutable project controller that tracks unsaved changes.
pub struct ProjectSession {
    project: Project,
    dirty: bool,
    /// Monotonic counter bumped by every accepted mutation.
    ///
    /// Lets an observer such as the GUI refresh detect "did the project change
    /// since I last looked?" by comparing two integers, instead of serializing
    /// the whole document and comparing strings on every frame.
    revision: u64,
    document_cache: RefCell<Option<(u64, String)>>,
}

impl ProjectSession {
    /// Opens a clean session around a project.
    #[must_use]
    pub const fn new(project: Project) -> Self {
        Self {
            project,
            dirty: false,
            revision: 1,
            document_cache: RefCell::new(None),
        }
    }

    /// Applies a command and marks the project dirty only after success.
    ///
    /// # Errors
    ///
    /// Returns the project validation error and leaves the dirty flag unchanged.
    pub fn dispatch(&mut self, command: ProjectCommand) -> Result<(), ProjectError> {
        self.project.apply(command)?;
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.document_cache.borrow_mut().take();
        Ok(())
    }

    /// Returns the current project.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    /// Replaces the project wholesale, as a load or recovery does.
    ///
    /// The revision keeps advancing across the replacement so observers cannot
    /// mistake a newly loaded project for the previous one.
    pub fn replace(&mut self, project: Project) {
        self.project = project;
        self.dirty = false;
        self.revision = self.revision.wrapping_add(1);
        self.document_cache.borrow_mut().take();
    }

    /// Returns the current mutation revision.
    ///
    /// The value changes whenever an applied command mutates the project, and
    /// is only meaningful compared against an earlier reading of the same
    /// session. A fresh session starts at a non-zero value, so `0` is usable as
    /// a "nothing observed yet" sentinel.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns whether commands have changed the persisted state.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Serializes and marks the session clean, representing a successful save.
    #[must_use]
    pub fn save(&mut self) -> String {
        let document = self.document();
        self.mark_saved();
        document
    }

    /// Serializes the current state without changing dirty status.
    #[must_use]
    pub fn document(&self) -> String {
        if let Some((revision, document)) = self.document_cache.borrow().as_ref() {
            if *revision == self.revision {
                return document.clone();
            }
        }
        let document = self.project.serialize();
        *self.document_cache.borrow_mut() = Some((self.revision, document.clone()));
        document
    }

    /// Marks the session clean after an external persistence operation succeeds.
    pub const fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Marks the session dirty after recovering an unswitched temporary file.
    pub const fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
