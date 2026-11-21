//! Transient events: things that happen rather than things that are.  They
//! are delivered to subscriptions that asked for events and are not stored.

use crate::types::*;
use crate::Key;
use serde::{Deserialize, Serialize};

/// Where an input event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputSource {
    /// The keypad or hookswitch.
    Physical,
    /// `Command::InjectKey` (web mirror, SSH, tests).
    Injected,
    /// Headset button (EHS or Bluetooth).
    Headset,
}

/// A transient event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// A key went down or up.  Emitted by the input owner (`opendeskphone-ui`
    /// reads the HAL and re-emits, so every listener sees one stream).
    Key { key: Key, down: bool, source: InputSource },
    /// The handset was lifted (`off_hook`) or replaced.
    Hook { off_hook: bool, source: InputSource },
    /// The touchscreen was touched at pixel (`x`, `y`) of the main screen
    /// (D80).  Emitted by the input owner, as keys are.
    Touch { phase: crate::TouchPhase, x: u16, y: u16, source: InputSource },
    /// A new incoming call started alerting.
    IncomingCall { call: CallId },
    /// A call ended; carries its final snapshot (for the call log, screens,
    /// and apps).
    CallEnded { call: Call },
    /// An inbound DTMF digit (RFC 4733 or INFO).
    DtmfReceived { call: CallId, digit: char },
    /// A DPMA push or other server event for apps and the core:
    /// `name` is the event (`digium.config.update`, an app event name, ...).
    Push { name: String, body: String },
    /// A server event that arrived on a call's INVITE or on an INFO in
    /// the call's dialog, for apps: `kind` is `X-Event-Type`, `data` the
    /// event's data, and `method` the SIP method that carried it.
    CallPush { name: String, kind: String, data: String, method: String, call: CallId },
    /// A SIP MESSAGE that is not DPMA traffic.
    Message { account: AccountId, from: Party, content_type: String, body: String },
    /// A configuration merge finished (`generation` as in `ConfigInfo`).
    ConfigApplied { generation: u64 },
    /// An event addressed to one app (from another subsystem or app).
    App { app: String, name: String, payload: String },
    /// A sound an app started (`Command::PlayData`) is no longer playing;
    /// `position_secs` is how far into the sound it got.
    PlaybackStopped { app: String, position_secs: u32 },
    /// The wall clock jumped (SNTP set it); timers on wall time should
    /// re-arm.
    TimeChanged,
    /// The phone is about to reboot or shut down.
    ShuttingDown { reason: String },
}
