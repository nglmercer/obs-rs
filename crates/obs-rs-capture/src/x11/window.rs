//! X11 window enumeration and geometry tracking over the same safe socket.
//!
//! A window is captured by reading the region of the root window it currently
//! occupies rather than by reading the window drawable directly. That choice is
//! deliberate: the root image is the one whose depth, visual, and colour masks
//! the connection already negotiated and the decoder is already verified
//! against, whereas a client window may use a different visual entirely. The
//! cost is that a window covered by another window captures whatever is on top;
//! the benefit is that every window on every server decodes correctly.
//!
//! Because the region is re-read from the server on every frame, moving or
//! resizing a window is tracked without the caller doing anything, and a window
//! that has been destroyed surfaces as a typed error on the next frame instead
//! of silently capturing the wrong pixels.

use std::{io::Write, os::unix::net::UnixStream};

use super::super::CaptureError;
use super::{
    connection::{display_socket, handshake, read_authorization},
    error::{platform_error, protocol_error, read_exact_x11, x11_io_error},
    protocol::{read_u16_le, read_u32_le, ServerInfo},
};

/// Core-protocol opcodes used by window discovery.
const X11_GET_GEOMETRY: u8 = 14;
const X11_INTERN_ATOM: u8 = 16;
const X11_QUERY_TREE: u8 = 15;
const X11_GET_PROPERTY: u8 = 20;
const X11_TRANSLATE_COORDINATES: u8 = 40;

/// Predefined atoms from the core protocol, which never need interning.
const ATOM_ATOM: u32 = 4;
const ATOM_STRING: u32 = 31;
const ATOM_WM_NAME: u32 = 39;

/// Longest property payload accepted, in 4-byte units.
///
/// A client list is a few hundred window IDs at most, and a title is a short
/// string; a server that claims otherwise is not worth allocating for.
const MAX_PROPERTY_WORDS: u32 = 1_024;

/// Longest window title kept, in bytes.
const MAX_TITLE_BYTES: usize = 160;

/// Most windows returned by one enumeration.
///
/// Discovery walks every child of the root on servers without EWMH, and a
/// runaway client should not be able to make the picker unbounded.
const MAX_WINDOWS: usize = 256;

/// One capturable top-level window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct X11Window {
    id: u32,
    title: String,
    width: u32,
    height: u32,
}

impl X11Window {
    /// Creates a window descriptor.
    #[must_use]
    pub const fn new(id: u32, title: String, width: u32, height: u32) -> Self {
        Self {
            id,
            title,
            width,
            height,
        }
    }

    /// Returns the X11 window resource ID.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Returns the window's reported title, or a placeholder when it has none.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the window's current width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the window's current height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the stable capture-device ID stored in a project.
    ///
    /// The resource ID is the only stable handle X11 offers, and it is written
    /// in hexadecimal because that is how every other X11 tool prints it.
    #[must_use]
    pub fn device_id(&self) -> String {
        format!("x11-window-{:08x}", self.id)
    }

    /// Returns the label a device catalog or picker shows.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} ({}x{})", self.title, self.width, self.height)
    }
}

/// Parses a stored device ID back into a window resource ID.
///
/// Accepts both the `x11-window-<hex>` form written by [`X11Window::device_id`]
/// and a bare decimal or `0x`-prefixed ID, because the latter is what a user
/// copying from `xwininfo` will have to hand.
#[must_use]
pub fn parse_window_id(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("x11-window-") {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u32::from_str_radix(hex, 16).ok();
    }
    value.parse().ok()
}

/// Lists the capturable top-level windows of `display`.
///
/// # Errors
///
/// Returns [`CaptureError::PlatformUnavailable`] when the display cannot be
/// reached, or [`CaptureError::Protocol`] when a reply cannot be decoded.
pub fn x11_windows(display: &str) -> Result<Vec<X11Window>, CaptureError> {
    let (socket_path, display_number) = display_socket(display)?;
    let authorization = read_authorization(&display_number);
    let mut stream =
        UnixStream::connect(&socket_path).map_err(|error| CaptureError::PlatformUnavailable {
            message: format!("connect to {}: {error}", socket_path.display()),
        })?;
    let server = handshake(&mut stream, authorization.as_ref())?;
    enumerate_windows(&mut stream, &server)
}

