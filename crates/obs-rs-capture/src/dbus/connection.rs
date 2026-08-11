//! A minimal session-bus client: SASL EXTERNAL auth, method calls, signals.
//!
//! The desktop portal answers a request with a signal on a per-request object
//! path rather than with the method reply, so the connection has to stay open
//! and readable for the whole handshake. That rules out shelling out to a
//! one-shot D-Bus utility, and is why this client exists.

use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use super::{
    codec::{decode, encode, protocol},
    value::Value,
};
use crate::CaptureError;

/// Message types this client sends and recognizes.
const METHOD_CALL: u8 = 1;
const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;

/// Header field codes, from the D-Bus specification.
const FIELD_PATH: u8 = 1;
const FIELD_INTERFACE: u8 = 2;
const FIELD_MEMBER: u8 = 3;
const FIELD_ERROR_NAME: u8 = 4;
const FIELD_REPLY_SERIAL: u8 = 5;
const FIELD_DESTINATION: u8 = 6;
const FIELD_SIGNATURE: u8 = 8;

/// Refuse to buffer a reply larger than the specification's own limit.
const MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

/// One decoded incoming message.
#[derive(Debug)]
pub(crate) struct Message {
    pub(crate) kind: u8,
    pub(crate) path: Option<String>,
    pub(crate) interface: Option<String>,
    pub(crate) member: Option<String>,
    pub(crate) error_name: Option<String>,
    pub(crate) reply_serial: Option<u32>,
    pub(crate) body: Vec<Value>,
}

impl Message {
    /// Returns whether this message is a broadcast signal rather than a reply.
    pub(crate) fn is_signal(&self) -> bool {
        self.kind == SIGNAL
    }
}

/// A connected session bus client.
pub(crate) struct Connection {
    stream: UnixStream,
    serial: u32,
    /// The unique name the bus assigned, such as `:1.42`.
    unique_name: String,
    /// Messages read while waiting for a different one.
    pending: Vec<Message>,
}

