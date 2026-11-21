//! Drawing and editing helpers shared by the native screens and the app
//! view renderer.

use crate::appview::{Font, InputMode};
use crate::gfx::{Canvas, HAlign, IRect, Rgb};
use crate::multitap::{MultiTap, Tap};
use crate::skin::Theme;
use crate::touch::Hit;
use opendeskphone_state::Key;

/// Selection and scroll position of a list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListState {
    pub selected: usize,
    pub top: usize,
    /// Touch: how far a finger has dragged the list, in pixels, short of
    /// a whole row ([`ListState::drag`]).
    pub drag: i32,
}

impl ListState {
    /// Moves the selection by `delta` among `len` rows, wrapping if asked.
    /// Returns whether it moved.
    pub fn step(&mut self, delta: i32, len: usize, wrap: bool) -> bool {
        if len == 0 {
            self.selected = 0;
            return false;
        }
        let old = self.selected.min(len - 1);
        let new = old as i32 + delta;
        let new = if wrap { new.rem_euclid(len as i32) } else { new.clamp(0, len as i32 - 1) } as usize;
        self.selected = new;
        new != old
    }

    /// Touch: a finger dragged the list `dy` pixels (up is negative: the
    /// rows follow the finger, so the list scrolls down).  The list moves
    /// by whole rows of `row_h` pixels; what is left of the way is kept
    /// for the next call.  [`list`] keeps the top row within the rows
    /// there are when it next draws.  Returns whether the list moved.
    pub fn drag(&mut self, dy: i32, row_h: i32, len: usize) -> bool {
        self.drag -= dy;
        let rows = self.drag / row_h.max(1);
        self.drag -= rows * row_h.max(1);
        let top = (self.top as i32 + rows).clamp(0, len.saturating_sub(1) as i32) as usize;
        let moved = top != self.top;
        self.top = top;
        moved
    }

    /// Touch: clamps to `len` rows without scrolling to the selection
    /// (the finger scrolls; nothing follows a cursor).
    pub fn settle_touch(&mut self, len: usize, visible: usize) {
        self.selected = self.selected.min(len.saturating_sub(1));
        self.top = self.top.min(len.saturating_sub(visible.max(1)));
    }

    /// Clamps to `len` rows and scrolls so the selection is within the
    /// `visible` rows.
    pub fn settle(&mut self, len: usize, visible: usize) {
        let visible = visible.max(1);
        self.selected = self.selected.min(len.saturating_sub(1));
        if self.selected < self.top {
            self.top = self.selected;
        }
        if self.selected >= self.top + visible {
            self.top = self.selected + 1 - visible;
        }
        self.top = self.top.min(len.saturating_sub(visible));
    }
}

/// A vertical scroll bar on the right edge of `r`.
pub fn scroll_bar(c: &mut Canvas, th: &Theme, r: IRect, total: i32, visible: i32, pos: i32) {
    if total <= visible || r.h <= 0 {
        return;
    }
    let track = IRect::new(r.right() - th.scroll_w, r.y, th.scroll_w, r.h);
    if !th.gray {
        c.fill(track, th.border);
    } else {
        c.vline(track.x, track.y, track.h, th.border);
    }
    let h = (r.h * visible / total).max(th.px(6));
    let y = r.y + (r.h - h) * pos / (total - visible).max(1);
    let thumb = IRect::new(track.x + 1, y, th.scroll_w - 1, h);
    c.fill(thumb, if th.gray { th.fg } else { th.secondary });
}

/// Draws rows of a list.  `draw_row(canvas, rect, index, selected, fg)`
/// paints the content of one row (its background is already painted).
/// Returns the number of visible rows.
///
/// On the touch skin each row shown is a region to touch,
/// [`Hit::Row`] of list 0 ([`list_as`] for another number), and the list
/// does not scroll to its selection: a finger scrolls it
/// ([`ListState::drag`]).
pub fn list(
    c: &mut Canvas,
    th: &Theme,
    r: IRect,
    len: usize,
    state: &mut ListState,
    row_h: i32,
    focused: bool,
    draw_row: impl FnMut(&mut Canvas, IRect, usize, bool, Rgb),
) -> usize {
    list_as(c, th, r, len, state, row_h, focused, 0, draw_row)
}