/// Collects the top-level windows a server is currently showing.
///
/// EWMH's `_NET_CLIENT_LIST` is preferred because it is the window manager's
/// own list of application windows, already filtered of panels, docks, and
/// override-redirect popups. A server or window manager without it falls back
/// to walking the root's children, which returns the same windows plus some
/// decoration frames — a longer list is better than an empty picker.
pub(crate) fn enumerate_windows(
    stream: &mut UnixStream,
    server: &ServerInfo,
) -> Result<Vec<X11Window>, CaptureError> {
    let candidates = client_list(stream, server)?;
    let mut windows = Vec::new();
    for window in candidates.into_iter().take(MAX_WINDOWS) {
        // A window that vanished between the list and the query is not an
        // error: enumeration races with the user closing something.
        let Ok((width, height)) = window_size(stream, window) else {
            continue;
        };
        if width == 0 || height == 0 {
            continue;
        }
        let title = window_title(stream, window)
            .unwrap_or_default()
            .unwrap_or_else(|| format!("Window 0x{window:08x}"));
        windows.push(X11Window::new(window, title, width, height));
    }
    // A stable order keeps the picker from reshuffling between openings; the
    // server's own order follows the stacking, which changes constantly.
    windows.sort_by(|left, right| left.title.cmp(&right.title).then(left.id.cmp(&right.id)));
    Ok(windows)
}

fn client_list(stream: &mut UnixStream, server: &ServerInfo) -> Result<Vec<u32>, CaptureError> {
    if let Some(atom) = intern_atom(stream, "_NET_CLIENT_LIST")? {
        let property = get_property(stream, server.root, atom, ATOM_ATOM)?;
        let windows = property
            .chunks_exact(4)
            .filter_map(|word| read_u32_le(word, 0).ok())
            .collect::<Vec<_>>();
        if !windows.is_empty() {
            return Ok(windows);
        }
    }
    query_tree(stream, server.root)
}

/// Returns the current size of one window, tracking moves and resizes.
///
/// # Errors
///
/// Returns [`CaptureError::Protocol`] when the window no longer exists, which
/// is what a caller treats as "the captured window was closed".
pub(crate) fn window_size(
    stream: &mut UnixStream,
    window: u32,
) -> Result<(u32, u32), CaptureError> {
    let reply = geometry(stream, window)?;
    Ok((
        u32::from(read_u16_le(&reply, 16)?),
        u32::from(read_u16_le(&reply, 18)?),
    ))
}

/// Returns the window's rectangle in root coordinates.
///
/// The window's own geometry is relative to its parent, which under a
/// reparenting window manager is the decoration frame rather than the root, so
/// the origin is translated rather than used directly.
///
/// # Errors
///
/// Returns [`CaptureError::Protocol`] when the window no longer exists.
pub(crate) fn window_root_rect(
    stream: &mut UnixStream,
    window: u32,
    root: u32,
) -> Result<(i32, i32, u32, u32), CaptureError> {
    let (width, height) = window_size(stream, window)?;
    let mut request = [0_u8; 16];
    request[0] = X11_TRANSLATE_COORDINATES;
    request[2..4].copy_from_slice(&4_u16.to_le_bytes());
    request[4..8].copy_from_slice(&window.to_le_bytes());
    request[8..12].copy_from_slice(&root.to_le_bytes());
    // The window's own origin, translated into the root's coordinate space.
    request[12..14].copy_from_slice(&0_i16.to_le_bytes());
    request[14..16].copy_from_slice(&0_i16.to_le_bytes());
    let reply = request_reply(stream, &request, "TranslateCoordinates")?;
    // Root coordinates are signed: a window can start off the left or top edge.
    let x = i32::from(read_u16_le(&reply, 12)?.cast_signed());
    let y = i32::from(read_u16_le(&reply, 14)?.cast_signed());
    Ok((x, y, width, height))
}

