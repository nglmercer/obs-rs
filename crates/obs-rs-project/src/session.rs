use super::{commands::ProjectCommand, error::ProjectError, model::Project};
/// Mutable project controller that tracks unsaved changes.
pub struct ProjectSession {
    project: Project,
    dirty: bool,
}

impl ProjectSession {
    /// Opens a clean session around a project.
    #[must_use]
    pub const fn new(project: Project) -> Self {
        Self {
            project,
            dirty: false,
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
        Ok(())
    }

    /// Returns the current project.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
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
        self.project.serialize()
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
