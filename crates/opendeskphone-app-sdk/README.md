# opendeskphone-app-sdk

The SDK for writing apps for [Open Desk Phone](https://github.com/jackrosenthal/opendeskphone), the open firmware for Digium D-series desk phones.

An app is a WebAssembly component.  The phone calls it once when it starts and then once for every event (a key, a softkey, a touch, a timer, a reply from the PBX), and the app answers by calling the phone: it builds its screen from widgets, sets softkeys, starts timers, reads and writes its keys and values, and sends requests to the PBX.  Everything that takes time returns an id at once and delivers its result later as an event, so an app never blocks the phone.

The version of this crate is the version of the firmware whose app interface it is.  The interface itself is `wit/app.wit` in this crate; the firmware generates its side from the same file.

## A minimal app

`Cargo.toml`:

```toml
[package]
name = "hello"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
opendeskphone-app-sdk = "=3.1.0"

[profile.release]
opt-level = "s"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

`src/lib.rs`:

```rust,ignore
use opendeskphone_app_sdk::prelude::*;

const SK_EXIT: u32 = 1;

#[derive(Default)]
struct Hello;

impl App for Hello {
    fn start(&mut self) {
        let area = screen("Hello");
        let text = ui::create(WidgetKind::Text, ROOT, area);
        ui::set_text(text, "Hello from an app.");
        ui::set_softkeys(ROOT, &[Softkey::new(SK_EXIT, "Exit")]);
    }

    fn handle(&mut self, event: Event) {
        if let Event::Softkey(SK_EXIT) = event {
            runtime::exit();
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Hello,
    name: "hello",
    display_name: "Hello",
    description: "Says hello",
}
```

That is the whole app: there is no other file, and no file beside the app on the phone.  An app implements `App` (`start` and `handle`) for a type that has a `Default`, and registers the type, with the app's manifest, in `export_app!`.  The phone makes one instance of it.

The release profile builds for size, as the built-in apps are built: the phone has little memory and little flash, and it compiles the app when it is installed.

## The manifest

The manifest is what the phone has to know about an app before it runs it.  It is written in `export_app!`, after `app` (the app's type), as fields in any order; `name` and `display_name` must be there.  A manifest the phone would refuse does not compile, and the error says why.

| Field | Meaning |
| --- | --- |
| `name` | The app's name on the phone: 1 to 64 characters, the first a letter or digit, the rest letters, digits, `_`, `.`, or `,`.  It cannot be the name of a built-in app, or `opendeskphone`, which is the phone's.  Installing an app replaces the installed app of the same name. |
| `id` | An identifier of the app, e.g., in reverse domain form (`"org.example.hello"`). |
| `display_name` | What the phone calls the app on screen. |
| `description` | A line about the app. |
| `kind` | `AppKind::Foreground` (the default): the app runs while it is on screen.  `AppKind::Background`: it starts with the phone and runs without being on screen. |
| `has_idle` | `true` if the app has a window on the idle screen (`ui::idle_root`).  Such an app starts with the phone, and keeps running when it leaves the screen, whatever its `kind`. |
| `hidden` | `true` marks the app as hidden. |
| `icon` | The app's icon: the content of a PNG file, `include_bytes!("hello.png")`.  At most 256 KiB. |
| `icon_for` | Icons for particular models of phone, each as `icon` is: `&[(PhoneModel::D60, include_bytes!("color.png"))]`.  A phone shows its model's icon if there is one, else `icon`. |
| `phone_icon` | One of the phone's icons, for an app without an icon: `Some(Icon::AppCalendar)`. |
| `memory_kib` | The most memory the app may have, in KiB.  The default is 4 MiB, and the phone gives no app more than 16 MiB. |

A manifest with everything:

```rust,ignore
opendeskphone_app_sdk::export_app! {
    app: Weather,
    name: "weather",
    id: "org.example.weather",
    display_name: "Weather",
    description: "The weather where the phone is",
    kind: AppKind::Background,
    has_idle: true,
    icon: include_bytes!("weather.png"),
    icon_for: &[
        (PhoneModel::D60, include_bytes!("weather-color.png")),
        (PhoneModel::D65, include_bytes!("weather-color.png")),
    ],
    memory_kib: 8192,
}
```

The manifest travels in the app's file: `export_app!` writes it, when the app is compiled, into the custom section `opendeskphone-app` of the WebAssembly module, as the protobuf message `opendeskphone.app.v1.Manifest` (`proto/manifest.proto` in this crate).  The phone reads it from there before it installs the app.

## Data

An app's data (pictures, sounds, tables) is in its code: `include_bytes!` and `include_str!`, as in any Rust program.  The phone unpacks nothing.

- A picture: `ui::define_image("logo", include_bytes!("logo.png"))` gives a PNG a name, once; `ui::set_image` and the `icon` of a list item then take `Image::Picture("logo".into())`.  `Image::Icon(...)` is one of the phone's icons.  At most 512 KiB and 1,048,576 pixels a picture, and 32 pictures; `define_image` returns an error for content the phone cannot show.  An app that wants another picture on another model of phone chooses by `ui::get_metrics()`.
- A sound: `audio::play(include_bytes!("tone.wav"), 0)`.
- The data counts toward the app's memory (`memory_kib`), since it is part of the program.

## Capabilities

Each interface of the phone except `runtime` is a capability, and each is a module of this crate with the same name.  An app does not declare its capabilities: they are the interfaces it uses (the imports of its WebAssembly component, which hold exactly the interfaces the app calls), and the phone gives it those and no others.

| Capability | What it is for |
| --- | --- |
| `ui` | Widgets, softkeys, keys, touches, and the window. |
| `canvas` | A widget the app draws itself, from filled rectangles, one-color bitmaps, and texts in the skin's fonts. |
| `timers` | One-shot and repeating timers, and the time. |
| `storage` | The app's keys and values, on the phone and on the PBX, which no other app can reach. |
| `config` | The app's provisioned configuration, the accounts, and settings of the phone that apps may read. |
| `pbx` | Requests to the PBX. |
| `http` | HTTP requests. |
| `calls` | Placing and controlling calls, and call events. |
| `presence` | The presence of other extensions, and the status of the phone's user. |
| `events` | Events pushed to the phone, and messages between apps. |
| `audio` | Playing a sound file. |
| `l10n` | The locale, common words, and the text of dates and times. |
| `hotdesk` | Changing which user the phone is provisioned for. |
| `diagnostics` | The state of the phone: its subsystems and its log. |
| `lamps` | The phone's lamps, while the app is on screen. |
| `input` | Every key and the hookswitch, while the app is on screen and asks for them. |
| `alarms` | The phone's alarm clock: the alarms, and setting them. |

`runtime` (logging, leaving the screen, exiting, random numbers) is not a capability: every app has it.

The phone stops an app whose code runs for more than two seconds to handle one event (waiting for the phone's answer, as in `calls::dial`, does not count), that takes more than ten seconds in all to handle one event, that asks for more memory than its limit, or that panics.  A panic's message, with the file and the line, is in the phone's log (`export_app!` sees to that).  An app that starts with the phone (`AppKind::Background`, or `has_idle`) is started again after such a fault: after 10 seconds, and after twice as long for each further fault in a row, up to an hour.  The count starts over when the user opens the app, and when the app has run for an hour without a fault.

What an app changes in its window while it handles one event appears together, when `handle` returns.

## Keys and the touchscreen

Most of the phones have keys and a small screen; the D80 has a touchscreen and no keys.  `ui::get_metrics()` tells an app which it is on: the `skin` (`SmallGray`, `LargeGray`, `Color`, or `Touch`), the size of the window (240x102, 320x140, 480x243, or 800x1160), the heights of a line, a title, and a list's row, and `keys`, which is `false` on a phone that is used by touch alone.  An app that lays its screen out from the metrics, as `screen`, `Form`, and `Message` of this crate do, fits every phone.

The widgets work by touch without the app's help.  Touching a row of a list highlights and activates it, as `select` does (`WidgetEvent::Highlighted`, if the row was not highlighted, then `Activated`); a finger scrolls a list and a scroll widget; touching an input focuses it for the phone's on-screen keyboard; and the softkeys are buttons under the window.  There is no X key, so the phone draws a back button over the top-left corner of the window, `back_width` wide and `title_height` high, which is the key `cancel`: `Event::Key(Key::Cancel)` for an app that claims it, as `screen` does (and `screen` begins the title after the button).  An app that draws its own screen keeps that corner free.

What is left to the app is what it did with a key that has no softkey: the keys an app claims with `ui::claim_keys` never come where there are no keys (`cancel` apart).  There, a widget takes touches instead: after `ui::claim_touch(widget, true)`, what a finger does on the widget (`ROOT`: on the window, where no widget takes it) comes as `Event::Touch((widget, touch))`, with places counted in pixels from the widget's top-left corner:

- `Touch::Tap(at)`: the finger came down and went up again at `at`.
- `Touch::Drag((from, to))`: the finger came down at `from` and is now at `to`; for every movement.
- `Touch::DragEnd((from, to))`: the finger was lifted at `to`.

```rust,ignore
fn show(&mut self) {
    let m = ui::get_metrics();
    ui::clear(ROOT);
    ui::claim_keys(ROOT, &[Key::Up, Key::Down, Key::Left, Key::Right, Key::Cancel]);
    self.board = canvas::create(ROOT, Rect::new(0, m.title_height as i16, m.width, m.width));
    ui::claim_touch(self.board, true);
    if m.keys {
        self.hint("Arrows move");
    } else {
        self.hint("Swipe to move");
    }
}

fn handle(&mut self, event: Event) {
    match event {
        Event::Key(Key::Up) => self.step(Dir::Up),
        // ...
        Event::Touch((w, Touch::DragEnd((from, to)))) if w == self.board => {
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            self.step(match (dx.abs() > dy.abs(), dx > 0, dy > 0) {
                (true, true, _) => Dir::Right,
                (true, false, _) => Dir::Left,
                (false, _, true) => Dir::Down,
                (false, _, false) => Dir::Up,
            });
        }
        Event::Touch((w, Touch::Tap(at))) if w == self.board => self.touched(at.x, at.y),
        _ => {}
    }
}
```

On a phone with keys nothing is touched: `claim_touch` does nothing there, and no `Event::Touch` comes.

## Storage

`storage` keeps an app's keys and values in one of three places, the `StorageLocation` that every function of it takes:

- `Local`: on the phone, which keeps them across restarts and firmware updates and deletes them when the app is removed.
- `PbxPrivate`: on the PBX, one set for each user.  The user is who the phone is provisioned for, so a user's keys are the same on every phone that user has, and are there again after a phone is reset: how far a user has come in a game.
- `PbxShared`: on the PBX, one set for the app's copies on all phones: a game's table of best times.

No other app can reach any of them.  The PBX locations need a PBX with Open Desk Phone's module for Asterisk (Open DPMA); there is nothing to configure in the app or on the phone.  A PBX that serves several tenants keeps a store for each, and a phone reaches only its tenant's.

| | `Local` | `PbxPrivate` and `PbxShared` |
| --- | --- | --- |
| A key | 1 to 64 bytes, not starting with `.`, without `/`, `\`, or NUL | 1 to 64 characters, each a letter, a digit, or one of `.`, `_`, `-`, `/`, and `:` |
| A value | At most 1 MiB | At most 4096 bytes |
| Keys | As many as fit on the flash | 64 for each user, and 1024 shared |
| The outcome | At once: `StorageReply::Done(outcome)` | Later: `StorageReply::Pending(id)`, then `Event::Storage((id, outcome))` |

| Function | Asks | The outcome |
| --- | --- | --- |
| `get(location, key)` | The entry of a key. | `Entry(Some(entry))`, or `Entry(None)` if the key is not set. |
| `list(location, prefix, after)` | The entries whose keys start with `prefix` and come after the key `after` (`""` for the first ones), in the order of their keys. | `Entries((entries, more))`: at most 64 entries and, past the first, about 16 KiB of values.  If `more`, ask again with the last key as `after`. |
| `set(location, key, value, expect)` | To set a key. | `Stored(entry)`, or `Conflict` (below). |
| `remove(location, key, expect)` | To remove a key. | `Removed`, or `Conflict`. |

Any of them can also end `Unreachable(text)` or `Refused(text)` (below).  `cancel(id)` forgets a request to the PBX: its result does not come.  `read_string(key)` is the value of a `Local` key as text.

A `StorageEntry` has the `key`, the `value`, a `version`, the time it was set (`modified`, in seconds since the Unix epoch by the PBX's clock, or the phone's for `Local`), and, on the PBX, who set it: `author_id` and `author_name`, which the PBX fills in, so an entry of a table of best times says whose it is without the app writing a name.

### Who the user is

`storage::user()` is the phone's user as the PBX knows them: `id` (which identifies the user and does not change), `name` (to show), and `extension`.  It is `None` until the PBX was first reached after the phone started.

### Two writers at the same time

Each write gives the entry a `version` that the key never had before.  `expect` says what the write is to replace:

- `None`: whatever is there.
- `Some(0)`: nothing; the key must not be set.
- `Some(version)`: the entry with that version.

When the key is not as expected, nothing is changed, and the outcome is `Conflict(current)` with the entry that is there now (`None` if the key is not set).  An app that changes a value that other phones change too, such as a list, reads it, changes it, writes it with the version it read, and on `Conflict` changes the entry it was shown and writes again.  Without `expect`, the later of two writes replaces the earlier.

A list of the best times that phones share, kept in one key:

```rust,ignore
use opendeskphone_app_sdk::StorageLocation::PbxShared;

const TABLE: &str = "best/7x7-easy";

impl Game {
    /// The player finished in `secs` seconds.
    fn finished(&mut self, secs: u32) {
        self.mine = Some(secs);
        self.wait_for(storage::get(PbxShared, TABLE));
    }

    /// Notes the request whose outcome is to come.
    fn wait_for(&mut self, reply: StorageReply) {
        if let StorageReply::Pending(id) = reply {
            self.asked = id;
        }
    }

    /// Writes the table again with the player's time in it, over exactly
    /// the entry it was read from.
    fn add_mine(&mut self, current: Option<StorageEntry>) {
        let Some(secs) = self.mine else { return };
        let name = storage::user().map(|u| u.name).unwrap_or_default();
        let (mut rows, version) = match &current {
            Some(entry) => (parse(&entry.value), entry.version),
            None => (Vec::new(), 0),
        };
        rows.push((secs, name));
        rows.sort();
        rows.truncate(10);
        self.wait_for(storage::set(PbxShared, TABLE, &format(&rows), Some(version)));
    }
}

impl App for Game {
    fn handle(&mut self, event: Event) {
        match event {
            Event::Storage((id, outcome)) if id == self.asked => match outcome {
                // The table as it is: add to it.  After a conflict this is
                // the table another phone wrote meanwhile.
                StorageOutcome::Entry(current) | StorageOutcome::Conflict(current) => {
                    self.add_mine(current)
                }
                StorageOutcome::Stored(entry) => {
                    self.mine = None;
                    self.show(parse(&entry.value));
                }
                // Kept in `self.mine` (and in `Local`, to outlast the
                // app): written at `Event::StorageOnline`.
                StorageOutcome::Unreachable(_) => {}
                other => log_warn(&format!("the table was not written: {other:?}")),
            },
            Event::StorageOnline if self.mine.is_some() => {
                self.wait_for(storage::get(PbxShared, TABLE));
            }
            _ => {}
        }
    }
}
```

A user's progress, which one phone writes at a time, needs no `expect`:

```rust,ignore
use opendeskphone_app_sdk::StorageLocation::{Local, PbxPrivate};

// When the player finishes a level:
let level_text = level.to_string();
let _ = storage::set(Local, "level", level_text.as_bytes(), None);
let _ = storage::set(PbxPrivate, "level", level_text.as_bytes(), None);

// When the app starts, and at `Event::StorageOnline`:
if let StorageReply::Pending(id) = storage::get(PbxPrivate, "level") {
    self.asked = id;
}

// Then, in `handle`:
Event::Storage((id, StorageOutcome::Entry(entry))) if id == self.asked => {
    let theirs = entry.and_then(|e| String::from_utf8(e.value).ok()?.parse::<u32>().ok());
    match theirs {
        // The phone was reset, or the user played on another phone.
        Some(level) if level > self.level => self.level = level,
        // The PBX was out of reach when the player came this far.
        _ => {
            let _ = storage::set(PbxPrivate, "level", self.level.to_string().as_bytes(), None);
        }
    }
}
```

### Changes from other phones

After `storage::watch(true)`, the app gets `Event::StorageChanged(change)` whenever another phone sets or removes one of its `PbxShared` keys, with the `location` and the `key`, while the app runs; it reads the key again if it shows it.  A phone is not told of its own writes, nor of a user's keys.  An app that is not running is told nothing: it reads what it shows when it starts.

### When the store cannot be reached

A request to the PBX ends `Unreachable(text)` when the phone has no session with the PBX (it is not provisioned, the network is down, or the PBX is) or the PBX does not answer (it may be one that keeps no app data).  The phone keeps nothing to send later: what the app wanted to write is the app's to keep, in `Local` if it must outlast the app, and to write again.  `Event::StorageOnline` says when: it comes to every running app that uses `storage` each time the PBX comes into reach, and also means that what the app read there before may have changed.  `storage::online()` says whether the PBX is in reach now.  A `set` or `remove` that ends `Unreachable` because no answer came may still have been done; with `expect`, writing again then ends in `Conflict` with the entry it wrote.  A `Local` request ends `Unreachable` only when the phone cannot read or write its flash.

`Refused(text)` is the store's refusal of a request that it will refuse again: a key that cannot be one, a value over the limit, or a store that already has as many keys as it may (for `Local`, a full flash).

## The alarm clock

The alarm clock is the phone's, not an app's: the phone keeps the alarms, and when one is due it sounds the alarm's tone and shows the alarm with Snooze and Dismiss over whatever is on screen, whether or not any app runs.  `alarms` is the interface to it for an app that shows the alarms and changes them, as the built-in Alarms app does (`apps/alarms`); the web interface's page "Alarms" changes the same alarms.

An `Alarm` has an `id`, a time of day (`hour` and `minute`, in the phone's time zone), the days of the week it rings on (`days`: bit 0 is Sunday, bit 6 Saturday; 0 is a one-time alarm, which rings the next time the clock shows its time and is then switched off), a `tone` (one of the alarm clock's tones, which are not the ringtones), and `enabled`.

| Function | Does |
| --- | --- |
| `list()` | The alarms, in the order they were added. |
| `set(alarm)` | Adds an alarm (`id` 0) or changes the one with that `id`, and returns its `id`.  The phone keeps at most 16 alarms. |
| `remove(id)` | Removes an alarm. |
| `tones()` | The tones an alarm can ring with. |
| `preview(tone)` | Sounds an alarm tone for a few seconds, for choosing one; `None` stops it. |
| `next()` | The next time an alarm rings: its `id` and the time, in seconds since the Unix epoch. |
| `snoozed()` | The snoozed alarm, if one is, and when it rings again. |
| `dismiss()` | Ends the alarm that is ringing or snoozed. |

`Event::AlarmsChanged` comes to every running app that uses `alarms` whenever the alarm clock changed: an alarm was set, changed, or removed, from any side, or rang, or was snoozed or dismissed.

```rust,ignore
use opendeskphone_app_sdk::alarms::{self, Alarm};

// Every Monday at 8:00, with the phone's first alarm tone.
let id = alarms::set(&Alarm {
    id: 0,
    hour: 8,
    minute: 0,
    days: 1 << 1,
    tone: String::new(),
    enabled: true,
})?;
```

## Building

Building an app takes the WebAssembly target of Rust and `wasm-tools`.  Install them once:

```text
rustup target add wasm32-unknown-unknown
cargo install wasm-tools
```

Then, in the app's directory:

```text
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new target/wasm32-unknown-unknown/release/hello.wasm -o hello.wasm
```

The first command builds the crate into a WebAssembly module, which cargo names after the crate (with `_` for every `-` in the crate's name).  The second makes a WebAssembly component of the module, and that is the app: one file, here `hello.wasm`, with the manifest and all of the app's data in it.  The phone goes by the manifest's `name`, not by the name of the file.

The phone compiles the app's code when the app is installed, which takes it about 2 seconds and another 0.12 seconds for every KiB of code.  It takes a file of up to 8 MiB with up to 1 MiB of code in it.

## Installing

In the phone's web interface, the page "Apps" has the button "Install App", which uploads the file.  The phone reads the manifest, compiles the component (calls are not disturbed, and the phone can be used meanwhile), and lists the app with the others; installing an app whose `name` is already installed replaces that app.  "Remove" on the same page removes an installed app with what it stored.

After a firmware update that changes the phone's WebAssembly engine, the phone compiles the installed apps again by itself, one after the other, when it starts.

## License

GPL-2.0-or-later, as the firmware.