/// [`list`], for a screen with more than one list: the rows' regions are
/// [`Hit::Row`] of list `list`.
pub fn list_as(
    c: &mut Canvas,
    th: &Theme,
    r: IRect,
    len: usize,
    state: &mut ListState,
    row_h: i32,
    focused: bool,
    list: u32,
    mut draw_row: impl FnMut(&mut Canvas, IRect, usize, bool, Rgb),
) -> usize {
    let row_h = row_h.max(1);
    let visible = (r.h / row_h).max(1) as usize;
    if th.touch {
        state.settle_touch(len, visible);
    } else {
        state.settle(len, visible);
    }
    let bar = len > visible;
    let w = if bar { r.w - th.scroll_w } else { r.w };
    let old = c.push_clip(r);
    for i in 0..visible {
        let idx = state.top + i;
        if idx >= len {
            break;
        }
        let rr = IRect::new(r.x, r.y + i as i32 * row_h, w, row_h);
        if th.touch {
            c.hit(rr, Hit::Row { list, row: idx as u32 });
        }
        let sel = idx == state.selected && focused;
        if sel {
            if th.gray {
                c.fill(rr, th.sel_bg);
            } else {
                c.fill(IRect { h: rr.h - 1, ..rr }, th.sel_bg);
            }
        }
        draw_row(c, rr, idx, sel, if sel { th.sel_fg } else { th.fg });
        if !th.gray {
            // Stock: a hairline under each row.
            c.hline(rr.x + th.px(16), rr.bottom() - 1, rr.w - th.px(24), th.border);
        }
    }
    if bar {
        scroll_bar(c, th, r, len as i32, visible as i32, state.top as i32);
    }
    c.restore_clip(old);
    visible
}

/// A title bar across `r`.  On the touch skin it begins with the back
/// button ([`back_button`]), which does what the X key does on a phone
/// that has it.
pub fn title_bar(c: &mut Canvas, th: &Theme, r: IRect, title: &str) {
    c.fill(r, th.title_bg);
    let r = if th.touch {
        let (back, rest) = r.split_left(th.back_w);
        back_button(c, th, back, th.title_fg);
        // The title begins where the other skins' does, after the button.
        IRect { x: rest.x - th.title_pad, w: rest.w + th.title_pad, ..rest }
    } else {
        r
    };
    let f = th.font(Font::Title);
    // Stock: left-aligned on every skin (x 15 on color, 3 on gray).
    c.text_in(f, IRect { x: r.x + th.title_pad, w: r.w - 2 * th.title_pad, ..r }, title, th.title_fg, HAlign::Left);
}

/// The touch skin's back button, in `r`: an arrow, and a region that is
/// [`Key::Cancel`] (which leaves a screen, as the X key does).  For a
/// screen that draws its own title.
pub fn back_button(c: &mut Canvas, th: &Theme, r: IRect, fg: Rgb) {
    if let Some(icon) = th.icon("left") {
        c.icon_in(icon, r, fg);
    }
    c.hit(r, Hit::Key(Key::Cancel));
}

/// A button of the touch skin in `r`: a rounded shape in `fill` (with an
/// outline in `edge`, if given) with `label` in `fg` in its middle, and a
/// region that is `hit`.  A label starting with `icon:` draws that icon
/// instead.
pub fn button(c: &mut Canvas, th: &Theme, r: IRect, label: &str, fill: Rgb, edge: Option<Rgb>, fg: Rgb, hit: Hit) {
    let rad = th.px(6);
    match edge {
        Some(edge) => {
            c.fill_round_aa(r, rad, edge);
            c.fill_round_aa(r.inset(2), rad - 2, fill);
        }
        None => c.fill_round_aa(r, rad, fill),
    }
    if let Some(icon) = label.strip_prefix("icon:").and_then(|n| th.icon(n)) {
        c.icon_in(icon, r, fg);
    } else {
        // The smaller font for a label the usual one has no room for.
        let inner = r.inset_xy(th.px(4), 0);
        let f = th.font(Font::Bold);
        let f = if f.width(label) > inner.w { th.font(Font::Small) } else { f };
        c.text_in(f, inner, label, fg, HAlign::Center);
    }
    c.hit(r, hit);
}

