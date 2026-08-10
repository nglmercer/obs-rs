use super::super::CaptureError;
use super::{
    error::protocol_error,
    protocol::{ImageByteOrder, VisualMasks},
};

/// Geometry of one X11 image reply, resolved once before the decode loop.
struct DecodeLayout {
    width: usize,
    row_stride: usize,
    row_bytes: usize,
    bytes_per_pixel: usize,
}

/// A channel mask with its shift and range resolved once per frame.
///
/// `mask.trailing_zeros()` and the derived maximum are constant for the whole
/// frame, so they are computed here instead of per channel per pixel.
#[derive(Clone, Copy)]
struct ChannelScale {
    mask: u32,
    shift: u32,
    maximum: u64,
}

impl ChannelScale {
    fn new(mask: u32) -> Self {
        if mask == 0 {
            return Self {
                mask: 0,
                shift: 0,
                maximum: 1,
            };
        }
        let shift = mask.trailing_zeros();
        Self {
            mask,
            shift,
            maximum: u64::from(mask >> shift).max(1),
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "min constrains the value to 0..=255, so the cast is exact"
    )]
    fn apply(self, pixel: u32) -> u8 {
        if self.mask == 0 {
            return 0;
        }
        let value = u64::from((pixel & self.mask) >> self.shift);
        let scaled = value.saturating_mul(u64::from(u8::MAX)) / self.maximum;
        scaled.min(u64::from(u8::MAX)) as u8
    }
}

pub(crate) fn decode_pixels(
    width: usize,
    height: usize,
    row_stride: usize,
    bits_per_pixel: u8,
    byte_order: ImageByteOrder,
    masks: VisualMasks,
    data: &[u8],
) -> Result<Vec<u8>, CaptureError> {
    let bytes_per_pixel = usize::from(bits_per_pixel / 8);
    if bytes_per_pixel == 0 || bytes_per_pixel > 4 {
        return Err(protocol_error("X11 image has an unsupported pixel size"));
    }
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let mut output = vec![0_u8; pixel_bytes];
    if width == 0 || height == 0 {
        return Ok(output);
    }

    let layout = DecodeLayout {
        width,
        row_stride,
        row_bytes,
        bytes_per_pixel,
    };
    let scales = [
        ChannelScale::new(masks.red),
        ChannelScale::new(masks.green),
        ChannelScale::new(masks.blue),
    ];

    // The byte order is constant for the frame, so the branch is resolved once
    // here and each variant is monomorphized into its own tight loop.
    match byte_order {
        ImageByteOrder::LeastSignificantFirst => {
            decode_rows(&mut output, data, &layout, scales, read_pixel_le)?;
        }
        ImageByteOrder::MostSignificantFirst => {
            decode_rows(&mut output, data, &layout, scales, read_pixel_be)?;
        }
    }
    Ok(output)
}

fn decode_rows<F>(
    output: &mut [u8],
    data: &[u8],
    layout: &DecodeLayout,
    scales: [ChannelScale; 3],
    read: F,
) -> Result<(), CaptureError>
where
    F: Fn(&[u8]) -> u32,
{
    for (y, output_row) in output.chunks_exact_mut(layout.width * 4).enumerate() {
        let row_start = y
            .checked_mul(layout.row_stride)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let row_end = row_start
            .checked_add(layout.row_bytes)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let source_row = data
            .get(row_start..row_end)
            .ok_or_else(|| protocol_error("X11 pixel row is truncated"))?;

        // Paired chunk walk: the source and destination offsets advance with the
        // iterators instead of being recomputed per pixel.
        for (source, target) in source_row
            .chunks_exact(layout.bytes_per_pixel)
            .zip(output_row.chunks_exact_mut(4))
        {
            let pixel = read(source);
            target[0] = scales[0].apply(pixel);
            target[1] = scales[1].apply(pixel);
            target[2] = scales[2].apply(pixel);
            target[3] = 255;
        }
    }
    Ok(())
}

/// Reads one little-endian pixel of one to four bytes.
fn read_pixel_le(bytes: &[u8]) -> u32 {
    match *bytes {
        [b0, b1, b2, b3] => u32::from_le_bytes([b0, b1, b2, b3]),
        [b0, b1, b2] => u32::from_le_bytes([b0, b1, b2, 0]),
        [b0, b1] => u32::from_le_bytes([b0, b1, 0, 0]),
        [b0] => u32::from(b0),
        _ => 0,
    }
}

/// Reads one big-endian pixel of one to four bytes.
fn read_pixel_be(bytes: &[u8]) -> u32 {
    match *bytes {
        [b0, b1, b2, b3] => u32::from_be_bytes([b0, b1, b2, b3]),
        [b0, b1, b2] => u32::from_be_bytes([0, b0, b1, b2]),
        [b0, b1] => u32::from_be_bytes([0, 0, b0, b1]),
        [b0] => u32::from(b0),
        _ => 0,
    }
}

/// Scales one masked channel, exposed so tests can assert the mask contract.
#[cfg(test)]
pub(crate) fn scale_channel(pixel: u32, mask: u32) -> u8 {
    ChannelScale::new(mask).apply(pixel)
}

pub(crate) fn packed_row_bytes(width: usize, bits_per_pixel: u8) -> Result<usize, CaptureError> {
    width
        .checked_mul(usize::from(bits_per_pixel / 8))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })
}

pub(crate) fn padded_row_bytes(row_bytes: usize, scanline_pad: u8) -> Result<usize, CaptureError> {
    let pad = usize::from(scanline_pad / 8);
    if pad == 0 {
        return Err(protocol_error("X11 scanline padding is zero"));
    }
    let rounded = row_bytes
        .checked_add(pad - 1)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    Ok(rounded / pad * pad)
}
