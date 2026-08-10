use super::super::CaptureError;
use super::error::protocol_error;

pub(crate) const X11_TRUE_COLOR: u8 = 4;
const X11_DIRECT_COLOR: u8 = 5;
pub(crate) const X11_LOCAL_FAMILY: u16 = 256;
pub(crate) const X11_WILD_FAMILY: u16 = 65_535;
pub(crate) const X11_INTERNET_FAMILY: u16 = 0;

pub(crate) const X11_GET_IMAGE: u8 = 73;
pub(crate) const X11_Z_PIXMAP: u8 = 2;
pub(crate) const X11_MAX_REPLY_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageByteOrder {
    LeastSignificantFirst,
    MostSignificantFirst,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VisualMasks {
    pub(crate) red: u32,
    pub(crate) green: u32,
    pub(crate) blue: u32,
}

pub(crate) struct Authorization {
    pub(crate) name: Vec<u8>,
    pub(crate) data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ServerInfo {
    pub(crate) root: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) depth: u8,
    pub(crate) visual: u32,
    pub(crate) image_byte_order: ImageByteOrder,
    pub(crate) scanline_pad: u8,
    pub(crate) bits_per_pixel: u8,
    pub(crate) masks: VisualMasks,
}

pub(crate) fn setup_request(
    authorization: Option<&Authorization>,
) -> Result<Vec<u8>, CaptureError> {
    let (name, data) = authorization.map_or((&[][..], &[][..]), |value| {
        (value.name.as_slice(), value.data.as_slice())
    });
    let name_len =
        u16::try_from(name.len()).map_err(|_| protocol_error("X11 auth name too long"))?;
    let data_len =
        u16::try_from(data.len()).map_err(|_| protocol_error("X11 auth data too long"))?;
    let mut request = Vec::with_capacity(12 + name.len() + data.len() + 6);
    request.extend_from_slice(b"l\0");
    write_u16_le(&mut request, 11);
    write_u16_le(&mut request, 0);
    write_u16_le(&mut request, name_len);
    write_u16_le(&mut request, data_len);
    write_u16_le(&mut request, 0);
    request.extend_from_slice(name);
    append_padding(&mut request);
    request.extend_from_slice(data);
    append_padding(&mut request);
    Ok(request)
}

pub(crate) fn parse_setup(bytes: &[u8]) -> Result<ServerInfo, CaptureError> {
    if bytes.len() < 40 || bytes[0] != 1 {
        return Err(protocol_error("X11 setup response is truncated"));
    }
    let vendor_len = usize::from(read_u16_le(bytes, 24)?);
    let root_count = usize::from(bytes[28]);
    let format_count = usize::from(bytes[29]);
    let image_byte_order = match bytes[30] {
        0 => ImageByteOrder::LeastSignificantFirst,
        1 => ImageByteOrder::MostSignificantFirst,
        _ => return Err(protocol_error("X11 image byte order is invalid")),
    };
    let mut offset = 40_usize;
    offset = offset
        .checked_add(padded_length(vendor_len)?)
        .ok_or_else(|| protocol_error("X11 vendor field overflows"))?;
    let mut formats = Vec::with_capacity(format_count);
    for _ in 0..format_count {
        ensure_range(bytes, offset, 8)?;
        let depth = bytes[offset];
        let bits_per_pixel = bytes[offset + 1];
        let scanline_pad = bytes[offset + 2];
        formats.push((depth, bits_per_pixel, scanline_pad));
        offset += 8;
    }

    let mut root_info = None;
    for root_index in 0..root_count {
        ensure_range(bytes, offset, 40)?;
        let root = read_u32_le(bytes, offset)?;
        let width = u32::from(read_u16_le(bytes, offset + 20)?);
        let height = u32::from(read_u16_le(bytes, offset + 22)?);
        let visual = read_u32_le(bytes, offset + 32)?;
        let depth = bytes[offset + 38];
        let allowed_depths = usize::from(bytes[offset + 39]);
        offset += 40;
        let mut masks = None;
        for _ in 0..allowed_depths {
            ensure_range(bytes, offset, 8)?;
            let visual_depth = bytes[offset];
            let visual_count = usize::from(read_u16_le(bytes, offset + 2)?);
            offset += 8;
            let visual_bytes = visual_count
                .checked_mul(24)
                .ok_or_else(|| protocol_error("X11 visual list overflows"))?;
            ensure_range(bytes, offset, visual_bytes)?;
            for visual_index in 0..visual_count {
                let visual_offset = offset + visual_index * 24;
                let visual_id = read_u32_le(bytes, visual_offset)?;
                let class = bytes[visual_offset + 4];
                if visual_depth == depth
                    && visual_id == visual
                    && matches!(class, X11_TRUE_COLOR | X11_DIRECT_COLOR)
                {
                    masks = Some(VisualMasks {
                        red: read_u32_le(bytes, visual_offset + 8)?,
                        green: read_u32_le(bytes, visual_offset + 12)?,
                        blue: read_u32_le(bytes, visual_offset + 16)?,
                    });
                }
            }
            offset += visual_bytes;
        }
        if root_index == 0 {
            root_info = Some((root, width, height, depth, visual, masks));
        }
    }

    let (root, width, height, depth, visual, masks) =
        root_info.ok_or_else(|| protocol_error("X11 setup contains no root screen"))?;
    let (_, bits_per_pixel, scanline_pad) = formats
        .into_iter()
        .find(|(format_depth, _, _)| *format_depth == depth)
        .ok_or_else(|| protocol_error("X11 setup has no root-depth pixmap format"))?;
    if !matches!(bits_per_pixel, 16 | 24 | 32) {
        return Err(protocol_error(format!(
            "X11 root visual uses an unsupported pixel size: {bits_per_pixel} bits per pixel"
        )));
    }
    let masks = masks.ok_or_else(|| protocol_error("X11 root visual is not TrueColor"))?;
    if masks.red == 0 || masks.green == 0 || masks.blue == 0 {
        return Err(protocol_error("X11 root visual has an empty color mask"));
    }
    Ok(ServerInfo {
        root,
        width,
        height,
        depth,
        visual,
        image_byte_order,
        scanline_pad,
        bits_per_pixel,
        masks,
    })
}

fn padded_length(length: usize) -> Result<usize, CaptureError> {
    length
        .checked_add(3)
        .map(|value| value / 4 * 4)
        .ok_or_else(|| protocol_error("X11 setup length overflows"))
}

fn ensure_range(bytes: &[u8], offset: usize, length: usize) -> Result<(), CaptureError> {
    offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .map(|_| ())
        .ok_or_else(|| protocol_error("X11 setup response is truncated"))
}

fn append_padding(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

pub(crate) fn write_u16_le(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}


pub(crate) fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, CaptureError> {
    ensure_range(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

pub(crate) fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, CaptureError> {
    ensure_range(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

pub(crate) fn read_be_field(bytes: &[u8], offset: &mut usize) -> Option<u16> {
    let start = *offset;
    let end = start.checked_add(2)?;
    let value = u16::from_be_bytes(bytes.get(start..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

pub(crate) fn read_bytes_field<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let length = usize::from(read_be_field(bytes, offset)?);
    let start = *offset;
    let end = start.checked_add(length)?;
    let value = bytes.get(start..end)?;
    *offset = end;
    Some(value)
}
