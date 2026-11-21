---
title: "ui"
---

## Window

`ROOT` (handle 0) is the app's window, above the softkey bar (on the touchscreen, above the softkey buttons).  The phone fills the window with the skin's background color, then paints the widgets: parents before children, and siblings in creation order.  Each widget is clipped to the widget's bounds and to the bounds of every ancestor.

The phone holds back changes made during `start` or `handle` and shows them when the call returns.  A repaint for another reason during the call (the window comes on screen, or the user switches themes) shows the widgets as the call has left them so far.  Changes made while the window is off screen appear when the window returns.

Functions do nothing for a handle that doesn't exist, or for `ROOT` where a widget is required.  A value set on a kind of widget that doesn't use the value is stored without effect.

## Widget kinds

| `WidgetKind` | Shows | Keys handled when focused |
| --- | --- | --- |
| `Group` | Nothing; holds child widgets. | None. |
| `Text` | One line, or wrapped lines after `set_wrap(w, true)`. | None. |
| `TextList` | Rows, one selected.  Scrolls to keep the selection visible, with a scroll bar when the rows don't fit. | `up` and `down` move the selection, wrapping at the ends.  `select` activates the selected row. |
| `Scroll` | Child widgets, scrolled vertically, with a scroll bar.  The content height is the lowest bottom edge of the visible children. | `up` and `down` scroll one `line_height`.  Scrolls to keep a focused descendant visible. |
| `Input` | A one-line text field with a cursor. | Digits, `*`, and `#` type.  `left` and `right` move the cursor.  `select` submits.  `up` and `down` focus the previous or next visible `Input` or `TextList`. |
| `Image` | An image, centered at natural size. | None. |

