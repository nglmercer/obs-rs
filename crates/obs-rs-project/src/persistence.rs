use super::{error::ProjectError, model::Project, session::ProjectSession};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};
/// Crash-safe project-file persistence using temporary-file plus rename.
pub struct ProjectFileStore {
    final_path: PathBuf,
    temp_path: PathBuf,
}

impl ProjectFileStore {
    /// Creates a file store with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::InvalidPaths`] when a path is empty or both paths
    /// are equal.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
    ) -> Result<Self, ProjectError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(ProjectError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(ProjectError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            final_path,
            temp_path,
        })
    }

    /// Saves a session without marking it clean until the rename succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] for filesystem failures. The final path is
    /// left untouched when writing or synchronization fails.
    pub fn save(&self, session: &mut ProjectSession) -> Result<usize, ProjectError> {
        let document = session.document();
        let bytes = self.save_document(&document)?;
        session.mark_saved();
        Ok(bytes)
    }

    /// Writes an already serialized document without changing any session state.
    ///
    /// This is used for exports: the destination receives a crash-safe copy of
    /// the current document, while the active session remains open and dirty or
    /// clean exactly as it was before the export.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] for filesystem failures. The final path is
    /// left untouched when writing or synchronization fails.
    pub fn save_document(&self, document: &str) -> Result<usize, ProjectError> {
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| ProjectError::Io {
                    operation: "open project temporary file",
                    message: error.to_string(),
                })?;
            file.write_all(document.as_bytes())
                .map_err(|error| ProjectError::Io {
                    operation: "write project temporary file",
                    message: error.to_string(),
                })?;
            file.sync_all().map_err(|error| ProjectError::Io {
                operation: "sync project temporary file",
                message: error.to_string(),
            })?;
            fs::rename(&self.temp_path, &self.final_path).map_err(|error| ProjectError::Io {
                operation: "rename project file",
                message: error.to_string(),
            })?;
            Ok::<(), ProjectError>(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }
        Ok(document.len())
    }

    /// Loads and parses the final project file.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] for read failures or a parser error for an
    /// invalid document.
    pub fn load(&self) -> Result<Project, ProjectError> {
        let document = fs::read_to_string(&self.final_path).map_err(|error| ProjectError::Io {
            operation: "read project file",
            message: error.to_string(),
        })?;
        Project::parse(&document)
    }

    /// Reads a valid, unswitched temporary project after an interrupted save.
    ///
    /// The temporary file is never removed by this read. The caller can decide
    /// whether to recover it into memory and publish it through a later save.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] when the temporary file exists but cannot be
    /// read, or a parser error when its contents are incomplete or invalid.
    pub fn recover(&self) -> Result<Option<Project>, ProjectError> {
        if !self.temp_path.exists() {
            return Ok(None);
        }
        let document = fs::read_to_string(&self.temp_path).map_err(|error| ProjectError::Io {
            operation: "read project recovery file",
            message: error.to_string(),
        })?;
        Project::parse(&document).map(Some)
    }

    /// Returns whether an interrupted-save temporary file is present.
    #[must_use]
    pub fn recovery_available(&self) -> bool {
        self.temp_path.is_file()
    }

    /// Discards an interrupted-save temporary file after an explicit user
    /// decision.
    ///
    /// The final project is never touched. A missing recovery file is a
    /// successful no-op so a stale status cannot turn into a user-visible
    /// failure when another process already removed the artifact.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Io`] when the recovery file exists but cannot
    /// be removed.
    pub fn discard_recovery(&self) -> Result<bool, ProjectError> {
        if !self.temp_path.is_file() {
            return Ok(false);
        }
        fs::remove_file(&self.temp_path).map_err(|error| ProjectError::Io {
            operation: "remove project recovery file",
            message: error.to_string(),
        })?;
        Ok(true)
    }

    /// Returns the final project path.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary project path.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }
}
