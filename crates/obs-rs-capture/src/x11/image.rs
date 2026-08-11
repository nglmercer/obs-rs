use super::super::CaptureError;
use super::{
    error::protocol_error,
    protocol::{ImageByteOrder, VisualMasks},
};

/// Geometry of one X11 image reply, resolved once before the decode loop.
struct DecodeLayout {
    height: usize,
    row_stride: usize,
    row_bytes: usize,
    bytes_per_pixel: usize,
}

/// A channel mask with its shift and range resolved once per frame.
///
/// `mask.trailing_zeros()` and the derived maximum are constant for the whole
/// frame, so they are computed here instead of per channel per pixel.
#[derive(Clone)]
struct ChannelScale {
    mask: u32,
    shift: u32,
    maximum: u64,
    lookup: Option<[u8; 256]>,
}

impl ChannelScale {
    fn new(mask: u32) -> Self {
        if mask == 0 {
            return Self {
                mask: 0,
                shift: 0,
                maximum: 1,
                lookup: None,
            };
        }
        let shift = mask.trailing_zeros();
        let maximum = u64::from(mask >> shift).max(1);
        let lookup = if u8::try_from(maximum).is_ok() {
            let mut table = [0_u8; 256];
            for (value, output) in table.iter_mut().enumerate() {
                *output = u8::try_from(
                    (u64::try_from(value).unwrap_or(0) * u64::from(u8::MAX)) / maximum,
                )
                .unwrap_or(u8::MAX);
            }
            Some(table)
        } else {
            None
        };
        Self {
            mask,
            shift,
            maximum,
            lookup,
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "min constrains the value to 0..=255, so the cast is exact"
    )]
    fn apply(&self, pixel: u32) -> u8 {
        if self.mask == 0 {
            return 0;
        }
        let value = u64::from((pixel & self.mask) >> self.shift);
        if let Some(table) = self.lookup.as_ref() {
            return table[usize::try_from(value).unwrap_or(usize::from(u8::MAX))];
        }
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
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let mut output = Vec::with_capacity(pixel_bytes);
    decode_pixels_into(
        width,
        height,
        row_stride,
        bits_per_pixel,
        byte_order,
        masks,
        data,
        &mut output,
    )?;
    Ok(output)
}

#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the X11 image decode boundary"
)]
pub(crate) fn decode_pixels_into(
    width: usize,
    height: usize,
    row_stride: usize,
    bits_per_pixel: u8,
    byte_order: ImageByteOrder,
    masks: VisualMasks,
    data: &[u8],
    output: &mut Vec<u8>,
) -> Result<(), CaptureError> {
    let bytes_per_pixel = usize::from(bits_per_pixel / 8);
    if bytes_per_pixel == 0 || bytes_per_pixel > 4 {
        return Err(protocol_error("X11 image has an unsupported pixel size"));
    }
    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    output.clear();
    output.reserve(pixel_bytes);
    if width == 0 || height == 0 {
        return Ok(());
    }

    let layout = DecodeLayout {
        height,
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
            decode_rows(output, data, &layout, &scales, read_pixel_le)?;
        }
        ImageByteOrder::MostSignificantFirst => {
            decode_rows(output, data, &layout, &scales, read_pixel_be)?;
        }
    }
    Ok(())
}

fn decode_rows<F>(
    output: &mut Vec<u8>,
    data: &[u8],
    layout: &DecodeLayout,
    scales: &[ChannelScale; 3],
    read: F,
) -> Result<(), CaptureError>
where
    F: Fn(&[u8]) -> u32,
{
    for y in 0..layout.height {
        let row_start = y
            .checked_mul(layout.row_stride)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let row_end = row_start
            .checked_add(layout.row_bytes)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let source_row = data
            .get(row_start..row_end)
            .ok_or_else(|| protocol_error("X11 pixel row is truncated"))?;

        for source in source_row.chunks_exact(layout.bytes_per_pixel) {
            let pixel = read(source);
            output.extend_from_slice(&[
                scales[0].apply(pixel),
                scales[1].apply(pixel),
                scales[2].apply(pixel),
                255,
            ]);
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
