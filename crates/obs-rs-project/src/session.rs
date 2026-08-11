use super::{commands::ProjectCommand, error::ProjectError, model::Project};
use std::{cell::RefCell, collections::VecDeque};

/// Maximum number of prior project states retained for undo.
///
/// A project document is bounded by [`crate::MAX_PROJECT_BYTES`], so the whole
/// history is bounded too and an editing session cannot grow without limit.
pub const MAX_HISTORY_DEPTH: usize = 64;

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
    /// States that preceded each accepted mutation, oldest first.
    undo_stack: VecDeque<Project>,
    /// States undone but not yet superseded by a new edit, oldest first.
    redo_stack: VecDeque<Project>,
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
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
        }
    }

    /// Applies a command and marks the project dirty only after success.
    ///
    /// A successful command records the previous state so it can be undone, and
    /// discards any redo branch, because redoing onto a diverged state would
    /// reapply an edit the user has already replaced.
    ///
    /// # Errors
    ///
    /// Returns the project validation error and leaves the dirty flag unchanged.
    pub fn dispatch(&mut self, command: ProjectCommand) -> Result<(), ProjectError> {
        // The snapshot is taken before the mutation but only committed to the
        // history after it succeeds, so a rejected command leaves no undo step.
        let previous = self.project.clone();
        self.project.apply(command)?;
        self.push_history(previous);
        self.redo_stack.clear();
        self.dirty = true;
        self.bump_revision();
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
    /// mistake a newly loaded project for the previous one. History is dropped:
    /// a load is a different document, not an edit of the current one, so
    /// undoing across it would resurrect state from an unrelated project.
    pub fn replace(&mut self, project: Project) {
        self.project = project;
        self.dirty = false;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.bump_revision();
    }

    /// Returns whether an earlier state is available to restore.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Returns whether an undone state is available to reapply.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Restores the state that preceded the most recent accepted command.
    ///
    /// Returns `false` when there is nothing to undo, which callers report as a
    /// no-op rather than as a failure.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop_back() else {
            return false;
        };
        let current = std::mem::replace(&mut self.project, previous);
        // The redo stack mirrors the undo bound, so a long undo run cannot make
        // the session retain more than twice the bounded history.
        if self.redo_stack.len() == MAX_HISTORY_DEPTH {
            self.redo_stack.pop_front();
        }
        self.redo_stack.push_back(current);
        self.dirty = true;
        self.bump_revision();
        true
    }

    /// Reapplies the most recently undone state.
    ///
    /// Returns `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop_back() else {
            return false;
        };
        let current = std::mem::replace(&mut self.project, next);
        self.push_history(current);
        self.dirty = true;
        self.bump_revision();
        true
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

    /// Records one prior state, dropping the oldest once the bound is reached.
    fn push_history(&mut self, previous: Project) {
        if self.undo_stack.len() == MAX_HISTORY_DEPTH {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(previous);
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.document_cache.borrow_mut().take();
    }
}
