use std::fmt;

use obs_rs_media::{MediaError, VideoFormat};

use super::types::TextureId;
/// Errors raised by render resources and composition.
#[derive(Debug, Eq, PartialEq)]
pub enum RenderError {
    /// A backend cannot be created with zero texture capacity.
    ZeroCapacity,
    /// A texture allocation would exceed the backend resource limit.
    TextureLimit { limit: usize },
    /// A texture allocation would exceed the aggregate byte budget.
    TextureByteLimit { limit: usize, requested: usize },
    /// Texture ID allocation overflowed.
    IdExhausted,
    /// A texture handle is not owned by the backend.
    UnknownTexture(TextureId),
    /// An operation requires a recovered context.
    ContextLost,
    /// A frame and texture use different formats.
    FormatMismatch {
        /// Format owned by the texture.
        expected: VideoFormat,
        /// Format supplied by the caller.
        actual: VideoFormat,
    },
    /// A texture has not received an uploaded frame.
    TextureNotReady(TextureId),
    /// A composition request contains no layers.
    EmptyComposition,
    /// The backend does not implement metadata-rich layer submission.
    LayerSubmissionUnsupported,
    /// A native surface provider cannot be imported by this backend.
    SurfaceUnsupported { provider: String },
    /// A media invariant failed during CPU composition.
    Media(MediaError),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("render texture capacity must be non-zero"),
            Self::TextureLimit { limit } => {
                write!(formatter, "render texture limit reached: {limit}")
            }
            Self::TextureByteLimit { limit, requested } => write!(
                formatter,
                "render texture allocation of {requested} bytes exceeds {limit}-byte budget"
            ),
            Self::IdExhausted => formatter.write_str("render texture ID space is exhausted"),
            Self::UnknownTexture(id) => {
                write!(formatter, "render texture {} does not exist", id.value())
            }
            Self::ContextLost => formatter.write_str("render context is lost"),
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "render format {actual:?} does not match {expected:?}"
                )
            }
            Self::TextureNotReady(id) => {
                write!(formatter, "render texture {} has no frame", id.value())
            }
            Self::EmptyComposition => {
                formatter.write_str("render composition needs at least one layer")
            }
            Self::LayerSubmissionUnsupported => {
                formatter.write_str("render backend does not support scene-layer submission")
            }
            Self::SurfaceUnsupported { provider } => {
                write!(
                    formatter,
                    "render backend cannot import {provider} surfaces"
                )
            }
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RenderError {}
