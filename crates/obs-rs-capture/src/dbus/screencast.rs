//! The `org.freedesktop.portal.ScreenCast` handshake.
//!
//! On Wayland a client cannot read the screen directly: the compositor hands
//! out a `PipeWire` node through the desktop portal, after the user has picked
//! what to share in the compositor's own dialog. That dialog is the display
//! picker on Wayland, which is why this module replaces rather than duplicates
//! the X11 monitor list.

use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex, MutexGuard, OnceLock, TryLockError,
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    connection::{Connection, Message},
    value::{options, Value},
};
use crate::{lifecycle::CaptureCancellation, CaptureError};

const PORTAL_SERVICE: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST_INTERFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
const SESSION_INTERFACE: &str = "org.freedesktop.portal.Session";

/// Source type bit for a whole monitor.
const SOURCE_TYPE_MONITOR: u32 = 1;
/// Cursor mode bit for a cursor drawn into the frames.
const CURSOR_MODE_EMBEDDED: u32 = 2;
/// Cursor mode bit for frames without a cursor.
const CURSOR_MODE_HIDDEN: u32 = 1;
/// `persist_mode` 2 keeps the permission until the app revokes it, which is
/// what lets a stored token reopen the same screen without a dialog.
const PERSIST_UNTIL_REVOKED: u32 = 2;

/// The portal dialog waits for a person, so it gets a human-scale timeout.
const USER_RESPONSE_TIMEOUT: Duration = Duration::from_mins(3);

/// Portal handshakes share one compositor-owned dialog. Serializing the
/// handshake prevents a source update and an explicit picker from presenting
/// two dialogs at once.
static HANDSHAKE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Distinguishes this process's request paths from any other client's.
static REQUEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A live screen-cast session and the `PipeWire` node it produced.
///
/// Dropping this closes the portal session, which stops the compositor's
/// stream, so the value must be held for as long as frames are wanted.
pub struct ScreenCastSession {
    connection: Connection,
    session_handle: String,
    node_id: u32,
    width: u32,
    height: u32,
    /// Token that reopens this exact selection without prompting again.
    restore_token: Option<String>,
    closed: bool,
}

impl ScreenCastSession {
    /// Returns the `PipeWire` node the compositor is streaming into.
    #[must_use]
    pub const fn node_id(&self) -> u32 {
        self.node_id
    }

    /// Returns the stream size the portal reported, if it reported one.
    #[must_use]
    pub const fn size(&self) -> Option<(u32, u32)> {
        if self.width == 0 || self.height == 0 {
            None
        } else {
            Some((self.width, self.height))
        }
    }

    /// Returns the token that reopens this selection without a dialog.
    #[must_use]
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// Closes the portal session.
    ///
    /// Called from [`Drop`], and safe to call more than once.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        close_portal_session(&mut self.connection, &self.session_handle);
    }
}

impl Drop for ScreenCastSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// How the user should be asked which screen to share.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorMode {
    /// Draw the pointer into the captured frames, as OBS does by default.
    Embedded,
    /// Capture without a pointer.
    Hidden,
}

impl CursorMode {
    const fn bit(self) -> u32 {
        match self {
            Self::Embedded => CURSOR_MODE_EMBEDDED,
            Self::Hidden => CURSOR_MODE_HIDDEN,
        }
    }
}

/// Opens a screen-cast session through the desktop portal.
///
/// When `restore_token` names a previous selection the compositor may reuse it
/// and skip the dialog; otherwise the user is asked which screen to share.
///
/// # Errors
///
/// Returns [`CaptureError::PlatformUnavailable`] when no portal is reachable
/// or the user cancels, and [`CaptureError::Protocol`] when a reply cannot be
/// decoded.
pub fn open_screencast(
    restore_token: Option<&str>,
    cursor: CursorMode,
) -> Result<ScreenCastSession, CaptureError> {
    open_screencast_cancellable(restore_token, cursor, &CaptureCancellation::new())
}

