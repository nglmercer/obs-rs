use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use super::{bundle::DiagnosticBundle, error::DiagnosticError, types::DiagnosticFileState};
/// A crash-safe diagnostics writer using temporary-file plus rename finalization.
pub struct AtomicDiagnosticFileWriter {
    final_path: PathBuf,
    temp_path: PathBuf,
    state: DiagnosticFileState,
    committed_bytes: Option<usize>,
}

impl AtomicDiagnosticFileWriter {
    /// Creates an open writer with explicit, distinct final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticError::InvalidPaths`] when either path is empty or the
    /// paths are identical.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
    ) -> Result<Self, DiagnosticError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty()
            || temp_path.as_os_str().is_empty()
            || final_path == temp_path
        {
            return Err(DiagnosticError::InvalidPaths);
        }
        Ok(Self {
            final_path,
            temp_path,
            state: DiagnosticFileState::Open,
            committed_bytes: None,
        })
    }

    /// Encodes, synchronizes, and atomically renames one bundle into place.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer is closed, the bundle violates its limits,
    /// or a filesystem operation fails. A failed write removes the temporary file.
    pub fn finalize(&mut self, bundle: &DiagnosticBundle) -> Result<usize, DiagnosticError> {
        self.ensure_open("finalize")?;
        let bytes = bundle.encode()?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| io_error("open temporary file", &error))?;
            file.write_all(&bytes)
                .map_err(|error| io_error("write temporary file", &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync temporary file", &error))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| io_error("rename diagnostics bundle", &error))?;
            Ok::<(), DiagnosticError>(())
        })();

        if let Err(error) = result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }
        self.committed_bytes = Some(bytes.len());
        self.state = DiagnosticFileState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes a temporary artifact if present.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer is already closed or the temporary file
    /// cannot be removed.
    pub fn abort(&mut self) -> Result<(), DiagnosticError> {
        self.ensure_open("abort")?;
        match fs::remove_file(&self.temp_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("remove temporary file", &error)),
        }
        self.state = DiagnosticFileState::Aborted;
        Ok(())
    }

    /// Returns the writer lifecycle state.
    #[must_use]
    pub const fn state(&self) -> DiagnosticFileState {
        self.state
    }

    /// Returns the selected final path.
    #[must_use]
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Returns the selected temporary path.
    #[must_use]
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    /// Returns the committed encoded byte count after finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), DiagnosticError> {
        if self.state == DiagnosticFileState::Open {
            Ok(())
        } else {
            Err(DiagnosticError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}
fn io_error(operation: &str, error: &std::io::Error) -> DiagnosticError {
    DiagnosticError::Io {
        operation: operation.to_owned(),
        message: error.to_string(),
    }
}
