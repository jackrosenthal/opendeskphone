//! Keys, the hookswitch, and the touchscreen.

use crate::{Key, Result, TouchPhase};
use std::os::fd::RawFd;
use std::time::Duration;

/// One input event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Key { key: Key, down: bool },
    /// `off_hook`: the handset was lifted.
    Hook { off_hook: bool },
    /// The touchscreen (D80): one finger, at pixel (`x`, `y`) of the main
    /// screen as the UI draws it (the backend turns the panel's own
    /// coordinates into these).
    Touch { phase: TouchPhase, x: u16, y: u16 },
}

/// Keypad, hookswitch, and touchscreen.
pub trait Input: Send {
    /// Waits for the next event.  `None` timeout blocks; `Some(0)` polls.
    /// Returns `Ok(None)` on timeout.
    fn next_event(&mut self, timeout: Option<Duration>) -> Result<Option<InputEvent>>;
    /// Whether the handset is lifted right now (read at start-up, since no
    /// event announces the initial position).
    fn off_hook(&mut self) -> Result<bool>;
    /// File descriptors that become readable when an event is pending, for
    /// callers that multiplex with `poll(2)` or an async reactor.  Empty if
    /// the backend has none (then use `next_event` with a timeout).
    fn poll_fds(&self) -> Vec<RawFd>;
}
