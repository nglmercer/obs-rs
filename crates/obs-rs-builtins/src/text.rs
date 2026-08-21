use crate::{portable::parse_color, portable::parse_format, TEXT_SOURCE_KIND};
use obs_rs_config::Config;
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

/// The largest UTF-8 document accepted by the portable text source.
const MAX_TEXT_BYTES: usize = 4_096;
const DEFAULT_FONT_SIZE: &str = "24";
const MAX_FONT_SIZE: u32 = 128;

pub(crate) struct TextSourceFactory {
    kind: Identifier,
}

impl TextSourceFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        let kind = Identifier::new(TEXT_SOURCE_KIND).map_err(PluginError::InvalidIdentifier)?;
        Ok(Self { kind })
    }
}

impl SourceFactory for TextSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }
        let format = parse_format(settings)?;
        let text = parse_text(settings)?;
        let color = parse_color(settings.get("color").unwrap_or("#FFFFFFFF"))?;
        let font_size = parse_font_size(settings)?;
        let frame = render_text(format, &text, color, font_size)?;
        Ok(Box::new(TextSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            format,
            frame,
        }))
    }
}

struct TextSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    frame: VideoFrame,
}

impl Source for TextSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let text = parse_text(settings)?;
        let color = parse_color(settings.get("color").unwrap_or("#FFFFFFFF"))?;
        let font_size = parse_font_size(settings)?;
        let frame = render_text(format, &text, color, font_size)?;
        self.format = format;
        self.frame = frame;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        Ok(Some(self.frame.at_timestamp(request.timestamp())))
    }
}

fn parse_text(settings: &Config) -> Result<String, SourceError> {
    let text = settings.get("text").unwrap_or("");
    if text.len() > MAX_TEXT_BYTES {
        return Err(SourceError::invalid_setting(
            "text",
            format!("text exceeds the {MAX_TEXT_BYTES}-byte limit"),
        ));
    }
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(SourceError::invalid_setting(
            "text",
            "text contains an unsupported control character",
        ));
    }
    Ok(text.to_owned())
}

fn parse_font_size(settings: &Config) -> Result<u32, SourceError> {
    let value = settings
        .get("font_size")
        .unwrap_or(DEFAULT_FONT_SIZE)
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting("font_size", error.to_string()))?;
    if !(1..=MAX_FONT_SIZE).contains(&value) {
        return Err(SourceError::invalid_setting(
            "font_size",
            format!("font size must be between 1 and {MAX_FONT_SIZE}"),
        ));
    }
    Ok(value)
}

fn render_text(
    format: VideoFormat,
    text: &str,
    color: [u8; 4],
    font_size: u32,
) -> Result<VideoFrame, SourceError> {
    let width = format.width() as usize;
    let height = format.height() as usize;
    let scale = usize::try_from(font_size.saturating_add(6) / 7).unwrap_or(1);
    let cell_width = 6 * scale;
    let line_height = 8 * scale;
    let mut pixels = vec![0_u8; format.rgba_bytes()];
    let mut cursor_x = 0_usize;
    let mut cursor_y = 0_usize;

    for character in text.chars() {
        match character {
            '\r' => continue,
            '\n' => {
                cursor_x = 0;
                cursor_y = cursor_y.saturating_add(line_height);
                continue;
            }
            '\t' => {
                cursor_x = cursor_x.saturating_add(cell_width * 4);
                continue;
            }
            _ => {}
        }
        if cursor_x.saturating_add(5 * scale) > width {
            cursor_x = 0;
            cursor_y = cursor_y.saturating_add(line_height);
        }
        if cursor_y.saturating_add(7 * scale) > height {
            break;
        }
        let rows = glyph_rows(character);
        for (glyph_y, row) in rows.into_iter().enumerate() {
            for glyph_x in 0..5 {
                if row & (1_u8 << (4 - glyph_x)) == 0 {
                    continue;
                }
                for pixel_y in 0..scale {
                    let y = cursor_y + glyph_y * scale + pixel_y;
                    let row_start = y * width * 4;
                    for pixel_x in 0..scale {
                        let x = cursor_x + glyph_x * scale + pixel_x;
                        let offset = row_start + x * 4;
                        pixels[offset..offset + 4].copy_from_slice(&color);
                    }
                }
            }
        }
        cursor_x = cursor_x.saturating_add(cell_width);
    }

    VideoFrame::new(format, Timestamp::ZERO, pixels)
        .map_err(|error| SourceError::invalid_setting("text", error.to_string()))
}

/// Returns a small deterministic 5x7 bitmap. Lowercase text intentionally
/// shares the uppercase glyphs until a font-resource boundary is introduced.
#[allow(
    clippy::too_many_lines,
    reason = "the bounded glyph table is clearer as one auditable source-local mapping"
)]
fn glyph_rows(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b11110, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '?' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00110,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00110, 0b00100,
        ],
        ':' => [
            0b00000, 0b00110, 0b00110, 0b00000, 0b00110, 0b00110, 0b00000,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '_' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111,
        ],
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        '=' => [
            0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000, 0b00000,
        ],
        ' ' => [0; 7],
        _ => [
            0b11111, 0b10001, 0b10101, 0b10001, 0b10101, 0b10001, 0b11111,
        ],
    }
}
