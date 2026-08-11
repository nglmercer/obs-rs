use obs_rs_media::{RawVideoFrame, VideoFormat, VideoFrame};

use super::{
    error::RenderError,
    layer::{OpaqueFrameSurface, SceneLayer, SurfaceImportMode},
    types::{RenderCapabilities, RenderMetrics, RenderState, TextureId},
};
/// Safe operations required from a hardware or CPU render backend.
pub trait RenderBackend {
    /// Returns backend capabilities.
    fn capabilities(&self) -> RenderCapabilities;

    /// Returns the current context state.
    fn state(&self) -> RenderState;

    /// Returns resource and data-movement counters collected by the backend.
    #[must_use]
    fn metrics(&self) -> RenderMetrics {
        RenderMetrics::default()
    }

    /// Reports whether a producer's opaque surface can use direct import.
    #[must_use]
    fn surface_import_mode(&self, _provider: &str) -> SurfaceImportMode {
        SurfaceImportMode::Unsupported
    }

    /// Submits ordered frames plus transform/filter metadata in one reusable
    /// backend operation.
    ///
    /// The default preserves source compatibility for existing third-party
    /// backends. Implementations opt in when they can fuse layer processing.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::LayerSubmissionUnsupported`] by default.
    fn submit_layers(
        &mut self,
        _target: TextureId,
        _layers: &[SceneLayer<'_>],
    ) -> Result<(), RenderError> {
        Err(RenderError::LayerSubmissionUnsupported)
    }

    /// Imports one opaque surface into an existing texture.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::SurfaceUnsupported`] by default.
    fn submit_surface(
        &mut self,
        _texture: TextureId,
        surface: &OpaqueFrameSurface,
    ) -> Result<(), RenderError> {
        Err(RenderError::SurfaceUnsupported {
            provider: surface.provider().to_owned(),
        })
    }

    /// Allocates an empty texture resource.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when the context is lost, the resource limit is
    /// reached, or ID allocation overflows.
    fn create_texture(&mut self, format: VideoFormat) -> Result<TextureId, RenderError>;

    /// Destroys a texture resource.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::UnknownTexture`] for an unknown handle.
    fn destroy_texture(&mut self, texture: TextureId) -> Result<(), RenderError>;

    /// Uploads one owned frame into a texture.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when the context, handle, or format is invalid.
    fn upload(&mut self, texture: TextureId, frame: &VideoFrame) -> Result<(), RenderError>;

    /// Uploads an owned frame without requiring a defensive copy at the backend
    /// boundary. Backends that do not have an ownership-aware implementation
    /// retain the borrowed upload as a compatibility fallback.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when the context, handle, or format is invalid.
    fn upload_owned(&mut self, texture: TextureId, frame: VideoFrame) -> Result<(), RenderError> {
        self.upload(texture, &frame)
    }

    /// Converts and uploads a packed or planar frame through the media boundary.
    ///
    /// The default implementation keeps backend contracts in RGBA8 while
    /// allowing capture adapters to submit their native validated layout.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Media`] for a conversion failure or any error
    /// returned by [`Self::upload`].
    fn upload_raw(&mut self, texture: TextureId, frame: &RawVideoFrame) -> Result<(), RenderError> {
        let rgba = frame.clone().into_rgba8().map_err(RenderError::Media)?;
        self.upload(texture, &rgba)
    }

    /// Converts and uploads an owned packed or planar frame without cloning the
    /// input layout or the converted RGBA buffer.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Media`] for a conversion failure or any error
    /// returned by [`Self::upload_owned`].
    fn upload_raw_owned(
        &mut self,
        texture: TextureId,
        frame: RawVideoFrame,
    ) -> Result<(), RenderError> {
        let rgba = frame.into_rgba8().map_err(RenderError::Media)?;
        self.upload_owned(texture, rgba)
    }

    /// Composites texture layers in order into a target texture.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when a layer is missing, not uploaded, or has a
    /// format mismatch.
    fn composite(&mut self, target: TextureId, layers: &[TextureId]) -> Result<(), RenderError>;

    /// Reads a texture back into an owned CPU frame.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when readback is unsupported or the texture is not
    /// ready.
    fn readback(&mut self, texture: TextureId) -> Result<VideoFrame, RenderError>;

    /// Recovers a lost context and invalidates uploaded resource contents.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when recovery cannot be completed.
    fn recover(&mut self) -> Result<(), RenderError>;
}
