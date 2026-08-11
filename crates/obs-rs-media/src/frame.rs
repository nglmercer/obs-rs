use super::{
    error::MediaError, filters::FrameFilter, format::VideoFormat, time::Timestamp,
    transform::FrameTransform, transition::FrameTransition,
};
use rayon::prelude::*;
/// An owned, tightly packed RGBA8 video frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFrame {
    format: VideoFormat,
    timestamp: Timestamp,
    pixels: Vec<u8>,
}

impl VideoFrame {
    /// Creates a frame after checking the buffer length against `format`.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BufferSize`] when `pixels` is not exactly the number
    /// of RGBA8 bytes required by `format`.
    pub fn new(
        format: VideoFormat,
        timestamp: Timestamp,
        pixels: Vec<u8>,
    ) -> Result<Self, MediaError> {
        let expected = format.rgba_bytes();
        if pixels.len() != expected {
            return Err(MediaError::BufferSize {
                expected,
                actual: pixels.len(),
            });
        }

        Ok(Self {
            format,
            timestamp,
            pixels,
        })
    }

    /// Creates a frame from a buffer whose length is already known to match.
    ///
    /// Internal fast path for call sites that derive `pixels` from
    /// [`VideoFormat::rgba_bytes`] and therefore cannot violate the length
    /// invariant. Callers outside this crate must use [`VideoFrame::new`].
    pub(crate) const fn new_unchecked(
        format: VideoFormat,
        timestamp: Timestamp,
        pixels: Vec<u8>,
    ) -> Self {
        debug_assert!(pixels.len() == format.rgba_bytes());
        Self {
            format,
            timestamp,
            pixels,
        }
    }

    /// Creates a solid-color frame.
    #[must_use]
    pub fn solid(format: VideoFormat, timestamp: Timestamp, color: [u8; 4]) -> Self {
        let len = format.rgba_bytes();
        let mut pixels = vec![0_u8; len];
        pixels
            .par_chunks_exact_mut(4)
            .for_each(|pixel| pixel.copy_from_slice(&color));

        Self::new_unchecked(format, timestamp, pixels)
    }