[`canvas::create`](canvas.md#create) creates one more kind, a canvas.

## Focus and keys

At most one widget in the window has the focus, set by [`set_focus`](#set_focus).  The phone handles each key press, while the window is on screen, by the first step that applies:

1. A notice from the phone takes `select`, `cancel`, and the keys below the display.
2. Line key 1 takes the app off the screen.  The volume keys, `Mute`, `Speaker`, and `Headset` go to the phone.
3. A key below the display triggers the softkey above that key, except in a [full-screen](#set_full_screen) window.
4. The first claim ([`claim_keys`](#claim_keys)) on the key, checked on the focused widget, then each ancestor, then the window, produces [`Event::Key`](app-events.md#key).
5. The focused widget handles the key, as under [Widget kinds](#widget-kinds).  If no widget has the focus and the window has exactly one visible `TextList` or `Input`, that widget handles the key.
6. An unclaimed `cancel` takes the app off the screen.
7. The phone handles any other key as on any screen (`redial`, for example), or ignores the key.

[`input::capture`](input.md) replaces steps 2 to 7: every key goes to the app.

## Touch

The D80 has a touchscreen and no keys ([`Skin::Touch`](types.md#metrics-and-skin); `keys` is `false` in the metrics).  The stock widgets work by touch with no help from the app:

| Touch | Effect |
| --- | --- |
| A tap on a row of a `TextList` | Focuses the list, selects the row (`WidgetEvent::Highlighted`, unless the row was selected), and activates the row (`WidgetEvent::Activated`). |
| A drag on a `TextList` | Scrolls the list, if the rows don't all fit. |
| A drag on a `Scroll`, or on anything in one | Scrolls the `Scroll`. |
| A tap on an `Input` | Focuses the input, and the phone's on-screen keyboard types into the input. |
| A tap on the keyboard's action key | Focuses the window's next `Input` (`WidgetEvent::Focused`), where the key says "Next".  From the last `Input`, where it says "Done", submits the input (`WidgetEvent::Submitted`) and puts the keyboard away. |
| A tap on a softkey button | [`Event::Softkey`](app-events.md#softkey), or, for "More", a menu of the softkeys that have no button. |
| A tap on the back button | The key `cancel`: `Event::Key(Key::Cancel)` if claimed, and otherwise the app leaves the screen. |
| A tap on the home button | The app leaves the screen for the home screen, as with the first line key on other models. |

A list row is `row_height` tall, which is tall enough for a finger.  A list doesn't scroll to keep the selection visible while the user scrolls by hand; `set_selected` scrolls the selected row into view.  A `Scroll` brings a newly focused descendant into view once.

The top `title_height` pixels of the window are the title bar, the same on every screen of the phone.  The phone paints the bar in the title's colors, under the app's widgets, and draws two buttons over its ends, each `back_width` wide: the back button at the left and the home button at the right.  Put the title between them, as [`screen`](helpers.md#screen) does: a `Text` from `back_width` to `width - back_width`, left-aligned, in `Font::Title`.  A widget of the window's own that lies wholly within the bar is drawn in the title's text color unless it sets a color.  Give nothing in the bar a background, and put nothing under the two buttons.

A [full-screen](#set_full_screen) window, where the app draws what it likes, has no title bar and no home button.  Its back button is an arrow on a round button of its own over the top-left corner, `back_width` wide and `title_height` tall, so that it shows on any picture.  Keep that corner free.

While a finger is on a softkey button, a button of the title bar, a key of the keyboard, or a row of a `TextList`, the phone shows it as pressed.  A widget that takes touches shows the finger itself, from `Touch::Down`.

A form submits from its last input: on `WidgetEvent::Submitted`, do what the form is for (as [`Form::focus_next`](helpers.md#form) returning `false` tells), so that the user needs no softkey after the keyboard.

To handle touches itself, a widget takes touches with [`claim_touch`](#claim_touch), and the touches arrive as [`Event::Touch`](app-events.md#touch).  A touch goes to the topmost widget under the finger that takes touches or is a `TextList`, an `Input`, or a `Scroll`.  Widgets of other kinds that don't take touches let a touch through to what is under them.  The window is under every widget, and gets a touch only if the window takes touches and no widget does.

An app written for keys needs changes only where a key has no counterpart on the screen: a key claimed with [`claim_keys`](#claim_keys) never arrives (except `cancel`), so check `keys` in the metrics and offer the action as a softkey, a list row, or a touch.

Two habits of a screen made for keys don't suit the touchscreen:

- A softkey that acts on the highlighted row ("Play", "Details", "Delete").  The user touches a row to act on it, so make `Activated` do the row's main action, and put the other actions on the screen that the row opens.  A row that was touched stays highlighted, but no softkey should depend on that.
- A softkey that does what `cancel` does ("Back", "Exit").  The back button is always there.

[`nav::softkeys`](helpers.md#navsoftkeys) leaves such softkeys out on the touchscreen.

The widgets have no buttons.  The SDK's [controls](helpers.md#controls) are buttons and switches drawn on canvases that take touches: for a screen's main action, one button across the bottom of the window; for a setting that is on or off, a switch; for one of a few choices, a row of buttons, one of them on.  A list there has no highlighted row until the user touches one: give a list the focus with [`nav::focus_list`](helpers.md#navfocus_list), unless its highlighted row is the value that is chosen.

## Softkeys

[`set_softkeys`](#set_softkeys) sets softkeys on a widget or the window.  The softkey bar shows the softkeys of the focused widget, or else of the nearest ancestor that has any, or else of the window.

Up to four softkeys fill the four positions in order.  With more than four, the bar shows three at a time and, in the fourth position, a "More" key that pages through the softkeys, wrapping around.

On the touchscreen, the softkeys are buttons in a row below the window, side by side: the buttons share the width of the row, and one softkey alone is as wide as the window.  With more than four softkeys, the row has the first three and "More", which opens a menu of the others over the window; nothing pages.

While an `Input` has the focus and the third position is empty, a phone with keys shows "Delete" there.  "Delete" deletes the character before the cursor and sends `WidgetEvent::Changed`.  The touchscreen has no "Delete": its keyboard has a backspace key.

## Idle window

With `has_idle`, an app has a second window, on the idle screen.  Widgets created under [`idle_root`](#idle_root), directly or through a descendant, belong to the idle window.  Unlike the main window:

- The size is small and fixed ([`idle_size`](#idle_size)).
- The phone draws no background.  Widgets appear over the wallpaper of the user's theme, and text takes the theme's color for text on the wallpaper.
- The window receives no keys.  Softkeys, key claims, and focus have no effect, and no `Event::Widget` arrives.
- The idle screen shows one idle window at a time.  Windows with at least one widget take turns of `idle_cycle_seconds` (a configuration setting; 10 seconds by default, and 0 hides all idle windows).  [`Event::IdleScreenShow` and `Event::IdleScreenHide`](app-events.md#idlescreenshow-and-idlescreenhide) report the turns.
- `set_full_screen`, `get_metrics`, `input::capture`, and `lamps` apply only to the main window.

## Functions

### `get_metrics`

```rust
pub fn get_metrics() -> Metrics
```


See [`Metrics`](types.md#metrics-and-skin).  `height` reflects [`set_full_screen`](#set_full_screen).

### `idle_size`

```rust
pub fn idle_size() -> Size
```


The size of the idle window: 296 by 36 on the color models, 160 by 34 on the D70, and 114 by 30 on the D40, D45, and D50.  Works with or without `has_idle`.

### `idle_root`

```rust
pub fn idle_root() -> Option<Widget>
```


The root of the app's idle window, `Some(0x8000_0000)`, or `None` without `has_idle`.

### `create`

```rust
pub fn create(kind: WidgetKind, parent: Widget, bounds: Rect) -> Widget
```


Creates a widget.  `parent` is `ROOT`, the idle window's root, or another widget; `bounds` is relative to the parent.  A new widget is visible, has no text, and uses `Font::Normal` and `Align::Left`.

Any widget can be a parent, but only `Group` and `Scroll` draw children.  If `parent` doesn't exist, the widget becomes a top-level widget of the window that the handle would belong to.  If `parent` is in the idle window and the app has none, `create` creates nothing and returns 0.

### `destroy`

```rust
pub fn destroy(w: Widget)
```


Destroys the widget and all descendants.  If one of them had the focus, no widget has the focus.  `destroy(ROOT)` does nothing; use `clear`.

### `clear`

```rust
pub fn clear(w: Widget)
```


Destroys all descendants of the widget.  `clear(ROOT)` also removes the window's softkeys, key claims, touch claim, and focus; `clear` on the idle window's root does the same for the idle window.  Images from `define_image` stay defined.

### `set_bounds`

```rust
pub fn set_bounds(w: Widget, bounds: Rect)
```


Moves and resizes the widget.

### `set_visible`

```rust
pub fn set_visible(w: Widget, visible: bool)
```


Shows or hides the widget and all descendants.  `up` and `down` skip a hidden `TextList` or `Input`.  Hiding the focused widget or an ancestor of the focused widget clears the focus, as `set_focus(ROOT)` does; showing the widget doesn't restore the focus.

### `set_focus`

```rust
pub fn set_focus(w: Widget)
```


Any widget kind can have the focus, which selects the softkeys shown and the widget where the search for key claims starts.  `ROOT`, or a handle that doesn't exist, clears the focus.  A focus change resets the softkey bar to the first page.

### `set_text`

```rust
pub fn set_text(w: Widget, text: &str)
```


Sets the text of a `Text`; on an `Input`, same as [`set_value`](#set_value).

Without wrapping, a `Text` draws one line, centered vertically.  With wrapping, lines are `line_height` apart, and the block of lines is centered vertically.

### `set_align`

```rust
pub fn set_align(w: Widget, align: Align)
```


Sets the horizontal alignment of a `Text`.  `Left` is the default.

### `set_font`

```rust
pub fn set_font(w: Widget, font: Font)
```


Sets the font of a `Text` or an `Input`.  `Font::Normal` is the default.  List rows always use `Font::Normal`.

### `set_wrap`

```rust
pub fn set_wrap(w: Widget, wrap: bool)
```


Turns word wrap on or off for a `Text`.  Wrapped text breaks between words onto as many lines as the width requires; lines beyond the height are clipped.

### `set_highlight`

```rust
pub fn set_highlight(w: Widget, on: bool)
```


Draws a `Text`, `Group`, `Scroll`, or `Image` in the skin's selection colors, as a list draws the selected row: the bounds filled with the selection background, and text in the selection text color.  `false` restores the default colors.  An `Input` ignores the setting.

### `set_items`

```rust
pub fn set_items(w: Widget, items: &[ListItem])
```


Replaces the rows of a `TextList`.  A `ListItem` has the row's `text`, a `detail` right-aligned at the right edge, and an `icon: Option<Image>` at the left; `detail` may be empty.

- If any item has an icon, the list has an icon column one `row_height` wide.
- If any item has a detail, the list has a detail column of 40% of the width.
- `Image::Icon(icon)` is a [built-in icon](#built-in-icons).  `Image::Picture(name)` is the image that [`define_image`](#define_image) defined as `name`, scaled down to fit a `row_height` square; a name without an image draws nothing.
- The selection keeps the row number, or moves to the last row if the list got shorter.  No `WidgetEvent::Highlighted` is sent.

### `set_selected`

```rust
pub fn set_selected(w: Widget, index: u32)
```


Selects a row of a `TextList`.  Rows are numbered from 0; an index past the end selects the last row.  No event is sent.

### `selected`

```rust
pub fn selected(w: Widget) -> Option<u32>
```


The selected row of a `TextList`.  `None` for a list with no rows, another kind of widget, or a handle that doesn't exist.

### `set_value`

```rust
pub fn set_value(w: Widget, value: &str)
```


Sets the text of an `Input`, as given (the input mode and maximum length don't apply), and moves the cursor to the end.  No `WidgetEvent::Changed` is sent.  On a `Text`, same as `set_text`.

### `value`

```rust
pub fn value(w: Widget) -> String
```


The text of an `Input` or a `Text`, or `""` for a handle that doesn't exist.  In the `Password` and `Pin` modes, the actual text, not the dots on screen.

### `set_input_mode`

```rust
pub fn set_input_mode(w: Widget, mode: InputMode)
```


Sets how the dial pad types into an `Input`:

| `InputMode` | Typing | Display |
| --- | --- | --- |
| `Text` (default) | Multi-tap: a key pressed again within one second cycles through the key's characters: `2` through `a b c 2 A B C`, `1` through punctuation, and `0` through a space and `0`. | As typed. |
| `Numeric` | Digits.  `*` types `.`, and `#` types nothing. | As typed. |
| `Dial` | Digits, `*`, and `#`. | As typed. |
| `Password` | Same as `Text`. | Dots, except for the character being cycled. |
| `Pin` | Digits only. | Dots. |

Existing text doesn't change.

### `set_max_length`

```rust
pub fn set_max_length(w: Widget, max: u32)
```


Sets the maximum number of characters that the user can type into an `Input`: 255 by default, and at most 65,535 (larger values count as 65,535).  At the limit, key presses are ignored.  Longer existing text is kept.

### `define_image`

```rust
pub fn define_image(name: &str, png: &[u8]) -> Result<(), Error>
```


Defines a named image for `Image::Picture(name)`, in `set_image` and in list item icons.  `png` is the contents of a PNG file.

Redefining a name affects later calls; a widget or list that shows the old image keeps the old image until the next `set_image` or `set_items`.  Definitions last for the instance.

The phone decodes the image during the call, so `Ok` means that the image can be displayed.  An image larger than the window is scaled down to fit, keeping the aspect ratio; images are never scaled up.  The grayscale skins render four shades, by brightness.

| Error | Cause |
| --- | --- |
| `Invalid("a picture needs a plain name")` | `name` is empty, is `.` or `..`, or contains `/`, `\`, or a NUL character. |
| `Invalid("a picture may be 512 KiB")` | `png` is larger than 512 KiB. |
| `Invalid("an app may define 32 pictures")` | The app already has 32 names, and `name` is a new one. |
| `Invalid("not a PNG picture the phone can show (at most 1,048,576 pixels)")` | `png` is not the contents of a PNG file, the file is corrupt, or the image has more than 1,048,576 pixels. |

A failed call changes nothing: an image defined earlier under the name remains.

### `set_image`

```rust
pub fn set_image(w: Widget, image: &Image)
```

```rust
pub enum Image { Icon(Icon), Picture(String) }
```

Sets an `Image` widget to a [built-in icon](#built-in-icons), or to the image that the app defined as `name` (`Picture(name)`).  A name without an image shows nothing, and a name that `define_image` would reject is ignored.  One-color built-in icons take the widget's text color.

### `claim_keys`

```rust
pub fn claim_keys(w: Widget, keys: &[Key])
```


Sets the keys ([`Key`](types.md#keys)) that the widget, or the window for `ROOT`, receives as [`Event::Key`](app-events.md#key) instead of the built-in behavior.  The list replaces the widget's earlier claims; an empty list removes them.  A key with a number out of range (`Digit(10)`, `Softkey(5)`, `Line(0)`) is dropped.

A widget's claims apply while the widget or a descendant has the focus; the window's claims always apply.  Claiming `Cancel` keeps `cancel` from taking the app off the screen.

Line key 1, the volume keys, `Mute`, `Speaker`, and `Headset` can't be claimed (see [`input::capture`](input.md)).  The four keys below the display can be claimed only in a full-screen window.

On a phone without keys ([`Metrics::keys`](types.md#metrics-and-skin) is `false`), the only key that arrives is `Cancel`, from the [back button](#touch).

### `claim_touch`

```rust
pub fn claim_touch(w: Widget, on: bool)
```

With `true`, the widget, or the window for `ROOT`, takes touches: what a finger does on the widget arrives as [`Event::Touch`](app-events.md#touch) instead of the built-in behavior, with places counted from the widget's top-left corner.  `false` restores the built-in behavior.  Any kind of widget can take touches, including a canvas, and a `TextList` or `Input` that takes touches no longer handles them itself.  See [Touch](#touch) for which widget gets a touch.

`clear(ROOT)` ends the window's claim.  On a phone without a touchscreen, `claim_touch` has no effect.

```rust
// A canvas the user taps and swipes.
let board = canvas::create(ROOT, Rect::new(0, 100, 400, 400));
ui::claim_touch(board, true);

// In `handle`:
Event::Touch((w, Touch::Tap(at))) if w == board => self.tap(at.x, at.y),
// (A game that reacts when the finger lands matches `Touch::Down(at)`.)
Event::Touch((w, Touch::DragEnd((from, to)))) if w == board => {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    if dx.abs() > dy.abs() {
        self.swipe(if dx > 0 { Dir::Right } else { Dir::Left });
    } else {
        self.swipe(if dy > 0 { Dir::Down } else { Dir::Up });
    }
}
```

### `set_full_screen`

```rust
pub fn set_full_screen(on: bool)
```


Expands the app's window to the whole display (`true`), or restores the area above the softkey bar (`false`).

- A full-screen window has no softkey bar.  The four keys below the display do nothing unless claimed as `Key::Softkey(1)` to `Key::Softkey(4)`.  On the touchscreen, a full-screen window has no softkey buttons, and keeps the back button.
- `get_metrics` reports the new height (272, 160, 120, or 1280).  Widgets keep their bounds; lay out the screen again.
- An unclaimed `cancel` and line key 1 still take the app off the screen.  Calls and notices from the phone cover a full-screen window like any other.

### `set_softkeys`

```rust
pub fn set_softkeys(w: Widget, keys: &[Softkey])
```


Sets the softkeys of a widget, or of the window for `ROOT`.  [`Event::Softkey`](app-events.md#softkey) reports the `id` of the pressed softkey.  An empty list removes a widget's softkeys, so the bar shows those of an ancestor or the window.  Each call resets the bar to the first page.  See [Softkeys](#softkeys).

## Built-in icons

The variants of `Icon`, by their names in the interface (`phone-in` is `Icon::PhoneIn`).  One-color icons, which take the text color:

`alarm`, `apps`, `backspace`, `brightness`, `calllog`, `clock`, `conference`, `contacts`, `desk`, `dnd`, `down`, `error`, `forward`, `handset`, `headset`, `hold`, `info`, `left`, `lock`, `more`, `mute`, `net-down`, `network`, `ok`, `page`, `park`, `phone`, `phone-in`, `phone-missed`, `phone-out`, `phone-unregistered`, `presence`, `registered`, `right`, `ring`, `settings`, `speaker`, `transfer`, `unregistered`, `up`, `update`, `voicemail`, and `warning`.

App icons, in color on the color models (`app-contacts` is `Icon::AppContacts`): `app-alarms`, `app-app`, `app-boxpusher`, `app-calendar`, `app-calllog`, `app-contacts`, `app-desksparrow`, `app-diagnostics`, `app-forward`, `app-hotdesking`, `app-info`, `app-lightup`, `app-network`, `app-paging`, `app-parking`, `app-queues`, `app-settings`, `app-status`, `app-update`, and `app-voicemail`.