/// Opens a screen-cast session and closes any pending portal request when the
/// owning asynchronous source is cancelled.
///
/// # Errors
///
/// Returns a portal or cancellation error when the session cannot be created,
/// the user cancels the request, or the portal response is invalid.
#[allow(
    clippy::too_many_lines,
    reason = "the portal handshake remains one cancellable transaction"
)]
pub fn open_screencast_cancellable(
    restore_token: Option<&str>,
    cursor: CursorMode,
    cancelled: &CaptureCancellation,
) -> Result<ScreenCastSession, CaptureError> {
    let _handshake = acquire_handshake_lock(cancelled)?;
    if cancelled.is_cancelled() {
        return Err(CaptureError::NotRunning);
    }
    let mut connection = Connection::session()?;
    let sender = escaped_sender(connection.unique_name());

    let session_token = next_token("session");
    let results = request(
        &mut connection,
        &sender,
        SCREENCAST_INTERFACE,
        "CreateSession",
        cancelled,
        |token| {
            vec![options([
                ("handle_token", Value::Str(token.to_owned())),
                ("session_handle_token", Value::Str(session_token.clone())),
            ])]
        },
    )?;
    let session_handle = results
        .get("session_handle")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol("the portal returned no session handle"))?
        .to_owned();

    let mut select = vec![
        ("types", Value::Uint32(SOURCE_TYPE_MONITOR)),
        ("multiple", Value::Bool(false)),
        ("cursor_mode", Value::Uint32(cursor.bit())),
        ("persist_mode", Value::Uint32(PERSIST_UNTIL_REVOKED)),
    ];
    if let Some(token) = restore_token.filter(|token| !token.trim().is_empty()) {
        select.push(("restore_token", Value::Str(token.to_owned())));
    }
    let _results = match request(
        &mut connection,
        &sender,
        SCREENCAST_INTERFACE,
        "SelectSources",
        cancelled,
        |token| {
            let mut entries = select
                .iter()
                .map(|(key, value)| (*key, value.clone()))
                .collect::<Vec<_>>();
            entries.push(("handle_token", Value::Str(token.to_owned())));
            vec![
                Value::ObjectPath(session_handle.clone()),
                dictionary(entries),
            ]
        },
    ) {
        Ok(results) => results,
        Err(error) => {
            close_portal_session(&mut connection, &session_handle);
            return Err(error);
        }
    };
    if cancelled.is_cancelled() {
        close_portal_session(&mut connection, &session_handle);
        return Err(CaptureError::NotRunning);
    }

    let results = match request(
        &mut connection,
        &sender,
        SCREENCAST_INTERFACE,
        "Start",
        cancelled,
        |token| {
            vec![
                Value::ObjectPath(session_handle.clone()),
                // An empty parent window lets the compositor place the dialog.
                Value::Str(String::new()),
                options([("handle_token", Value::Str(token.to_owned()))]),
            ]
        },
    ) {
        Ok(results) => results,
        Err(error) => {
            close_portal_session(&mut connection, &session_handle);
            return Err(error);
        }
    };
    let (node_id, width, height) = match first_stream(&results) {
        Ok(stream) => stream,
        Err(error) => {
            close_portal_session(&mut connection, &session_handle);
            return Err(error);
        }
    };
    Ok(ScreenCastSession {
        connection,
        session_handle,
        node_id,
        width,
        height,
        restore_token: results
            .get("restore_token")
            .and_then(Value::as_str)
            .map(str::to_owned),
        closed: false,
    })
}

/// Portal results are keyed strings; this is the decoded `results` map.
type Results = std::collections::BTreeMap<String, Value>;

fn close_portal_session(connection: &mut Connection, session_handle: &str) {
    let _ = connection.call_no_reply(
        PORTAL_SERVICE,
        session_handle,
        SESSION_INTERFACE,
        "Close",
        &[],
    );
}

/// Runs one portal method and waits for the `Response` signal it answers with.
///
/// The portal replies to the method immediately with the object path of a
/// request, and delivers the real answer later as a signal on that path. The
/// path is predictable from the caller's bus name and the token, so it is
/// computed up front rather than parsed out of the racy method reply.
fn request(
    connection: &mut Connection,
    sender: &str,
    interface: &str,
    member: &str,
    cancelled: &CaptureCancellation,
    arguments: impl Fn(&str) -> Vec<Value>,
) -> Result<Results, CaptureError> {
    if cancelled.is_cancelled() {
        return Err(CaptureError::NotRunning);
    }
    let token = next_token(member);
    let expected = format!("{PORTAL_PATH}/request/{sender}/{token}");
    connection.call(
        PORTAL_SERVICE,
        PORTAL_PATH,
        interface,
        member,
        &arguments(&token),
    )?;
    let response = wait_for_response(connection, &expected, cancelled)?;
    let code = response
        .body
        .first()
        .and_then(Value::as_u32)
        .ok_or_else(|| protocol(format!("{member} returned no response code")))?;
    match code {
        0 => {}
        1 => {
            return Err(CaptureError::PermissionDenied);
        }
        _ => {
            return Err(CaptureError::PlatformUnavailable {
                message: format!("the desktop portal could not complete {member}"),
            })
        }
    }
    Ok(response
        .body
        .get(1)
        .and_then(Value::as_dict)
        .cloned()
        .unwrap_or_default())
}

