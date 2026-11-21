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

Clears the window with `ui::clear(ROOT)`, which also removes the window's softkeys, key claims, and touch claim; claims `cancel` for the window; creates a `Text` in `Font::Title`, `title_height` tall, across the top; and returns the rectangle below the title.  On the touchscreen, the title starts to the right of the [back button](ui.md#touch) (`back_width`).

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
