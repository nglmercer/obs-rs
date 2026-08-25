//! Frame-to-frame transition rendering kept separate from frame transforms.

use super::{
    error::MediaError,
    frame::{to_byte, VideoFrame},
    transition::{FrameTransition, SlideDirection},
};

impl VideoFrame {
    /// Produces a deterministic transition between two same-format frames.
    ///
    /// `destination` is taken by value and becomes the result buffer: a cut
    /// returns it untouched and a cross-fade blends `source` into it in place,
    /// so neither path copies a frame. The destination timestamp is used for
    /// the result. Cross-fades, fade-to-color, and slide transitions use
    /// integer arithmetic, which makes offline previews and live output use
    /// the same correctness oracle.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] for different formats or
    /// [`MediaError::InvalidTransition`] for an invalid transition progress value.
    pub fn transitioned(
        source: &Self,
        mut destination: Self,
        transition: FrameTransition,
    ) -> Result<Self, MediaError> {
        if source.format() != destination.format() {
            return Err(MediaError::FormatMismatch {
                expected: source.format(),
                actual: destination.format(),
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
                for (target, source_byte) in
                    destination.pixels_mut().iter_mut().zip(source.pixels())
                {
                    let value = u32::from(*source_byte) * source_weight
                        + u32::from(*target) * destination_weight;
                    *target = to_byte((value + 500) / 1_000);
                }
                Ok(destination)
            }
            FrameTransition::FadeToColor {
                progress_milli,
                color,
            } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                if progress_milli <= 500 {
                    let color_weight = u32::from(progress_milli).saturating_mul(2);
                    let source_weight = 1_000 - color_weight;
                    for (index, (target, source_byte)) in destination
                        .pixels_mut()
                        .iter_mut()
                        .zip(source.pixels())
                        .enumerate()
                    {
                        let value = u32::from(*source_byte) * source_weight
                            + u32::from(color[index % 4]) * color_weight;
                        *target = to_byte((value + 500) / 1_000);
                    }
                } else {
                    let destination_weight =
                        u32::from(progress_milli.saturating_sub(500)).saturating_mul(2);
                    let color_weight = 1_000 - destination_weight;
                    for (index, target) in destination.pixels_mut().iter_mut().enumerate() {
                        let value = u32::from(color[index % 4]) * color_weight
                            + u32::from(*target) * destination_weight;
                        *target = to_byte((value + 500) / 1_000);
                    }
                }
                Ok(destination)
            }
            FrameTransition::Slide {
                progress_milli,
                direction,
            } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                apply_slide(source, &mut destination, progress_milli, direction);
                Ok(destination)
            }
        }
    }
}

/// Applies the bounded slide transition in place without allocating a second
/// frame. The destination region is visited from right to left so an original
/// destination pixel is read before the output can overwrite it.
fn apply_slide(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) {
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let progress_pixels = u64::from(progress_milli)
        .saturating_mul(u64::from(destination.format().width()))
        .checked_div(1_000)
        .and_then(|pixels| usize::try_from(pixels).ok())
        .unwrap_or(width)
        .min(width);
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    match direction {
        SlideDirection::Left => {
            let destination_start = width.saturating_sub(progress_pixels);
            for row in 0..height {
                let row_start = row * width * 4;
                for x in (0..width).rev() {
                    let target_start = row_start + x * 4;
                    let source_x = if x < destination_start {
                        x + progress_pixels
                    } else {
                        x - destination_start
                    };
                    let source_start = row_start + source_x * 4;
                    let source_pixel: [u8; 4] = if x < destination_start {
                        source_pixels[source_start..source_start + 4]
                            .try_into()
                            .expect("RGBA pixel has four bytes")
                    } else {
                        // Reverse traversal leaves this lower source
                        // coordinate untouched until it is read.
                        target_pixels[source_start..source_start + 4]
                            .try_into()
                            .expect("RGBA pixel has four bytes")
                    };
                    target_pixels[target_start..target_start + 4].copy_from_slice(&source_pixel);
                }
            }
        }
    }
}
