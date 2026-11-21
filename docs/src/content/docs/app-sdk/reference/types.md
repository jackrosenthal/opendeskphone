---
title: "Shared types"
---

The crate exports these types at the root, e.g., `opendeskphone_app_sdk::Rect`.

## Handles and identifiers

```rust
pub type Widget = u32;
pub const ROOT: Widget = 0;
```

`ROOT` is the app's [window](ui.md#window).  Handles of widgets in the [idle window](ui.md#idle-window) have the high bit set.  Handles aren't reused within an instance.

Timer and request IDs are never 0.  The `pbx` requests, `http::request`, and the `storage` requests to the PBX share one ID counter, so pending requests have distinct IDs.

## Geometry

```rust
pub struct Rect { pub x: i16, pub y: i16, pub width: u16, pub height: u16 }
pub struct Size { pub width: u16, pub height: u16 }
pub struct Point { pub x: i16, pub y: i16 }

impl Rect {
    pub const fn new(x: i16, y: i16, width: u16, height: u16) -> Rect;
}
```

Coordinates are in pixels.  A `Point` is a place in a widget, counted from the widget's top-left corner (see [`Touch`](#touch)).

## `Metrics` and `Skin`

```rust
pub enum Skin { SmallGray, LargeGray, Color, Touch }

pub struct Metrics {
    pub skin: Skin,
    pub width: u16,
    pub height: u16,
    pub line_height: u16,
    pub title_height: u16,
    pub title_offset: u16,
    pub row_height: u16,
    pub scroll_bar_width: u16,
    pub color: bool,
    pub keys: bool,
    pub back_width: u16,
}
```