impl Connection {
    /// Connects to the session bus named by `DBUS_SESSION_BUS_ADDRESS`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::PlatformUnavailable`] when no session bus is
    /// reachable, and [`CaptureError::Protocol`] when the handshake fails.
    pub(crate) fn session() -> Result<Self, CaptureError> {
        let address = env::var("DBUS_SESSION_BUS_ADDRESS").map_err(|_| {
            unavailable("DBUS_SESSION_BUS_ADDRESS is not set, so no desktop portal is reachable")
        })?;
        let path = unix_socket_path(&address)?;
        let stream = UnixStream::connect(&path)
            .map_err(|error| unavailable(format!("connect to the session bus {path}: {error}")))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .map_err(|error| unavailable(format!("session bus read timeout: {error}")))?;
        let mut connection = Self {
            stream,
            serial: 0,
            unique_name: String::new(),
            pending: Vec::new(),
        };
        connection.authenticate()?;
        let unique_name = connection
            .call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "Hello",
                &[],
            )?
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| protocol("the bus did not return a unique name"))?
            .to_owned();
        unique_name.clone_into(&mut connection.unique_name);
        Ok(connection)
    }

    /// Returns the unique bus name, used to build portal request paths.
    pub(crate) fn unique_name(&self) -> &str {
        &self.unique_name
    }

    /// SASL EXTERNAL, which the session bus accepts from the owning user.
    fn authenticate(&mut self) -> Result<(), CaptureError> {
        // The leading NUL byte is the transport's own protocol marker.
        self.stream
            .write_all(&[0])
            .map_err(|error| io_error(&error))?;
        let uid = current_uid();
        let mut hex = String::with_capacity(uid.len() * 2);
        for byte in uid.as_bytes() {
            use std::fmt::Write;
            // The user ID is sent as the hex of its decimal text, which is what
            // SASL EXTERNAL expects on a Unix transport.
            let _ = write!(hex, "{byte:02x}");
        }
        self.write_line(&format!("AUTH EXTERNAL {hex}"))?;
        let response = self.read_line()?;
        if !response.starts_with("OK") {
            return Err(protocol(format!(
                "the session bus rejected EXTERNAL authentication: {response}"
            )));
        }
        self.write_line("BEGIN")
    }

    fn write_line(&mut self, line: &str) -> Result<(), CaptureError> {
        self.stream
            .write_all(format!("{line}\r\n").as_bytes())
            .map_err(|error| io_error(&error))
    }

    /// Reads one CRLF-terminated authentication line, one byte at a time.
    ///
    /// The authentication phase is line oriented and must not over-read into
    /// the binary message stream that follows `BEGIN`.
    fn read_line(&mut self) -> Result<String, CaptureError> {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            self.stream
                .read_exact(&mut byte)
                .map_err(|error| io_error(&error))?;
            if byte[0] == b'\n' {
                break;
            }
            if byte[0] != b'\r' {
                line.push(byte[0]);
            }
            if line.len() > 1024 {
                return Err(protocol("session bus authentication line is too long"));
            }
        }
        String::from_utf8(line).map_err(|_| protocol("session bus reply is not UTF-8"))
    }

    /// Sends a method call and waits for its reply.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Protocol`] when the peer answers with an error
    /// message or a malformed reply.
    pub(crate) fn call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        arguments: &[Value],
    ) -> Result<Vec<Value>, CaptureError> {
        let serial = self.send_call(destination, path, interface, member, arguments)?;
        let reply = self.wait(Duration::from_secs(30), |message| {
            message.reply_serial == Some(serial) && matches!(message.kind, METHOD_RETURN | ERROR)
        })?;
        if reply.kind == ERROR {
            let detail = reply
                .body
                .first()
                .and_then(Value::as_str)
                .unwrap_or("no detail");
            return Err(protocol(format!(
                "{member} failed: {} ({detail})",
                reply.error_name.as_deref().unwrap_or("unknown error")
            )));
        }
        Ok(reply.body)
    }

    fn send_call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        arguments: &[Value],
    ) -> Result<u32, CaptureError> {
        self.serial = self.serial.wrapping_add(1).max(1);
        let serial = self.serial;
        let mut body = Vec::new();
        for argument in arguments {
            encode(&mut body, argument)?;
        }
        let signature = arguments
            .iter()
            .map(Value::signature)
            .collect::<Vec<_>>()
            .concat();
        let mut fields = vec![
            header_field(FIELD_PATH, Value::ObjectPath(path.to_owned())),
            header_field(FIELD_DESTINATION, Value::Str(destination.to_owned())),
            header_field(FIELD_INTERFACE, Value::Str(interface.to_owned())),
            header_field(FIELD_MEMBER, Value::Str(member.to_owned())),
        ];
        if !signature.is_empty() {
            fields.push(header_field(FIELD_SIGNATURE, Value::Signature(signature)));
        }

        let mut message = Vec::with_capacity(128 + body.len());
        message.push(b'l');
        message.push(METHOD_CALL);
        // No flags: a reply is expected.
        message.push(0);
        message.push(1);
        let body_length =
            u32::try_from(body.len()).map_err(|_| protocol("D-Bus body is too large"))?;
        message.extend_from_slice(&body_length.to_le_bytes());
        message.extend_from_slice(&serial.to_le_bytes());
        encode(
            &mut message,
            &Value::Array {
                element: "(yv)".to_owned(),
                items: fields,
            },
        )?;
        // The body always starts on an 8-byte boundary.
        while !message.len().is_multiple_of(8) {
            message.push(0);
        }
        message.extend_from_slice(&body);
        self.stream
            .write_all(&message)
            .map_err(|error| io_error(&error))?;
        Ok(serial)
    }

    /// Waits for the first message that satisfies `accept`.
    ///
    /// Messages that do not match are kept, so a signal that arrives while a
    /// method reply is outstanding is not lost.
    pub(crate) fn wait(
        &mut self,
        timeout: Duration,
        accept: impl Fn(&Message) -> bool,
    ) -> Result<Message, CaptureError> {
        if let Some(index) = self.pending.iter().position(&accept) {
            return Ok(self.pending.remove(index));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(CaptureError::Io {
                    message: "the desktop portal did not answer in time".to_owned(),
                });
            }
            match self.read_message() {
                Ok(message) => {
                    if accept(&message) {
                        return Ok(message);
                    }
                    self.pending.push(message);
                }
                Err(CaptureError::Io { .. }) => {
                    // A read timeout is the ordinary case while the user is
                    // still choosing a screen in the portal dialog.
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn read_message(&mut self) -> Result<Message, CaptureError> {
        let mut header = [0_u8; 16];
        self.stream
            .read_exact(&mut header)
            .map_err(|error| io_error(&error))?;
        if header[0] != b'l' {
            return Err(protocol("only little-endian D-Bus messages are supported"));
        }
        let kind = header[1];
        let body_length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let fields_length =
            u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
        // The field array length is followed by the fields, then padding to the
        // next 8-byte boundary, then the body.
        let padded_fields = fields_length.next_multiple_of(8);
        let total = padded_fields
            .checked_add(body_length)
            .filter(|total| *total <= MAX_MESSAGE_BYTES)
            .ok_or(CaptureError::ReplyTooLarge {
                bytes: body_length as u64,
            })?;
        let mut rest = vec![0_u8; total];
        self.stream
            .read_exact(&mut rest)
            .map_err(|error| io_error(&error))?;

        // Rebuild the buffer the way the sender aligned it: field data starts
        // at offset 16 of the complete message.
        let mut message = Vec::with_capacity(16 + rest.len());
        message.extend_from_slice(&header);
        message.extend_from_slice(&rest);

        let mut path = None;
        let mut interface = None;
        let mut member = None;
        let mut error_name = None;
        let mut reply_serial = None;
        let mut signature = String::new();
        let mut offset = 16;
        let fields_end = 16 + fields_length;
        while offset < fields_end {
            // Each (yv) struct is aligned to 8 bytes.
            while !offset.is_multiple_of(8) {
                offset += 1;
            }
            let code = message[offset];
            offset += 1;
            let value = decode(&message, &mut offset, "v")?;
            match code {
                FIELD_PATH => path = value.as_str().map(str::to_owned),
                FIELD_INTERFACE => interface = value.as_str().map(str::to_owned),
                FIELD_MEMBER => member = value.as_str().map(str::to_owned),
                FIELD_ERROR_NAME => error_name = value.as_str().map(str::to_owned),
                FIELD_REPLY_SERIAL => reply_serial = value.as_u32(),
                FIELD_SIGNATURE => {
                    if let Value::Variant(inner) = &value {
                        if let Value::Signature(text) = inner.as_ref() {
                            signature.clone_from(text);
                        }
                    }
                }
                _ => {}
            }
        }

        let body_start = 16 + padded_fields;
        let mut body = Vec::new();
        // Decoding reads from the whole message so every alignment stays
        // relative to the body start, which is itself 8-byte aligned.
        let mut offset = body_start;
        let mut remaining = signature.as_str();
        while !remaining.is_empty() {
            let element = super::codec::leading_type(remaining)?;
            body.push(decode(&message, &mut offset, element)?);
            remaining = &remaining[element.len()..];
        }
        Ok(Message {
            kind,
            path,
            interface,
            member,
            error_name,
            reply_serial,
            body,
        })
    }
}

/// Builds one `(yv)` header field.
fn header_field(code: u8, value: Value) -> Value {
    Value::Struct(vec![Value::Byte(code), value.into_variant()])
}

/// Extracts the socket path from a bus address such as `unix:path=/run/...`.
fn unix_socket_path(address: &str) -> Result<String, CaptureError> {
    address
        .split(';')
        .filter_map(|entry| entry.strip_prefix("unix:"))
        .flat_map(|entry| entry.split(','))
        .find_map(|pair| pair.strip_prefix("path="))
        .map(str::to_owned)
        .ok_or_else(|| unavailable(format!("no unix socket in the bus address {address}")))
}

/// Returns the current user ID as text, for SASL EXTERNAL.
///
/// The ID is read from `/proc/self/status` rather than through `libc`, keeping
/// the crate free of C bindings and `unsafe`.
fn current_uid() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|line| line.split_whitespace().next().map(str::to_owned))
        })
        .unwrap_or_else(|| "0".to_owned())
}

fn io_error(error: &std::io::Error) -> CaptureError {
    CaptureError::Io {
        message: format!("session bus: {error}"),
    }
}

fn unavailable(message: impl Into<String>) -> CaptureError {
    CaptureError::PlatformUnavailable {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_addresses_yield_their_unix_socket() {
        assert_eq!(
            unix_socket_path("unix:path=/run/user/1000/bus").expect("path"),
            "/run/user/1000/bus"
        );
        assert_eq!(
            unix_socket_path("unix:guid=abc,path=/run/user/1000/bus").expect("path"),
            "/run/user/1000/bus"
        );
        assert!(unix_socket_path("tcp:host=localhost,port=1").is_err());
    }

    #[test]
    fn the_current_uid_is_readable_on_this_platform() {
        assert!(current_uid().chars().all(|value| value.is_ascii_digit()));
    }
}
