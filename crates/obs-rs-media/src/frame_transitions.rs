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
            FrameTransition::Swipe {
                progress_milli,
                direction,
            } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                apply_swipe(source, &mut destination, progress_milli, direction);
                Ok(destination)
            }
        }
    }
}

/// Returns the number of pixels in the axis a transition moves along.
fn transition_axis_length(frame: &VideoFrame, direction: SlideDirection) -> usize {
    match direction {
        SlideDirection::Left | SlideDirection::Right => frame.format().width_index(),
        SlideDirection::Up | SlideDirection::Down => frame.format().height_index(),
    }
}

/// Converts bounded progress into a pixel offset on the selected axis.
fn transition_progress_pixels(
    frame: &VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) -> usize {
    let axis_length = transition_axis_length(frame, direction);
    let axis_length_u32 = match direction {
        SlideDirection::Left | SlideDirection::Right => frame.format().width(),
        SlideDirection::Up | SlideDirection::Down => frame.format().height(),
    };
    u64::from(progress_milli)
        .saturating_mul(u64::from(axis_length_u32))
        .checked_div(1_000)
        .and_then(|pixels| usize::try_from(pixels).ok())
        .unwrap_or(axis_length)
        .min(axis_length)
}

/// Converts a pixel coordinate into the start of its RGBA sample.
fn pixel_start(width: usize, x: usize, y: usize) -> usize {
    (y * width + x) * 4
}

/// Applies the bounded swipe transition in place without allocating a second
/// frame. Unlike slide, the destination stays fixed and only fills the area
/// uncovered by the source.
fn apply_swipe(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) {
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let progress_pixels = transition_progress_pixels(destination, progress_milli, direction);
    let axis_length = transition_axis_length(destination, direction);
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    for y in 0..height {
        for x in 0..width {
            let (source_x, source_y, source_visible) = match direction {
                SlideDirection::Left => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if x < boundary {
                        (x + progress_pixels, y, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Right => {
                    if x >= progress_pixels {
                        (x - progress_pixels, y, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Up => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if y < boundary {
                        (x, y + progress_pixels, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Down => {
                    if y >= progress_pixels {
                        (x, y - progress_pixels, true)
                    } else {
                        (x, y, false)
                    }
                }
            };
            if !source_visible {
                continue;
            }
            let target_start = pixel_start(width, x, y);
            let source_start = pixel_start(width, source_x, source_y);
            let source_pixel: [u8; 4] = source_pixels[source_start..source_start + 4]
                .try_into()
                .expect("RGBA pixel has four bytes");
            target_pixels[target_start..target_start + 4].copy_from_slice(&source_pixel);
        }
    }
}

/// Applies the bounded slide transition in place without allocating a second
/// frame. Traversal follows the movement direction so an original destination
/// pixel is read before the output can overwrite it.
fn apply_slide(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) {
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let progress_pixels = transition_progress_pixels(destination, progress_milli, direction);
    let axis_length = transition_axis_length(destination, direction);
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    let row_order = 0..height;
    for row in row_order {
        let y = if matches!(direction, SlideDirection::Up) {
            height - 1 - row
        } else {
            row
        };
        let column_order = 0..width;
        for column in column_order {
            let x = if matches!(direction, SlideDirection::Left) {
                width - 1 - column
            } else {
                column
            };
            let (sample_x, sample_y, source_visible) = match direction {
                SlideDirection::Left => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if x < boundary {
                        (x + progress_pixels, y, true)
                    } else {
                        (x - boundary, y, false)
                    }
                }
                SlideDirection::Right => {
                    if x >= progress_pixels {
                        (x - progress_pixels, y, true)
                    } else {
                        (x + axis_length.saturating_sub(progress_pixels), y, false)
                    }
                }
                SlideDirection::Up => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if y < boundary {
                        (x, y + progress_pixels, true)
                    } else {
                        (x, y - boundary, false)
                    }
                }
                SlideDirection::Down => {
                    if y >= progress_pixels {
                        (x, y - progress_pixels, true)
                    } else {
                        (x, y + axis_length.saturating_sub(progress_pixels), false)
                    }
                }
            };
            let target_start = pixel_start(width, x, y);
            let sample_start = pixel_start(width, sample_x, sample_y);
            let sample_pixel: [u8; 4] = if source_visible {
                source_pixels[sample_start..sample_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes")
            } else {
                // Traversal leaves the original destination sample untouched
                // until it is read: lower coordinates for left/up and higher
                // coordinates for right/down.
                target_pixels[sample_start..sample_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes")
            };
            target_pixels[target_start..target_start + 4].copy_from_slice(&sample_pixel);
        }
    }
}