[`ui::get_metrics`](ui.md#get_metrics) returns, by skin:

| Field | `SmallGray` (D40, D45, D50) | `LargeGray` (D70) | `Color` (D60, D62, D65) | `Touch` (D80) |
| --- | --- | --- | --- | --- |
| `width` | 240 | 320 | 480 | 800 |
| `height` | 102 | 140 | 243 | 1160 |
| `height` in full-screen mode | 120 | 160 | 272 | 1280 |
| `line_height` | 15 | 17 | 25 | 44 |
| `title_height` | 15 | 17 | 40 | 96 |
| `title_offset` | 1 | 1 | 4 | 8 |
| `row_height` | 15 | 17 | 32 | 96 |
| `scroll_bar_width` | 4 | 4 | 8 | 12 |
| `color` | `false` | `false` | `true` | `true` |
| `keys` | `true` | `true` | `true` | `false` |
| `back_width` | 0 | 0 | 0 | 96 |

`width` and `height` are the window's size.  `row_height` is the height of a list row.

`Skin::Touch` is the touchscreen of a phone without keys (the D80): an upright screen, about twice as many pixels to the inch as the color models have, with rows and buttons tall enough for a finger.

`keys` tells whether the phone has keys: a dial pad, arrow keys, and keys below the display.  Where `keys` is `false`, no key press ever arrives from a key, so offer every action on the screen: in a list, as a softkey, or on a widget that takes touches ([`ui::claim_touch`](ui.md#claim_touch)).  Don't show hints such as "Press 5" there.

`back_width` is the width of the [back button](ui.md#touch), which the phone draws over the top-left corner of the window on a phone without keys, `title_height` tall.  Keep that corner free; a title starts to the right of the button.  `back_width` is 0 on a phone with keys.

## Widgets

```rust
pub enum WidgetKind { Group, Text, TextList, Scroll, Input, Image }
pub enum Align { Left, Center, Right }
pub enum Font { Small, Normal, Large, Bold, Title }
pub enum InputMode { Text, Numeric, Dial, Password, Pin }

pub struct ListItem { pub text: String, pub detail: String, pub icon: Option<Image> }
pub enum Image { Icon(Icon), Picture(String) }
pub struct Softkey { pub id: u32, pub label: String }

pub enum WidgetEvent {
    Highlighted(u32),
    Activated(u32),
    Changed(String),
    Submitted(String),
}

impl ListItem {
    pub fn new(text: impl Into<String>) -> ListItem;
    pub fn with_detail(text: impl Into<String>, detail: impl Into<String>) -> ListItem;
}
impl Softkey {
    pub fn new(id: u32, label: impl Into<String>) -> Softkey;
}
```

See [`ui`](ui.md).

## `Touch`

```rust
pub enum Touch {
    Down(Point),
    Tap(Point),
    Drag((Point, Point)),
    DragEnd((Point, Point)),
}
```

What a finger did on a widget that takes touches ([`ui::claim_touch`](ui.md#claim_touch)), in [`Event::Touch`](app-events.md#touch).  Each `Point` counts pixels from the top-left corner of that widget (of the window, for `ROOT`).

| Variant | Meaning |
| --- | --- |
| `Down(at)` | The finger came down at `at`.  Arrives the moment the finger lands, before the phone knows whether the touch becomes a tap or a drag. |
| `Tap(at)` | The finger went up again at `at`, the place where it came down. |
| `Drag((from, to))` | The finger came down at `from` and has moved to `to`.  One event arrives for every movement. |
| `DragEnd((from, to))` | The finger of a drag was lifted at `to`. |

Every touch begins with `Down`, and the events of one touch arrive in this order:

| The finger | Events |
| --- | --- |
| Taps | `Down`, `Tap` |
| Drags | `Down`, a `Drag` for every movement, `DragEnd` |

Use `Down` for what can't wait until the finger is lifted, such as the flap or the jump of a game, and `Tap` for a button, which the user can still leave by sliding off.  An app that acts on `Down` shouldn't act again on the `Tap` that follows.

A finger that moves less than 18 pixels taps; a finger that moves farther drags, and doesn't tap.  A finger held in one place for 0.6 seconds taps then, and nothing more arrives when the finger is lifted.  A touch can also end after `Down` with no other event, when another screen (e.g., an incoming call) covers the window while the finger is down.  During a drag, `to` can lie outside the widget, with negative coordinates to the left of the widget or above.

## Keys

```rust
pub enum Key {
    Digit(u8),
    Star,
    Pound,
    Up,
    Down,
    Left,
    Right,
    Select,
    Cancel,
    VolumeUp,
    VolumeDown,
    Softkey(u8),
    Line(u8),
    Blf(u8),
    Mute,
    Speaker,
    Headset,
    Hold,
    Transfer,
    Redial,
    Messages,
    Conference,
    Info,
    BlfPageLeft,
    BlfPageRight,
    Status,
    Contacts,
    Apps,
    Menu,
    Expansion(ExpansionKey),
    Other(u16),
}

pub struct ExpansionKey { pub module: u8, pub key: u8 }
```

| Variant | Key |
| --- | --- |
| `Digit(0)` to `Digit(9)`, `Star`, `Pound` | The dial pad. |
| `Select`, `Cancel` | The check-mark key and the X key. |
| `Softkey(1)` to `Softkey(4)` | The four keys below the display, left to right. |
| `Line(1)` to `Line(6)` | The line keys, top to bottom. |
| `Blf(1)` to `Blf(10)` | The rapid-dial keys, top to bottom. |
| `Messages` | Voicemail (`Msgs`). |
| `BlfPageLeft`, `BlfPageRight` | The page keys of the rapid-dial keys. |
| `Expansion(ExpansionKey { module, key })` | A key of an expansion module, both from 0. |
| `Other(code)` | A key without a function of its own, by key code. |

No model has every key.

## `Error`

```rust
pub enum Error {
    Invalid(String),
    Failed(String),
}
```

| Variant | Meaning |
| --- | --- |
| `Invalid(text)` | An argument is invalid; the text says which and why. |
| `Failed(text)` | The phone failed or refused; the text says why. |

## HTTP

```rust
pub struct Header { pub name: String, pub value: String }
pub struct HttpResponse { pub status: u16, pub headers: Vec<Header>, pub body: Vec<u8> }
```

## PBX

```rust
pub enum PbxResult {
    Done,
    Presence(PbxPresence),
    PresenceOptions(Vec<PbxPresenceOption>),
    ParkingLots(Vec<ParkingLot>),
    Queues(Vec<CallQueue>),
    QueueStatus(QueueStatus),
    Voicemail(VoicemailPage),
    VoicemailFolders(Vec<VoicemailFolder>),
}

pub enum PbxError { Server(PbxServerError), Transport(String) }
pub struct PbxServerError { pub code: u32, pub message: String }

pub enum PbxPresenceState { NotSet, Unavailable, Available, Away, Xa, Chat, Dnd }

pub struct PbxPresence {
    pub option_id: u32,
    pub state: PbxPresenceState,
    pub sub_state: String,
    pub message: String,
}

pub struct PbxPresenceOption {
    pub id: u32,
    pub state: PbxPresenceState,
    pub sub_state: String,
    pub by_user: bool,
    pub active: bool,
}

pub struct ParkingLot {
    pub name: String,
    pub first_space: String,
    pub last_space: String,
    pub timeout: u32,
    pub calls: Vec<ParkedCall>,
}

pub struct ParkedCall {
    pub space: String,
    pub caller_name: String,
    pub caller_number: String,
    pub duration: u32,
}

pub enum QueueMembership { LoggedIn, Permanent, LoggedOut, Manager }

pub struct CallQueue {
    pub id: String,
    pub name: String,
    pub membership: QueueMembership,
    pub paused: bool,
    pub members: Option<u32>,
    pub members_logged_in: Option<u32>,
    pub callers_waiting: Option<u32>,
}

pub struct QueueStatus {
    pub id: String,
    pub name: String,
    pub members: u32,
    pub members_logged_in: u32,
    pub members_on_call: u32,
    pub callers_waiting: u32,
    pub calls: u32,
    pub completed_calls: u32,
    pub abandoned_calls: u32,
    pub member_list: Vec<QueueMember>,
    pub caller_list: Vec<QueueCaller>,
}

pub struct QueueMember { pub name: String, pub membership: QueueMembership, pub on_call: Option<u32> }
pub struct QueueCaller { pub name: String, pub number: String, pub position: u32, pub waiting: u32 }

pub enum VoicemailSort { Date, Id }

pub struct VoicemailPage {
    pub folder: String,
    pub page: u32,
    pub pages: u32,
    pub total: u32,
    pub messages: Vec<VoicemailMessage>,
}

pub struct VoicemailMessage {
    pub id: String,
    pub caller_name: String,
    pub caller_number: String,
    pub date: u64,
    pub duration: u32,
    pub read: bool,
}

pub struct VoicemailFolder { pub name: String, pub count: u32 }
```

| Field | Contents |
| --- | --- |
| `PbxPresence::option_id` | The `PbxPresenceOption::id` of the option that is set. |
| `sub_state`, `message` | May be empty. |
| `PbxPresenceOption::by_user` | The user made the option; otherwise the PBX has it. |
| `ParkingLot::first_space`, `last_space` | The lot's first and last parking spaces. |
| `ParkingLot::timeout`, `ParkedCall::duration` | Seconds before a parked call rings back; seconds since the call was parked. |
| `ParkedCall::space` | The number that takes the call. |
| `CallQueue::members`, `members_logged_in`, `callers_waiting` | The counts the PBX lets the phone see, if any. |
| `QueueStatus::members_logged_in` | The members logged in and not paused. |
| `QueueStatus::calls` | Calls today: completed and abandoned. |
| `QueueStatus::member_list`, `caller_list` | Empty unless the PBX lets the phone see them. |
| `QueueMember::on_call` | Seconds on the member's call, if any. |
| `QueueCaller::position`, `waiting` | 1 for the next caller to be answered; seconds waited. |
| `VoicemailPage::page`, `pages`, `total` | The page (from 1), the number of pages, and the messages in the folder. |
| `VoicemailMessage::date` | When the message was left, in seconds since the Unix epoch; 0 when the PBX's date isn't a time the phone can read (a number, or app_voicemail's text in UTC). |
| `VoicemailMessage::duration` | Seconds. |

See [`pbx`](pbx.md).

## Storage

```rust
pub enum StorageLocation { Local, PbxPrivate, PbxShared }

pub struct PbxUser { pub id: String, pub name: String, pub extension: String }

pub struct StorageEntry {
    pub key: String,
    pub value: Vec<u8>,
    pub version: u64,
    pub modified: u64,
    pub author_id: String,
    pub author_name: String,
}

pub enum StorageOutcome {
    Entry(Option<StorageEntry>),
    Entries((Vec<StorageEntry>, bool)),
    Stored(StorageEntry),
    Removed,
    Conflict(Option<StorageEntry>),
    Unreachable(String),
    Refused(String),
}

pub enum StorageReply { Done(StorageOutcome), Pending(u32) }

pub struct StorageChange { pub location: StorageLocation, pub key: String }
```

See [`storage`](storage.md).

## Calls

```rust
pub enum CallState {
    Dialing, Proceeding, Ringback, Ringing, Active, Held, Transferring, Ended,
}

pub struct CallInfo {
    pub id: u32,
    pub account: u8,
    pub incoming: bool,
    pub state: CallState,
    pub remote_name: String,
    pub remote_number: String,
    pub duration: u32,
}

pub struct AccountInfo {
    pub slot: u8,
    pub id: String,
    pub label: String,
    pub user: String,
    pub server: String,
    pub registered: bool,
}

pub struct Mwi { pub account: u8, pub new_messages: u32, pub old_messages: u32 }
```

| Field | Contents |
| --- | --- |
| `CallInfo::account`, `Mwi::account`, `AccountInfo::slot` | The account's slot, from 0. |
| `CallInfo::state` | `Dialing`, `Proceeding`, `Ringback`: outgoing, unanswered.  `Ringing`: incoming, unanswered.  `Held`: on hold, by either side.  `Ended`: over, and still listed for a few seconds. |
| `CallInfo::duration` | Seconds since the answer, as of the `CallInfo`'s creation; 0 before the answer. |
| `AccountInfo::id` | The account's `id` attribute in the phone's configuration. |

## Presence

```rust
pub enum PresenceStatus { Available, Chat, Away, Dnd, Xa, Unavailable, Unknown }
pub enum LineState { Idle, Ringing, Busy, Hold, HoldRinging, Unavailable, Unknown }

pub struct Presence {
    pub target: String,
    pub status: PresenceStatus,
    pub text: String,
    pub line_state: LineState,
}
```

| Field | Contents |
| --- | --- |
| `target` | The target string that the app passed. |
| `status` | `Xa` is extended away. |
| `text` | The status message; may be empty. |
| `line_state` | The contact's line.  `HoldRinging`: one call held and another ringing.  `Unavailable`: no device registered. |

## `Push`

```rust
pub enum PushMethod { Message, Invite, Info }

pub struct Push {
    pub name: String,
    pub kind: String,
    pub data: String,
    pub method: PushMethod,
    pub call: Option<u32>,
}
```

See [`Event::Push`](app-events.md#push).