    /// Returns the frame format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the frame timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns the immutable RGBA8 bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Consumes the frame and returns its owned RGBA8 buffer.
    ///
    /// Lets an encoder at the end of the pipeline take ownership of the pixels
    /// instead of copying them out of a borrowed frame.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Returns one pixel if `(x, y)` is inside the frame.
    ///
    /// The bounds check above makes the index conversions exact, so this reads
    /// the four bytes directly rather than running fallible conversions.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.format.width || y >= self.format.height {
            return None;
        }

        let offset = (y as usize * self.format.width_index() + x as usize) * 4;
        let pixel: &[u8; 4] = self.pixels.get(offset..offset + 4)?.try_into().ok()?;
        Some(*pixel)
    }

    /// Overlays `foreground` using straight-alpha RGBA blending.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] when the two frames do not share the
    /// same video format.
    pub fn blend_over(&mut self, foreground: &Self) -> Result<(), MediaError> {
        if self.format != foreground.format {
            return Err(MediaError::FormatMismatch {
                expected: self.format,
                actual: foreground.format,
            });
        }

        self.pixels
            .par_chunks_exact_mut(4)
            .zip(foreground.pixels.par_chunks_exact(4))
            .for_each(|(background, source)| {
                let source_alpha = u32::from(source[3]);
                if source_alpha == 0 {
                    return;
                }
                if source_alpha == u32::from(u8::MAX) {
                    background.copy_from_slice(source);
                    return;
                }

                let inverse_alpha = 255 - source_alpha;
                let background_alpha = u32::from(background[3]);
                let output_alpha = source_alpha + background_alpha * inverse_alpha / 255;
                if output_alpha == 0 {
                    background.fill(0);
                    return;
                }

                // The denominator and both alpha weights are identical for all
                // three colour channels, so they are computed once per pixel.
                let denominator = output_alpha * 255;
                let source_weight = source_alpha * 255;
                let background_weight = background_alpha * inverse_alpha;
                for channel in 0..3 {
                    let numerator = u32::from(source[channel]) * source_weight
                        + u32::from(background[channel]) * background_weight;
                    background[channel] = to_byte(numerator / denominator);
                }
                background[3] = to_byte(output_alpha);
            });

        Ok(())
    }

    /// Produces a deterministic transition between two same-format frames.
    ///
    /// `destination` is taken by value and becomes the result buffer: a cut
    /// returns it untouched and a cross-fade blends `source` into it in place,
    /// so neither path copies a frame. The destination timestamp is used for the
    /// result. Cross-fades interpolate every RGBA byte with integer arithmetic,
    /// which makes offline previews and live output use the same correctness
    /// oracle.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] for different formats or
    /// [`MediaError::InvalidTransition`] for an invalid cross-fade progress value.
    pub fn transitioned(
        source: &Self,
        mut destination: Self,
        transition: FrameTransition,
    ) -> Result<Self, MediaError> {
        if source.format != destination.format {
            return Err(MediaError::FormatMismatch {
                expected: source.format,
                actual: destination.format,
            });
        }

        match transition {
            FrameTransition::Cut => Ok(destination),
            FrameTransition::CrossFade { progress_milli } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                let destination_weight = u32::from(progress_milli);
                let source_weight = 1_000 - destination_weight;
                // Both buffers have the same format and therefore the same
                // length, so this is a straight paired walk with no branching
                // and no intermediate allocation.
                for (target, source_byte) in destination.pixels.iter_mut().zip(&source.pixels) {
                    let value = u32::from(*source_byte) * source_weight
                        + u32::from(*target) * destination_weight;
                    *target = to_byte((value + 500) / 1_000);
                }
                Ok(destination)
            }
        }
    }

    /// Applies a nearest-neighbor transform into a new transparent frame.
    ///
    /// Pixels outside the transformed source remain transparent. Alpha is
    /// multiplied by the transform opacity; RGB values are otherwise preserved.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] if the transform was not constructed
    /// through a valid constructor.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "source_x/source_y are bounds-checked against the validated frame \
                  dimensions above, so both are non-negative and below u32::MAX"
    )]
    pub fn transformed(&self, transform: FrameTransform) -> Result<Self, MediaError> {
        if transform.scale_x_milli == 0
            || transform.scale_y_milli == 0
            || transform.scale_x_milli > FrameTransform::MAX_SCALE_MILLI
            || transform.scale_y_milli > FrameTransform::MAX_SCALE_MILLI
        {
            return Err(MediaError::InvalidTransform);
        }

        let mut output = Self::solid(self.format, self.timestamp, [0, 0, 0, 0]);
        let width = i64::from(self.format.width);
        let height = i64::from(self.format.height);
        let scale_x = i64::from(transform.scale_x_milli);
        let scale_y = i64::from(transform.scale_y_milli);
        let output_width = self.format.width_index();
        let translate_x = i64::from(transform.translate_x);
        let translate_y = i64::from(transform.translate_y);
        let opacity = u32::from(transform.opacity);

        for y in 0..self.format.height {
            let local_y = i64::from(y) - translate_y;
            if local_y < 0 {
                continue;
            }
            let mut source_y = local_y * 1_000 / scale_y;
            if source_y >= height {
                continue;
            }
            if transform.flip_y {
                source_y = height - 1 - source_y;
            }
            // Both row bases are constant across the scanline. `source_y` and
            // `y` are within the validated frame height here, so the index
            // conversions are exact.
            let source_row = source_y as usize * output_width;
            let output_row = y as usize * output_width;

            for x in 0..self.format.width {
                let local_x = i64::from(x) - translate_x;
                if local_x < 0 {
                    continue;
                }
                let mut source_x = local_x * 1_000 / scale_x;
                if source_x >= width {
                    continue;
                }
                if transform.flip_x {
                    source_x = width - 1 - source_x;
                }

                let source_offset = (source_row + source_x as usize) * 4;
                let output_offset = (output_row + x as usize) * 4;
                let source_pixel = &self.pixels[source_offset..source_offset + 4];
                let target_pixel = &mut output.pixels[output_offset..output_offset + 4];
                target_pixel.copy_from_slice(source_pixel);
                let alpha = u32::from(target_pixel[3]) * opacity / u32::from(u8::MAX);
                target_pixel[3] = to_byte(alpha);
            }
        }

        Ok(output)
    }

    /// Applies one CPU filter and returns a new owned frame.
    ///
    /// This allocates and copies a full frame buffer (roughly 33 MB at 4K), so
    /// it suits offline and test call sites. On a media callback or any other
    /// per-frame path, use [`VideoFrame::apply_filter`] to filter in place.
    #[must_use]
    pub fn filtered(&self, filter: FrameFilter) -> Self {
        let mut output = self.clone();
        output.apply_filter(filter);
        output
    }

    /// Applies one CPU filter in place without allocating another frame.
    pub fn apply_filter(&mut self, filter: FrameFilter) {
        self.apply_filters(std::slice::from_ref(&filter));
    }

    /// Applies an ordered filter chain in one pass over the frame.
    ///
    /// Fusing the chain avoids reading and writing the complete pixel buffer for
    /// every individual filter while preserving the caller's filter order.
    pub fn apply_filters(&mut self, filters: &[FrameFilter]) {
        if filters.is_empty() {
            return;
        }
        self.pixels.par_chunks_exact_mut(4).for_each(|pixel| {
            for filter in filters {
                match *filter {
                    FrameFilter::Grayscale => {
                        let luma = (u32::from(pixel[0]) * 77
                            + u32::from(pixel[1]) * 150
                            + u32::from(pixel[2]) * 29)
                            / 256;
                        let luma = to_byte(luma);
                        pixel[0] = luma;
                        pixel[1] = luma;
                        pixel[2] = luma;
                    }
                    FrameFilter::Brightness { milli } => {
                        let multiplier = i32::from(milli) + 1_000;
                        for channel in &mut pixel[..3] {
                            let value = i32::from(*channel) * multiplier / 1_000;
                            *channel = to_byte(u32::try_from(value.max(0)).unwrap_or(u32::MAX));
                        }
                    }
                    FrameFilter::Opacity(opacity) => {
                        let alpha = u32::from(pixel[3]) * u32::from(opacity) / 255;
                        pixel[3] = to_byte(alpha);
                    }
                }
            }
        });
    }

    /// Clears RGB values on fully transparent pixels for canonical composition.
    pub fn clear_transparent_rgb(&mut self) {
        self.pixels.par_chunks_exact_mut(4).for_each(|pixel| {
            let mask = 0_u8.wrapping_sub(u8::from(pixel[3] != 0));
            pixel[0] &= mask;
            pixel[1] &= mask;
            pixel[2] &= mask;
        });
    }

    /// Calculates a stable FNV-1a checksum of the frame bytes.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for chunk in self.pixels.chunks_exact(8) {
            hash = fnv_step(hash, chunk[0]);
            hash = fnv_step(hash, chunk[1]);
            hash = fnv_step(hash, chunk[2]);
            hash = fnv_step(hash, chunk[3]);
            hash = fnv_step(hash, chunk[4]);
            hash = fnv_step(hash, chunk[5]);
            hash = fnv_step(hash, chunk[6]);
            hash = fnv_step(hash, chunk[7]);
        }
        for byte in self.pixels.chunks_exact(8).remainder() {
            hash = fnv_step(hash, *byte);
        }
        hash
    }
}

#[inline]
fn fnv_step(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "min constrains the value to 0..=255, so the cast is exact"
)]
fn to_byte(value: u32) -> u8 {
    value.min(u32::from(u8::MAX)) as u8
}