fn geometry(stream: &mut UnixStream, drawable: u32) -> Result<[u8; 32], CaptureError> {
    let mut request = [0_u8; 8];
    request[0] = X11_GET_GEOMETRY;
    request[2..4].copy_from_slice(&2_u16.to_le_bytes());
    request[4..8].copy_from_slice(&drawable.to_le_bytes());
    request_reply(stream, &request, "GetGeometry")
}

fn query_tree(stream: &mut UnixStream, root: u32) -> Result<Vec<u32>, CaptureError> {
    let mut request = [0_u8; 8];
    request[0] = X11_QUERY_TREE;
    request[2..4].copy_from_slice(&2_u16.to_le_bytes());
    request[4..8].copy_from_slice(&root.to_le_bytes());
    let reply = request_reply(stream, &request, "QueryTree")?;
    let payload = read_payload(stream, &reply)?;
    Ok(payload
        .chunks_exact(4)
        .filter_map(|word| read_u32_le(word, 0).ok())
        .collect())
}

fn window_title(stream: &mut UnixStream, window: u32) -> Result<Option<String>, CaptureError> {
    // `_NET_WM_NAME` is UTF-8 and is what a modern client sets; `WM_NAME` is
    // the Latin-1 fallback older clients still use.
    if let Some(utf8) = intern_atom(stream, "UTF8_STRING")? {
        if let Some(atom) = intern_atom(stream, "_NET_WM_NAME")? {
            let value = get_property(stream, window, atom, utf8)?;
            if let Some(title) = decode_title(&value) {
                return Ok(Some(title));
            }
        }
    }
    let value = get_property(stream, window, ATOM_WM_NAME, ATOM_STRING)?;
    Ok(decode_title(&value))
}

/// Turns a raw property payload into a bounded, single-line title.
fn decode_title(value: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(value);
    let text = text
        .trim_matches('\0')
        .trim()
        // A title containing a newline would break every single-line list it
        // is shown in, so control characters are replaced rather than kept.
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    Some(truncate_chars(&text, MAX_TITLE_BYTES))
}

