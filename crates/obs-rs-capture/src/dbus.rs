//! A small session-bus client and the desktop-portal screen-cast handshake.
//!
//! Wayland compositors deliberately offer no direct screen read, so capture
//! goes through `org.freedesktop.portal.ScreenCast`. Talking to it needs a
//! D-Bus connection that stays open across several round trips and receives
//! signals, which is implemented here in safe Rust over the session socket
//! rather than through a C library.

mod codec;
mod connection;
mod screencast;
mod value;

pub use screencast::{open_screencast, open_screencast_cancellable, CursorMode, ScreenCastSession};
