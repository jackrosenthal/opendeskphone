---
title: "SDK helpers"
---

Helpers call the interfaces on the other pages, and give the app the capabilities of those interfaces.

## Prelude

`use opendeskphone_app_sdk::prelude::*;` imports:

- The trait `App`, the constant `ROOT`, and the types `Event`, `Key`, `Widget`, `Rect`, `ListItem`, `Image`, `Icon`, `Softkey`, `WidgetKind`, `WidgetEvent`, `Skin`, `PbxResult`, `PbxError`, `StorageLocation`, `StorageEntry`, `StorageOutcome`, `StorageReply`, `TimeStyle`, `Touch`, `Point`, `AppKind`, `Form`, and `Message`.
- The functions `screen`, `content_rect`, `row_height`, and `has_keys`.
- The modules `audio`, `calls`, `config`, `events`, `http`, `l10n`, `pbx`, `presence`, `runtime`, `storage`, `timers`, and `ui`.

## Screens

### `screen`

```rust
pub fn screen(title: &str) -> Rect
```

Clears the window with `ui::clear(ROOT)`, which also removes the window's softkeys, key claims, and touch claim; claims `cancel` for the window; creates a `Text` in `Font::Title`, `title_height` tall, across the top; and returns the rectangle below the title.  On the touchscreen, the title starts to the right of the [back button](ui.md#touch) (`back_width`), and the title has the focus, so that a list made after it has no highlighted row until a row is touched (see [`nav::focus_list`](#navfocus_list)).

Make every screen's title with `screen`: the titles of all apps then look alike, and look like the phone's own.

`cancel` then arrives as `Event::Key(Key::Cancel)` instead of taking the app off the screen.  The app should call `runtime::background` or go back one screen.  On the touchscreen, the back button is `cancel`.

### `content_rect`

```rust
pub fn content_rect() -> Rect
```

Returns the whole window: `Rect::new(0, 0, metrics.width, metrics.height)`.

### `row_height`

```rust
pub fn row_height() -> u16
```

Returns `ui::get_metrics().row_height`.

### `has_keys`

```rust
pub fn has_keys() -> bool
```