/// The softkeys of the touch skin: buttons in rows of four above the
/// bottom of the screen ([`Theme::soft_button`]), one for each entry that
/// has a label, at the entry's position; each button's region is the
/// entry's [`Hit`].  The colors are the softkeys' of the other skins
/// ([`soft_style`]).  The background behind the buttons is the caller's.
pub fn soft_buttons(c: &mut Canvas, th: &Theme, keys: &[(Hit, String)]) {
    for (i, (hit, label)) in keys.iter().enumerate() {
        if label.is_empty() {
            continue;
        }
        let (fill, edge) = match soft_style(label) {
            SoftStyle::Normal => (th.sk_bg, Some(th.sk_edge)),
            SoftStyle::Go => (th.sk_go, None),
            SoftStyle::Stop => (th.sk_stop, None),
        };
        button(c, th, th.soft_button(i, keys.len()), label, fill, edge, th.sk_fg, *hit);
    }
}

/// How a softkey is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftStyle {
    Normal,
    /// Answer: green on the color skin.
    Go,
    /// End, ignore, reject: red on the color skin.
    Stop,
}

/// The style the stock firmware gives a native softkey label.
pub fn soft_style(label: &str) -> SoftStyle {
    use crate::strings as tr;
    if label == tr::ANSWER {
        SoftStyle::Go
    } else if [tr::END_CALL, tr::IGNORE, tr::REJECT].contains(&label) {
        SoftStyle::Stop
    } else {
        SoftStyle::Normal
    }
}

/// The four softkeys at the skin's measured positions
/// ([`Theme::softkey_rect`]).  All four shapes are always drawn, as on the
/// stock firmware; an empty label leaves its shape blank.  The background
/// behind the shapes is the caller's.  A label starting with `icon:` draws
/// that icon instead.
pub fn softkey_bar(c: &mut Canvas, th: &Theme, labels: &[String; 4]) {
    let f = th.font(Font::Normal);
    for (i, label) in labels.iter().enumerate() {
        let r = th.softkey_rect(i);
        let (fill, edge) = match soft_style(label) {
            SoftStyle::Normal => (th.sk_bg, Some(th.sk_edge)),
            SoftStyle::Go => (th.sk_go, None),
            SoftStyle::Stop => (th.sk_stop, None),
        };
        c.softkey_shape(r, fill, if th.gray { None } else { edge });
        if label.is_empty() {
            continue;
        }
        // The label sits in the rectangular part, above the point.
        let body = if th.gray { IRect { y: r.y + 1, h: r.h - 1, ..r } } else { IRect { y: r.y + 2, h: r.h - 5, ..r } };
        if let Some(icon) = label.strip_prefix("icon:").and_then(|n| th.icon(n)) {
            c.icon_in(icon, body, th.sk_fg);
        } else {
            // (On gray the shape is narrow and has no slanted sides.)
            c.text_in(f, body.inset_xy(if th.gray { 1 } else { 4 }, 0), label, th.sk_fg, HAlign::Center);
        }
    }
}

/// The loading bar of the boot screen: `permille` of `r` filled.  On the
/// color skin a bar with round, anti-aliased ends; on gray a pixel-fitted
/// outline with a solid fill inside, black and white only.
pub fn loading_bar(c: &mut Canvas, th: &Theme, r: IRect, permille: u32) {
    let p = permille.min(1000) as i32;
    if th.gray {
        // The outline, with its four corner pixels left out.
        c.hline(r.x + 1, r.y, r.w - 2, th.fg);
        c.hline(r.x + 1, r.bottom() - 1, r.w - 2, th.fg);
        c.vline(r.x, r.y + 1, r.h - 2, th.fg);
        c.vline(r.right() - 1, r.y + 1, r.h - 2, th.fg);
        // One pixel of background between the outline and the fill.
        let inner = r.inset(2);
        c.fill(IRect::new(inner.x, inner.y, inner.w * p / 1000, inner.h), th.fg);
        return;
    }
    let rad = r.h / 2;
    c.fill_round_aa(r, rad, th.load_track);
    // The fill keeps its round ends: it is never shorter than it is tall.
    let w = r.h + (r.w - r.h) * p / 1000;
    if p > 0 {
        c.fill_round_aa(IRect::new(r.x, r.y, w, r.h), rad, th.load_fill);
    }
}

