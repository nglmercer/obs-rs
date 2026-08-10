use obs_rs_media::{RawVideoFrame, VideoFormat, VideoFrame};

use super::{
    error::RenderError,
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