Returns `ui::get_metrics().keys`: `false` on a phone that is used by touch alone (the D80).  See [`Metrics`](types.md#metrics-and-skin).

### `nav::softkeys`

```rust
pub fn softkeys(keys: &[Softkey], for_keys: &[u32]) -> Vec<Softkey>
```

Returns the softkeys of `keys` that the phone needs: all of them on a phone with keys, and on the touchscreen those whose `id` isn't in `for_keys`.  Name in `for_keys` the softkeys that do what `cancel` does on the screen ("Back", "Exit"), because the touchscreen's [back button](ui.md#touch) is `cancel`, and the softkeys that act on the highlighted row of a list ("Open", "Select"), because a touch on the row does that.

```rust
ui::set_softkeys(ROOT, &nav::softkeys(
    &[Softkey::new(SK_REFRESH, "Refresh"), Softkey::new(SK_EXIT, "Exit")],
    &[SK_EXIT],
));
```

### `nav::focus_list`

```rust
pub fn focus_list(list: Widget)
```

Gives a list the focus on a phone with keys, where the arrows move the list's highlight.  On the touchscreen, does nothing: a list there has no highlighted row until the user touches one (a highlight at rest would look like a choice the user made).  Use `ui::set_focus` instead for a list whose highlighted row is the value that is chosen, such as the current setting.

### `Message`

```rust
pub struct Message { pub text: Widget }

impl Message {
    pub fn show(title: &str, text: &str, keys: &[Softkey]) -> Message;
}
```

A screen with a title and wrapped text.  `show` calls `screen`, creates the text widget, sets `keys` as the window's softkeys, and focuses the text.

### `nav::note`

```rust
pub fn note(area: Rect, text: &str) -> Widget
```

Creates a wrapped `Text` that fills `area` (the rectangle that `screen` returned), inset as the text of a `Message`: for a screen that shows a sentence in place of a list.  The insets of `note`, `Message`, and `Form` are larger on the touchscreen.

A text is in the middle of its widget's height, so on a phone with keys the sentence is in the middle of `area`.  On the touchscreen the sentence is centered, and its middle is a quarter of the way down `area`: in the upper part of the tall screen, which is read first.  Say in the sentence what is missing and what to do about it: "There are no alarms.  Touch New alarm to set one."

### `nav::Confirm`

```rust
pub struct Confirm { /* ... */ }

impl Confirm {
    pub fn show(title: &str, question: &str, action: &str, look: Look) -> Confirm;
    pub fn touched(&mut self, w: Widget, t: &Touch) -> Option<bool>;
}
```

Touch: a question before an action, on a screen of its own: `screen(title)`, the question as a [`note`](#navnote), and across the bottom two [buttons](#controls), "Cancel" at the left and the action at the right.  Name the action by what it does ("Delete", "Log out"), and give it `Look::Destructive` if it deletes something.  The back button cancels too.  Hand every `Event::Touch` to `touched`: it returns `Some(true)` when the action was touched, and `Some(false)` when "Cancel" was.

On a phone with keys, ask with a `Message` and softkeys instead.

### `Form`

```rust
pub struct Form { pub inputs: Vec<Widget> }

impl Form {
    pub fn show(title: &str, fields: &[(&str, InputMode, &str)], keys: &[Softkey]) -> Form;
    pub fn value(&self, index: usize) -> String;
    pub fn focus_next(&self, current: Widget) -> bool;
}
```

A screen with a title and a labeled input for each entry of `fields`: label, input mode, and initial value.

- `show` sets `keys` as the window's softkeys and focuses the first input.
- `value(index)` returns an input's text, or `""` for an `index` out of range.
- `focus_next(current)` focuses the input after `current`, and returns `false` if `current` is the last input or not in the form.  On `WidgetEvent::Submitted`, call `focus_next`, and submit the form on `false`.

## Controls

The `controls` module has the controls of the touchscreen that the widgets don't have: buttons and switches.  Each is a canvas (capability `canvas`) that takes touches, drawn in the skin's colors in the shape of the phone's own buttons.  They are for a phone without keys, where what an app does is touched on its screen; a phone with keys has softkeys for the same things.

```rust
pub enum Look { Primary, Secondary, Destructive, Call }

pub struct Button { pub widget: Widget, /* ... */ }

impl Button {
    pub fn new(parent: Widget, bounds: Rect, label: &str, look: Look) -> Button;
    pub fn with_icon(parent: Widget, bounds: Rect, icon: Icon, look: Look) -> Button;
    pub fn with_font(self, font: Font) -> Button;
    pub fn set_on(&mut self, on: bool);
    pub fn set_enabled(&mut self, enabled: bool);
    pub fn set_visible(&self, visible: bool);
    pub fn place(&mut self, bounds: Rect);
    pub fn touched(&mut self, w: Widget, t: &Touch) -> bool;
}

pub struct Switch { pub widget: Widget, /* ... */ }

impl Switch {
    pub fn new(parent: Widget, bounds: Rect, on: bool) -> Switch;
    pub fn in_row(parent: Widget, bounds: Rect, on: bool) -> Switch;
    pub fn is_on(&self) -> bool;
    pub fn set(&mut self, on: bool);
    pub fn touched(&mut self, w: Widget, t: &Touch) -> bool;
}

pub const GAP: u16;
pub fn height() -> u16;
pub fn split_bottom(area: Rect) -> (Rect, Rect);
pub fn columns(row: Rect, n: u16) -> Vec<Rect>;
pub fn disc(x: i16, y: i16, d: u16, color: Color) -> canvas::Op;
```

A button's `Look` says what the button is for:

| Look | For | Drawn |
| --- | --- | --- |
| `Primary` | The screen's main action ("Save", "Set time"). | Filled in the selection colors. |
| `Secondary` | Any other action. | Outlined, as the softkey buttons are. |
| `Destructive` | An action that deletes or erases ("Delete alarm"). | Filled red. |
| `Call` | An action that places a call ("Call back"). | Filled green. |

Hand every `Event::Touch` to the controls' `touched`: a control looks pressed while a finger is on it, from `Touch::Down`, and `touched` returns `true` when the finger is lifted where it came down (`Touch::Tap`) on that control.  A finger that slides off touches nothing.  A `Switch` turns itself over when touched, and `is_on` then says how it is.

```rust
let (_, bottom) = controls::split_bottom(area);
self.save = Some(Button::new(ROOT, bottom, "Save", Look::Primary));

// In handle:
Event::Touch((w, t)) => {
    if self.save.as_mut().is_some_and(|b| b.touched(w, &t)) {
        self.save();
    }
}
```

- `with_icon` shows one of the phone's icons in place of a label, in the text's color: for a `Secondary` button.  `with_font` puts the label in another font (a number pad's digits in `Font::Large`); otherwise the label is in the font of the phone's buttons, or a smaller one if it does not fit.
- `set_on` draws a button as chosen, in the selection colors: for one of a set of choices, such as a day of the week or one of AM and PM.  A button that is not enabled (`set_enabled(false)`) is drawn dim and is never touched.
- `place` moves a button, as `ui::set_bounds` moves a widget.
- A `Switch` is drawn at the right of its bounds, which are what a finger may touch.  `in_row` makes one for the end of a row of a list: the switch has the hairline under it that the list draws under its rows.
- `height` is a button's height: a list's row, enough for a finger.  `split_bottom` returns `area` without its bottom, and the place of a full-width button across the bottom, `GAP` in from the edges.  `columns` divides a row into places for buttons side by side.
- `disc` is a filled circle, for a drawing of the app's own.

The screen's main action is one button across the bottom of the window (`split_bottom`); keep an action that deletes apart from it.

## Logging and storage

```rust
pub fn log_error(msg: &str)
pub fn log_warn(msg: &str)
pub fn log_info(msg: &str)
pub fn log_debug(msg: &str)
pub fn read_string(key: &str) -> Option<String>
```

The log functions call [`runtime::log`](runtime.md#log) with levels 0 to 3.

`read_string` returns the value of a `StorageLocation::Local` key from [`storage::get`](storage.md#get) as a `String`, or `None` for a key that isn't set or a value that isn't valid UTF-8.

## `xml`

```rust
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

pub fn parse(text: &str) -> Element;

impl Element {
    pub fn attr(&self, name: &str) -> &str;
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element>;
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>);
}
```

An XML reader in the `xml` module, for the output of [`config::app_config`](config.md#app_config) and [`config::contact_lists`](config.md#contact_lists).  Keeps elements, attributes, and nesting; skips text content, comments, and declarations.

- `parse` returns an unnamed root whose children are the top-level elements.  For malformed text, the result holds everything read before the error.
- `attr` returns an attribute's value, or `""` if missing.
- `children_named` iterates over the direct children with the name.
- `find_all` appends every descendant with the name to `out`, depth first.
