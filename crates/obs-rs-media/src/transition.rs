use super::error::MediaError;
/// A video transition applied between two frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameTransition {
    /// Selects the destination frame immediately.
    Cut,
    /// Linearly interpolates source and destination bytes from 0 to 1000.
    CrossFade { progress_milli: u16 },
}

impl FrameTransition {
    /// Creates a cross-fade at a validated progress value.
    ///
    /// `0` selects the source frame and `1000` selects the destination frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransition`] when `progress_milli` is greater
    /// than `1000`.
    pub const fn cross_fade(progress_milli: u16) -> Result<Self, MediaError> {
        if progress_milli > 1_000 {
            return Err(MediaError::InvalidTransition { progress_milli });
        }
        Ok(Self::CrossFade { progress_milli })
    }
}
