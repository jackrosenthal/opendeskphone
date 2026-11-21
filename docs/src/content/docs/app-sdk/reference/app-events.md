---
title: "Events"
---

```rust
pub enum Event {
    Foreground,
    Background,
    Exit,
    IdleScreenShow,
    IdleScreenHide,
    Key(Key),
    Hook(bool),
    Softkey(u32),
    Widget((Widget, WidgetEvent)),
    Touch((Widget, Touch)),
    Timer(u32),
    Pbx((u32, Result<PbxResult, PbxError>)),
    Http((u32, Result<HttpResponse, String>)),
    Call(CallInfo),
    Push(Push),
    Mwi(Mwi),
    Presence(Presence),
    NewConfig,
    ServerChange,
    PlaybackStopped(u32),
    Message((String, String)),
    Storage((u32, StorageOutcome)),
    StorageChanged(StorageChange),
    StorageOnline,
    AlarmsChanged,
    HotdeskDone(Result<(), String>),
}
```

## `Foreground`

The app's window became the top screen: the app was opened, or a covering screen (a call, a menu, another app) closed.  On a color model, the app on screen also receives `Foreground` when the user switches themes, to redraw with new [`canvas::theme_color`](canvas.md#theme_color) colors.  Treat `Foreground` as "draw the screen," not as "the app was opened."

## `Background`

The app's window stopped being the top screen: another screen covers the window, the user pressed line key 1 or an unclaimed `cancel`, or the app called `runtime::background`.  After `handle` returns, the phone stops a `Foreground` app that has no idle window, unless the app called `runtime::set_exit_after_background(false)`; the app then receives `Exit`.

## `Exit`

