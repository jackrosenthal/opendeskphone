---
title: "input"
---

## `capture`

```rust
pub fn capture(on: bool)
```

Turns key capture on or off.  While capture is on and the app's window is the top screen:

- Every key press arrives as [`Event::Key`](app-events.md#key), including the keys that a window otherwise never receives (line key 1, the volume keys, `Mute`, `Speaker`, and `Headset`), the four keys below the display, and the keys that a widget would handle.
- Every hookswitch change arrives as [`Event::Hook`](app-events.md#hook).
- The phone does nothing else with these keys: no volume change, softkeys, widget handling, key claims, or dial tone when the handset is lifted.

With capture off, keys follow [Focus and keys](ui.md#focus-and-keys).

The third consecutive `cancel` takes the app off the screen and isn't delivered.  The first two are delivered.  Any other key resets the count.

On a phone without keys ([`Metrics::keys`](types.md#metrics-and-skin) is `false`), the only key is `cancel`, from the [back button](ui.md#touch), and capture doesn't change where touches go.

Calls and notices from the phone cover a capturing window like any other, and get the keys while shown; a notice takes `select`, `cancel`, and the four keys below the display.  Capture stays on while the window is off screen, and resumes when the window returns.