/// A horizontal progress bar.
pub fn progress(c: &mut Canvas, th: &Theme, r: IRect, percent: u32) {
    c.frame(r, if th.gray { th.fg } else { th.border });
    let inner = r.inset(2);
    let w = inner.w * percent.min(100) as i32 / 100;
    c.fill(IRect::new(inner.x, inner.y, w, inner.h), if th.gray { th.fg } else { th.sel_bg });
}

/// A level meter of `max` segments with `level` lit (volume popups).
pub fn meter(c: &mut Canvas, th: &Theme, r: IRect, level: u32, max: u32) {
    let max = max.max(1) as i32;
    let gap = if th.gray { 1 } else { th.px(3) };
    let seg = (r.w - gap * (max - 1)) / max;
    for i in 0..max {
        let x = r.x + i * (seg + gap);
        let cell = IRect::new(x, r.y, seg, r.h);
        if (i as u32) < level {
            c.fill(cell, if th.gray { th.fg } else { th.sel_bg });
        } else {
            c.frame(cell, if th.gray { th.fg } else { th.border });
        }
    }
}

/// A centered modal box of the given size; returns its inner area.
pub fn modal(c: &mut Canvas, th: &Theme, area: IRect, w: i32, h: i32) -> IRect {
    let w = w.min(area.w - 4);
    let h = h.min(area.h - 2);
    let r = IRect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
    if th.gray {
        c.fill(r, th.bg);
        c.frame(r, th.fg);
        c.frame(r.inset(1), th.fg);
    } else {
        c.fill_round(r.inset(-th.scale), th.px(5), th.sk_edge);
        c.fill_round(r, th.px(4), th.modal_bg);
    }
    r.inset(th.pad)
}

// ------------------------------------------------------------ text entry

/// Result of feeding a key to a text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Not an editing key.
    Ignored,
    /// Consumed without changing the text (cursor moved, limit reached).
    Consumed,
    /// The text changed.
    Changed,
}

fn byte_at(text: &str, cursor: usize) -> usize {
    text.char_indices().nth(cursor).map(|(i, _)| i).unwrap_or(text.len())
}

/// Deletes the character before the cursor.
pub fn edit_backspace(text: &mut String, cursor: &mut u16, tap: &mut MultiTap) -> Edit {
    tap.reset();
    let cur = (*cursor as usize).min(text.chars().count());
    if cur == 0 {
        return Edit::Consumed;
    }
    let a = byte_at(text, cur - 1);
    let b = byte_at(text, cur);
    text.replace_range(a..b, "");
    *cursor = (cur - 1) as u16;
    Edit::Changed
}