/// Truncates on a character boundary so a multi-byte title never splits.
fn truncate_chars(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

/// Resolves an atom name, returning `None` when the server does not know it.
fn intern_atom(stream: &mut UnixStream, name: &str) -> Result<Option<u32>, CaptureError> {
    let name = name.as_bytes();
    let name_length =
        u16::try_from(name.len()).map_err(|_| protocol_error("X11 atom name is too long"))?;
    let padded = name.len().div_ceil(4) * 4;
    let mut request = Vec::with_capacity(8 + padded);
    request.push(X11_INTERN_ATOM);
    // `only_if_exists` keeps discovery read-only: it never adds an atom to a
    // server that does not already support the extension being probed.
    request.push(1);
    let words = u16::try_from(2 + padded / 4)
        .map_err(|_| protocol_error("X11 InternAtom request is too long"))?;
    request.extend_from_slice(&words.to_le_bytes());
    request.extend_from_slice(&name_length.to_le_bytes());
    request.extend_from_slice(&0_u16.to_le_bytes());
    request.extend_from_slice(name);
    request.resize(8 + padded, 0);
    let reply = request_reply(stream, &request, "InternAtom")?;
    let atom = read_u32_le(&reply, 8)?;
    Ok((atom != 0).then_some(atom))
}

/// Reads one window property, returning an empty payload when it is unset.
fn get_property(
    stream: &mut UnixStream,
    window: u32,
    property: u32,
    kind: u32,
) -> Result<Vec<u8>, CaptureError> {
    let mut request = [0_u8; 24];
    request[0] = X11_GET_PROPERTY;
    // `delete` stays zero: reading a property must not consume it from under
    // the window manager that owns it.
    request[2..4].copy_from_slice(&6_u16.to_le_bytes());
    request[4..8].copy_from_slice(&window.to_le_bytes());
    request[8..12].copy_from_slice(&property.to_le_bytes());
    request[12..16].copy_from_slice(&kind.to_le_bytes());
    request[16..20].copy_from_slice(&0_u32.to_le_bytes());
    request[20..24].copy_from_slice(&MAX_PROPERTY_WORDS.to_le_bytes());
    let Ok(reply) = request_reply(stream, &request, "GetProperty") else {
        // An unset property on a live window is ordinary, not a failure.
        return Ok(Vec::new());
    };
    read_payload(stream, &reply)
}

/// Reads the variable-length payload that follows a 32-byte reply header.
fn read_payload(stream: &mut UnixStream, reply: &[u8; 32]) -> Result<Vec<u8>, CaptureError> {
    let words = read_u32_le(reply, 4)?;
    if words > MAX_PROPERTY_WORDS {
        return Err(CaptureError::ReplyTooLarge {
            bytes: u64::from(words) * 4,
        });
    }
    let bytes = usize::try_from(words)
        .ok()
        .and_then(|words| words.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge {
            bytes: u64::from(words) * 4,
        })?;
    let mut payload = vec![0_u8; bytes];
    read_exact_x11(stream, &mut payload)?;
    Ok(payload)
}

/// Sends one request and returns its 32-byte reply header.
///
/// An X11 error reply is turned into a typed protocol error naming the request,
/// which is what makes "the captured window was destroyed" legible rather than
/// an anonymous protocol failure.
fn request_reply(
    stream: &mut UnixStream,
    request: &[u8],
    name: &'static str,
) -> Result<[u8; 32], CaptureError> {
    stream
        .write_all(request)
        .map_err(|error| x11_io_error(&error))?;
    let mut reply = [0_u8; 32];
    read_exact_x11(stream, &mut reply)?;
    if reply[0] != 1 {
        return Err(platform_error(format!(
            "X11 {name} returned error code {}",
            reply[1]
        )));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_id_round_trips_through_the_parser() {
        let window = X11Window::new(0x0140_0007, "Terminal".to_owned(), 800, 600);

        assert_eq!(window.device_id(), "x11-window-01400007");
        assert_eq!(parse_window_id(&window.device_id()), Some(0x0140_0007));
    }

    #[test]
    fn a_window_id_is_accepted_in_the_forms_other_x11_tools_print() {
        // `xwininfo` prints hexadecimal; a script may well pass decimal.
        assert_eq!(parse_window_id("0x1400007"), Some(0x0140_0007));
        assert_eq!(parse_window_id("0X1400007"), Some(0x0140_0007));
        assert_eq!(parse_window_id("20971527"), Some(20_971_527));
        assert_eq!(parse_window_id("  0x1400007  "), Some(0x0140_0007));
        assert_eq!(parse_window_id("not-a-window"), None);
        assert_eq!(parse_window_id(""), None);
    }

    #[test]
    fn a_title_is_trimmed_of_padding_and_control_characters() {
        assert_eq!(
            decode_title(b"Firefox\0").as_deref(),
            Some("Firefox"),
            "a NUL-terminated property must not keep its terminator"
        );
        assert_eq!(
            decode_title(b"Editor\n\tmain.rs").as_deref(),
            Some("Editor  main.rs"),
            "a newline would break every single-line list this is shown in"
        );
        assert_eq!(decode_title(b"   ").as_deref(), None);
        assert_eq!(decode_title(b"").as_deref(), None);
    }

    #[test]
    fn a_long_title_is_truncated_on_a_character_boundary() {
        // Three-byte characters cannot be cut at an arbitrary byte offset.
        let title = "★".repeat(MAX_TITLE_BYTES);

        let decoded = decode_title(title.as_bytes()).expect("a long title is still usable");

        assert!(decoded.len() <= MAX_TITLE_BYTES);
        assert!(
            decoded.chars().all(|character| character == '★'),
            "truncation must not leave a partial character behind"
        );
    }

    #[test]
    fn a_window_label_names_its_current_size() {
        let window = X11Window::new(7, "Slides".to_owned(), 1920, 1080);

        assert_eq!(window.label(), "Slides (1920x1080)");
        assert_eq!(window.width(), 1920);
        assert_eq!(window.height(), 1080);
        assert_eq!(window.id(), 7);
        assert_eq!(window.title(), "Slides");
    }
}