The last event of an instance.  The phone sends `Exit` before discarding the instance for `runtime::exit`, `runtime::restart`, the stop after `Background`, removal or replacement of the app, a configuration that disables a built-in app, or shutdown.  The handler has the usual 2 seconds and can call any function, but later results are never delivered.  After a [fault](index.md#faults-and-restarts), no `Exit` arrives.

## `IdleScreenShow` and `IdleScreenHide`

`IdleScreenShow`: the app's [idle window](ui.md#idle-window) appeared, because the idle screen is the top screen and the app has the turn among the apps with idle windows.

`IdleScreenHide`: the idle window left the screen, because another screen (a call, a menu, an app) covers the idle screen, or the turn passed to another app.

## `Key`

```rust
Event::Key(key)
```

A key claimed with [`ui::claim_keys`](ui.md#claim_keys) by the window or one of the app's widgets was pressed, or any key while [`input::capture`](input.md) is on.  On a phone without physical buttons, the only key is `Key::Cancel`, from the [back button](ui.md#touch).  Releases and repeats aren't reported, and the event doesn't name the claiming widget.  [`Key`](types.md#keys) names the key.

## `Hook`

```rust
Event::Hook(off_hook)
```

The handset was lifted (`true`) or replaced (`false`) while the app's window was on screen with [`input::capture`](input.md) on.  Without capture, the app isn't notified.

## `Softkey`

```rust
Event::Softkey(id)
```

`id` is the `id` of the pressed `Softkey` from [`ui::set_softkeys`](ui.md#set_softkeys).  The "More" key and the "Delete" key of an input send no `Softkey`.

## `Widget`

```rust
Event::Widget((widget, widget_event))
```

| `WidgetEvent` | Sent when |
| --- | --- |
| `Highlighted(row)` | The user moved a list's selection with `up` or `down`, or touched a row that wasn't selected.  Not sent for `ui::set_selected` or `ui::set_items`. |
| `Activated(row)` | The user pressed `select` on a list with at least one row, or touched a row (after `Highlighted`, if the row wasn't selected). |
| `Changed(text)` | The user typed into or deleted from an input.  Not sent for `ui::set_value`. |
| `Submitted(text)` | The user pressed `select` in an input. |

## `Touch`

```rust
Event::Touch((widget, touch))
```

A finger came down, tapped, or dragged on `widget`, a widget that takes touches ([`ui::claim_touch`](ui.md#claim_touch)); `ROOT` is the window.  [`Touch`](types.md#touch) tells what the finger did and where, counted from the widget's top-left corner.  Only a phone with a touchscreen sends `Touch` ([`Skin::Touch`](types.md#metrics-and-skin)).  Touches on the stock widgets, the softkeys, and the back button arrive as the events of those: `Widget`, `Softkey`, and `Key(Key::Cancel)`.

## `Timer`

```rust
Event::Timer(id)
```

A timer from [`timers::set`](timers.md#set) fired.

## `Pbx`

```rust
Event::Pbx((id, result))
```

The result of a [`pbx`](pbx.md) request: exactly one for each request that the app didn't cancel.

## `Http`

```rust
Event::Http((id, result))
```

The result of an [`http::request`](http.md#request).

## `Call`

```rust
Event::Call(info)
```

A call appeared, or a call's state, remote name, or remote number changed ([`CallInfo`](types.md#calls)).  The `duration` count doesn't send events.  After [`calls::observe(true)`](calls.md#observe), the app receives events for every call on the phone.  A call's end is one event with `CallState::Ended`.

## `Push`

```rust
Event::Push(push)
```

The PBX pushed an event whose name the app observes with [`events::observe`](events.md#observe).

| Field | Contents |
| --- | --- |
| `name` | The event's name. |
| `kind` | For an event with a call, the `X-Event-Type` header; otherwise `""`. |
| `data` | The event's data as text: for an event through the DPMA session, the payload; for an `INVITE` or `INFO`, the `X-Event-Data` header, or else the body unless the body is SDP. |
| `method` | `PushMethod::Message` for an event through the DPMA session; `Invite` or `Info` for an event with a call. |
| `call` | For `INVITE` and `INFO`, the call. |

For an `INVITE` or `INFO`, the name is the last of the `X-Digium-Event-Name`, `X-Event-Name`, and `X-Event-Type` headers.

An incoming call whose event name the configuration assigns to apps (by default, `digium.incomingCall.voicemail`) doesn't ring and has no call screen.  The app answers or rejects the call with [`calls::answer`](calls.md#answer) or [`calls::reject`](calls.md#reject) and `push.call`.  If an event whose name starts with `digium.incomingCall.` names no call, the phone sets `call` to the newest ringing call and `method` to `Invite`.

Such a call belongs to the apps that received the push, and ends when the last of those apps has left the screen or stopped.  An app off screen when the push arrives keeps the call until the app has been on screen and left, or stops.  A call whose event no running app observes ends when the app on screen leaves the screen.

## `Mwi`

```rust
Event::Mwi(mwi)
```

An account's count of new or old messages changed.  Every running app that uses `calls` receives the event.

## `Presence`

```rust
Event::Presence(presence)
```

A watched contact's presence changed, and the app observes a matching target with [`presence::observe`](presence.md#observe).

## `NewConfig`

The phone applied a new merged configuration, so [`config`](config.md) values may have changed.  Every running app that uses `config` receives the event.

## `ServerChange`

An account registered with a different host: the alternate host, or the primary again.  No payload; [`config::accounts`](config.md#accounts) shows the registrations.  Every running app that uses `events` receives the event.  The push `digium.phone.server_change` never arrives as `Event::Push`.

## `PlaybackStopped`

```rust
Event::PlaybackStopped(position_secs)
```

The sound from the app's [`audio::play`](audio.md#play) stopped: the sound ended, the app called `audio::stop`, another sound replaced the sound, or the ringer cut the sound off.  The value is the position in seconds.

## `Message`

```rust
Event::Message((name, payload))
```

An app, possibly this app, called [`events::send`](events.md#send) with this app's name.  The event doesn't identify the sender.  Receiving needs no capability.

## `Storage`

```rust
Event::Storage((id, outcome))
```

The result of a [`storage`](storage.md) request to a PBX location: exactly one for each `StorageReply::Pending(id)` that the app didn't cancel.

## `StorageChanged`

```rust
Event::StorageChanged(change)
```

Another phone set or removed the app's `PbxShared` key `change.key`.  Sent after [`storage::watch(true)`](storage.md#watch).

## `StorageOnline`

The PBX locations of [`storage`](storage.md) became reachable, for the first time or again.  Requests that ended in `Unreachable` can be retried, and earlier reads may be stale.  Every running app that uses `storage` receives the event.

## `AlarmsChanged`

An alarm was added, changed, or removed (by any app or the web interface), rang, or was snoozed or dismissed.  Every running app that uses [`alarms`](alarms.md) receives the event.

## `HotdeskDone`

```rust
Event::HotdeskDone(result)
```

The outcome of this app's [`hotdesk::login`](hotdesk.md#login) or [`hotdesk::logout`](hotdesk.md#logout) call that returned `Ok`, once per such call.