/// Applies a key press to a one-line text field.
pub fn edit_key(
    text: &mut String,
    cursor: &mut u16,
    mode: InputMode,
    max_len: usize,
    tap: &mut MultiTap,
    key: Key,
    now_ms: u64,
) -> Edit {
    let len = text.chars().count();
    let cur = (*cursor as usize).min(len);
    match key {
        Key::Left => {
            tap.reset();
            *cursor = cur.saturating_sub(1) as u16;
            Edit::Consumed
        }
        Key::Right => {
            tap.reset();
            *cursor = (cur + 1).min(len) as u16;
            Edit::Consumed
        }
        Key::Digit(_) | Key::Star | Key::Pound => {
            let action = match mode {
                InputMode::Text | InputMode::Password => tap.tap(key, now_ms),
                InputMode::Dial => key.dial_char().map(Tap::Insert),
                InputMode::Numeric | InputMode::Pin => match key {
                    Key::Digit(d) => Some(Tap::Insert((b'0' + d) as char)),
                    Key::Star if mode == InputMode::Numeric => Some(Tap::Insert('.')),
                    _ => None,
                },
            };
            match action {
                None => Edit::Consumed,
                Some(Tap::Insert(ch)) => {
                    if len >= max_len {
                        tap.reset();
                        return Edit::Consumed;
                    }
                    text.insert(byte_at(text, cur), ch);
                    *cursor = (cur + 1) as u16;
                    Edit::Changed
                }
                Some(Tap::Replace(ch)) => {
                    if cur == 0 {
                        return Edit::Consumed;
                    }
                    let a = byte_at(text, cur - 1);
                    let b = byte_at(text, cur);
                    text.replace_range(a..b, ch.encode_utf8(&mut [0; 4]));
                    Edit::Changed
                }
            }
        }
        _ => Edit::Ignored,
    }
}

/// Draws a one-line text field with a cursor.  `pending` marks the
/// character before the cursor as still cycling (multi-tap).
#[allow(clippy::too_many_arguments)]
pub fn text_field(
    c: &mut Canvas,
    th: &Theme,
    r: IRect,
    text: &str,
    cursor: usize,
    mode: InputMode,
    focused: bool,
    pending: bool,
    placeholder: &str,
    font: Font,
) {
    let f = th.font(font);
    if th.gray {
        c.fill(r, th.bg);
        c.frame(r, th.fg);
    } else {
        c.fill_round(r, th.px(4), if focused { th.accent } else { th.sk_edge });
        c.fill_round(r.inset(th.scale), th.px(3), th.input_bg());
    }
    let inner = r.inset_xy(th.pad.max(4), 2);
    if text.is_empty() && !placeholder.is_empty() {
        c.text_in(f, inner, placeholder, th.secondary, HAlign::Left);
    }
    let masked = matches!(mode, InputMode::Password | InputMode::Pin);
    let shown: String = if masked {
        let n = text.chars().count();
        text.chars()
            .enumerate()
            .map(|(i, ch)| if pending && i + 1 == cursor.min(n) { ch } else { '\u{2022}' })
            .collect()
    } else {
        text.to_string()
    };
    let cur_byte = byte_at(&shown, cursor);
    let cur_x = f.width(&shown[..cur_byte]);
    // Scroll so the cursor stays visible.
    let scroll = (cur_x - (inner.w - 2)).max(0);
    let old = c.push_clip(inner);
    let y = inner.y + (inner.h - f.height()) / 2 + f.ascent as i32;
    c.text(f, inner.x - scroll, y, &shown, th.fg);
    if focused {
        let x = inner.x - scroll + cur_x;
        if pending && cursor > 0 {
            // Underline the cycling character.
            let prev = byte_at(&shown, cursor - 1);
            let w = f.width(&shown[prev..cur_byte]);
            c.hline(x - w, y + 2, w, th.fg);
        } else {
            c.vline(x, inner.y + 1, inner.h - 2, if th.gray { th.fg } else { th.accent });
        }
    }
    c.restore_clip(old);
}

/// Draws `label: value` on one row, value right-aligned.
pub fn key_value(c: &mut Canvas, th: &Theme, r: IRect, label: &str, value: &str, fg: Rgb, dim: Rgb) {
    let f = th.font(Font::Normal);
    let vw = f.width(value).min(r.w * 2 / 3);
    let (l, v) = r.split_right(vw);
    c.text_in(f, IRect { w: (l.w - th.pad).max(0), ..l }, label, fg, HAlign::Left);
    c.text_in(f, v, value, dim, HAlign::Right);
}

/// The rows area under a title bar: stock's color lists start two pixels
/// below the title (rows at y 42, 74, ...).
pub fn body_rows(th: &Theme, body: IRect) -> IRect {
    if th.gray { body } else { IRect { y: body.y + 2, h: body.h - 2, ..body } }
}