/// Waits for a portal response in short slices so cancellation can close the
/// request instead of leaving the compositor dialog behind for three minutes.
fn wait_for_response(
    connection: &mut Connection,
    expected: &str,
    cancelled: &CaptureCancellation,
) -> Result<Message, CaptureError> {
    let deadline = Instant::now() + USER_RESPONSE_TIMEOUT;
    loop {
        if cancelled.is_cancelled() {
            let _ =
                connection.call_no_reply(PORTAL_SERVICE, expected, REQUEST_INTERFACE, "Close", &[]);
            return Err(CaptureError::NotRunning);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CaptureError::Io {
                message: "the desktop portal did not answer in time".to_owned(),
            });
        }
        match connection.wait(remaining.min(Duration::from_millis(100)), |message| {
            is_response(message, expected)
        }) {
            Ok(message) => return Ok(message),
            Err(CaptureError::Io { .. }) => {}
            Err(error) => return Err(error),
        }
    }
}

fn acquire_handshake_lock(
    cancelled: &CaptureCancellation,
) -> Result<MutexGuard<'static, ()>, CaptureError> {
    let lock = HANDSHAKE_LOCK.get_or_init(|| Mutex::new(()));
    loop {
        if cancelled.is_cancelled() {
            return Err(CaptureError::NotRunning);
        }
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn is_response(message: &Message, path: &str) -> bool {
    message.is_signal()
        && message.interface.as_deref() == Some(REQUEST_INTERFACE)
        && message.member.as_deref() == Some("Response")
        && message.path.as_deref() == Some(path)
}

/// Reads the node ID and size of the first stream in a `Start` result.
fn first_stream(results: &Results) -> Result<(u32, u32, u32), CaptureError> {
    let streams = results
        .get("streams")
        .and_then(Value::as_items)
        .ok_or_else(|| protocol("the portal returned no stream list"))?;
    let stream = streams
        .first()
        .and_then(Value::as_items)
        .ok_or_else(|| protocol("the portal returned an empty stream list"))?;
    let node_id = stream
        .first()
        .and_then(Value::as_u32)
        .ok_or_else(|| protocol("the portal stream has no node ID"))?;
    let size = stream
        .get(1)
        .and_then(Value::as_dict)
        .and_then(|properties| properties.get("size"))
        .and_then(Value::as_items)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    let extent = |index: usize| match size.get(index) {
        Some(Value::Int32(value)) => u32::try_from(*value).unwrap_or(0),
        Some(Value::Uint32(value)) => *value,
        _ => 0,
    };
    Ok((node_id, extent(0), extent(1)))
}

/// Builds an `a{sv}` from a runtime-sized list of entries.
fn dictionary(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.into_variant()))
            .collect(),
    )
}

/// Rewrites `:1.42` into the `1_42` form the portal uses in request paths.
fn escaped_sender(unique_name: &str) -> String {
    unique_name
        .trim_start_matches(':')
        .chars()
        .map(|character| if character == '.' { '_' } else { character })
        .collect()
}

/// Returns a token that is unique within this process.
fn next_token(prefix: &str) -> String {
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let prefix = prefix
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_lowercase();
    format!("obsrs_{prefix}_{counter}")
}

fn protocol(message: impl Into<String>) -> CaptureError {
    CaptureError::Protocol {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_use_the_escaped_bus_name() {
        assert_eq!(escaped_sender(":1.42"), "1_42");
        assert_eq!(escaped_sender("1.2.3"), "1_2_3");
    }

    #[test]
    fn tokens_are_unique_and_path_safe() {
        let first = next_token("Start");
        let second = next_token("Start");

        assert_ne!(first, second);
        assert!(first.starts_with("obsrs_start_"));
        assert!(first
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '_'));
    }

    #[test]
    fn stream_results_yield_the_node_and_size() {
        let results = Results::from([(
            "streams".to_owned(),
            Value::Array {
                element: "(ua{sv})".to_owned(),
                items: vec![Value::Struct(vec![
                    Value::Uint32(63),
                    options([(
                        "size",
                        Value::Struct(vec![Value::Int32(2560), Value::Int32(1440)]),
                    )]),
                ])],
            },
        )]);

        assert_eq!(first_stream(&results).expect("stream"), (63, 2560, 1440));
    }

    #[test]
    fn a_missing_stream_list_is_a_protocol_error() {
        assert!(matches!(
            first_stream(&Results::new()),
            Err(CaptureError::Protocol { .. })
        ));
    }
}
