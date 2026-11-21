//! The Bluetooth application (D65): the paired headsets and the devices in
//! range, which the application looks for as soon as it opens, and the
//! question a pairing asks.  Bluetooth is on whenever the phone has a
//! controller.
//!
//! The title bar says when a search is under way.  Below it, the devices
//! are in two groups under small headings, the paired ones and the others
//! found, and the softkeys act on the highlighted device (Select is the
//! first softkey); every row of the list is a device.  A pairing question
//! is a dialog over the list.
//!
//! Everything shown comes from the `Bluetooth` state topic, and everything
//! done is a command of the Bluetooth domain; the screen keeps only the
//! highlighted device, its scroll position, and the PIN being typed.
//!
//! On the touch skin (D80) nothing is highlighted: each device's row is
//! taller, with its state under its name and a button for what is done to
//! it (Pair, Connect, or Disconnect), and a touch on the row does what
//! Select does to it.  A paired headset's row also has a button for its
//! own screen ([`DeviceScreen`]), which is where it is forgotten.  A
//! finger scrolls the list, and a button across the bottom searches.  The
//! pairing question has its own buttons, and the PIN is typed in an
//! [`EditScreen`].

use super::*;
use crate::gfx::{Rgb, mix};
use opendeskphone_state::{Bluetooth, BluetoothDevice, BluetoothPairing};

const SK_PAIR: u32 = 1;
const SK_CONNECT: u32 = 2;
const SK_DISCONNECT: u32 = 3;
const SK_BACK: u32 = 4;
const SK_SEARCH: u32 = 5;
const SK_FORGET: u32 = 6;
const SK_ACCEPT: u32 = 7;
const SK_REFUSE: u32 = 8;
const SK_ERASE: u32 = 9;
/// Touch: a paired headset's own screen.
const SK_DEVICE: u32 = 10;

/// The longest PIN Bluetooth takes.
const PIN_MAX: usize = 16;
/// The dots of the searching indicator, and how long each is lit.  (A
/// frame of this screen takes the D65 nearly 20 ms: measured, a step every
/// 100 ms kept the processor 18 % busy while a search ran, one every
/// 300 ms 6 %.)
const PULSE_DOTS: u64 = 3;
const PULSE_MS: u64 = 300;

/// Touch: the PIN field of the pairing question ([`Hit::Custom`]).  The
/// buttons of a device's row are `Hit::Custom(action << 16 | row)`, the
/// action a softkey's id.
const HIT_PIN: u32 = 1;

/// Touch: the height of a device's row (its name, and its state under it).
fn device_h(th: &Theme) -> i32 {
    if th.touch() { th.row_h + th.px(12) } else { th.row_h }
}

/// The height of a line of the list.
fn line_h(th: &Theme, line: &Line) -> i32 {
    match line {
        Line::Heading(_) if th.gray => th.line_h,
        Line::Heading(_) => th.px(26),
        Line::Device(_) => device_h(th),
    }
}

/// Touch: the PIN, typed for the device at `addr` while it asks for one
/// (its question may be gone by the time the PIN is typed).
fn pin_entry(addr: String, typed: &str) -> Box<dyn Screen> {
    EditScreen::boxed("bluetooth-pin", tr::PROV_ENTER_PIN, typed, InputMode::Numeric, move |cx, pin| {
        let pin: String = pin.chars().filter(char::is_ascii_digit).take(PIN_MAX).collect();
        if pin.is_empty() {
            return Nav::Stay;
        }
        if cx.st.bluetooth.pairing.as_ref().is_some_and(|q| q.addr == addr && q.pin_needed) {
            cx.post(Command::BluetoothAnswerPairing { addr: addr.clone(), accept: true, pin: Some(pin) });
        }
        Nav::Pop
    })
}

fn label(d: &BluetoothDevice) -> &str {
    if d.name.is_empty() { &d.addr } else { &d.name }
}

/// A device in the list.
#[derive(Clone, Copy)]
enum Row<'a> {
    Paired(&'a BluetoothDevice),
    /// Found by the search and not paired.
    Available(&'a BluetoothDevice),
}

impl<'a> Row<'a> {
    fn device(self) -> &'a BluetoothDevice {
        match self {
            Row::Paired(d) | Row::Available(d) => d,
        }
    }
}

/// The devices in the order shown: the paired ones, then the others found
/// (strongest first).
fn rows(bt: &Bluetooth) -> Vec<Row<'_>> {
    let available = bt.found.iter().filter(|d| !bt.paired.iter().any(|p| p.addr == d.addr));
    bt.paired.iter().map(Row::Paired).chain(available.map(Row::Available)).collect()
}

/// A line of the list: a group's heading, or the device of a row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Heading(&'static str),
    Device(usize),
}

fn lines(rows: &[Row]) -> Vec<Line> {
    let mut v = Vec::with_capacity(rows.len() + 2);
    for (i, row) in rows.iter().enumerate() {
        let first = match row {
            Row::Paired(_) => i == 0,
            Row::Available(_) => i == 0 || matches!(rows[i - 1], Row::Paired(_)),
        };
        if first {
            v.push(Line::Heading(match row {
                Row::Paired(_) => tr::BT_PAIRED_DEVICES,
                Row::Available(_) => tr::BT_AVAILABLE_DEVICES,
            }));
        }
        v.push(Line::Device(i));
    }
    v
}

/// The application's one screen; opening it starts a search, unless a
/// pairing is under way.
pub(crate) fn screen() -> Box<dyn Screen> {
    Box::new(BluetoothScreen {
        opened: false,
        selected: String::new(),
        index: 0,
        top: 0,
        drag: 0,
        pin: String::new(),
        pin_for: String::new(),
    })
}

struct BluetoothScreen {
    opened: bool,
    /// The highlighted device, by address, so that the highlight stays on
    /// it as devices are found and the list reorders; and its place, for
    /// when it goes.  (Touch: the device last touched.)
    selected: String,
    index: usize,
    /// The first line of the list in view.
    top: usize,
    /// Touch: how far a finger has dragged the list, in pixels, short of a
    /// line.
    drag: i32,
    /// The PIN being typed, and the device that asked for it.
    pin: String,
    pin_for: String,
}

/// What Select does to a device: the first softkey of its row.
fn primary(bt: &Bluetooth, row: Row) -> u32 {
    match row {
        Row::Paired(d) if d.addr == bt.connected => SK_DISCONNECT,
        Row::Paired(_) => SK_CONNECT,
        Row::Available(_) if bt.pairing_with.is_empty() => SK_PAIR,
        // One pairing at a time.
        Row::Available(_) => 0,
    }
}

impl BluetoothScreen {
    /// The highlighted row.
    fn current(&self, rows: &[Row]) -> Option<usize> {
        if rows.is_empty() {
            return None;
        }
        Some(rows.iter().position(|r| r.device().addr == self.selected).unwrap_or(self.index.min(rows.len() - 1)))
    }

    fn highlight(&mut self, rows: &[Row], i: usize) {
        self.index = i;
        self.selected = rows[i].device().addr.clone();
    }

    /// The question, with the PIN reset when it is a new one.
    fn question<'a>(&mut self, bt: &'a Bluetooth) -> Option<&'a BluetoothPairing> {
        let q = bt.pairing.as_ref()?;
        if self.pin_for != q.addr {
            self.pin_for = q.addr.clone();
            self.pin.clear();
        }
        Some(q)
    }

    fn answer(&mut self, cx: &Cx, accept: bool) -> Nav {
        let Some(q) = self.question(&cx.st.bluetooth) else { return Nav::Stay };
        let pin = if q.pin_needed && accept {
            if self.pin.is_empty() {
                // Touch: there are no digit keys; the PIN is typed first.
                return if cx.th.touch() { Nav::Push(pin_entry(q.addr.clone(), "")) } else { Nav::Stay };
            }
            Some(self.pin.clone())
        } else {
            None
        };
        cx.post(Command::BluetoothAnswerPairing { addr: q.addr.clone(), accept, pin });
        self.pin.clear();
        Nav::Stay
    }

    fn draw_list(&mut self, cx: &Cx, c: &mut Canvas, body: IRect, rows: &[Row]) {
        let th = cx.th;
        let bt = &cx.st.bluetooth;
        let lines = lines(rows);
        let height = |l: &Line| line_h(th, l);
        // Touch: nothing is highlighted.
        let current = if th.touch() { None } else { self.current(rows) };
        if let Some(i) = current {
            self.highlight(rows, i);
        }
        if th.touch() {
            // A finger scrolls the list ([`Screen::drag`]), no farther
            // than shows its last line at the bottom.
            let last = (0..lines.len()).find(|t| lines[*t..].iter().map(height).sum::<i32>() <= body.h).unwrap_or(0);
            self.top = self.top.min(last);
        } else {
            // Scroll so that the highlighted row is in view, with its
            // group's heading when there is room, and no room is left
            // below the end.
            let sel = lines.iter().position(|l| Some(*l) == current.map(Line::Device)).unwrap_or(0);
            let first = if sel > 0 && matches!(lines[sel - 1], Line::Heading(_)) { sel - 1 } else { sel };
            self.top = self.top.min(first).min(lines.len().saturating_sub(1));
            while self.top < sel && lines[self.top..=sel].iter().map(height).sum::<i32>() > body.h {
                self.top += 1;
            }
            while self.top > 0 && lines[self.top - 1..].iter().map(height).sum::<i32>() <= body.h {
                self.top -= 1;
            }
        }
        let total: i32 = lines.iter().map(height).sum();
        let bar = total > body.h;
        let w = if bar { body.w - th.scroll_w } else { body.w };
        let f = th.font(Font::Normal);
        let old = c.push_clip(body);
        let mut y = body.y;
        for line in &lines[self.top..] {
            if y >= body.bottom() {
                break;
            }
            let r = IRect::new(body.x, y, w, height(line));
            y += r.h;
            let i = match *line {
                Line::Heading(text) => {
                    // Small, dim, and sitting on the rows it heads.
                    let small = th.font(Font::Small);
                    let descent = small.height() - small.ascent as i32;
                    c.text(small, r.x + th.title_pad, r.bottom() - descent - if th.gray { 1 } else { th.px(6) }, text, th.secondary);
                    continue;
                }
                Line::Device(i) => i,
            };
            if th.touch() {
                touch_row(cx, c, r, rows[i], i);
                continue;
            }
            let selected = Some(i) == current;
            if selected {
                c.fill(if th.gray { r } else { IRect { h: r.h - 1, ..r } }, th.sel_bg);
            }
            let fg = if selected { th.sel_fg } else { th.fg };
            let (icon, rest) = r.split_left(th.icon + 2 * th.pad);
            let rest = IRect { w: rest.w - th.pad, ..rest };
            let device = rows[i].device();
            let (value, tint): (&str, Rgb) = match rows[i] {
                Row::Paired(d) => {
                    if let Some(ic) = th.icon("headset") {
                        c.icon_in(ic, icon, fg);
                    }
                    if d.addr == bt.connected { (tr::BT_CONNECTED, th.good) } else { (tr::BT_NOT_CONNECTED, th.secondary) }
                }
                Row::Available(d) if d.addr == bt.pairing_with => (tr::BT_PAIRING, th.accent),
                Row::Available(_) => ("", th.secondary),
            };
            if value.is_empty() {
                c.text_in(f, rest, label(device), fg, HAlign::Left);
            } else {
                widgets::key_value(c, th, rest, label(device), value, fg, if selected || th.gray { fg } else { tint });
            }
            if !th.gray {
                c.hline(r.x + 16, r.bottom() - 1, r.w - 24, th.border);
            }
        }
        c.restore_clip(old);
        let before: i32 = lines[..self.top].iter().map(height).sum();
        widgets::scroll_bar(c, th, body, total, body.h, before);
    }

    /// The pairing question, in a box over the list: whom it is with, and
    /// the six digits to compare or the PIN being typed.
    fn draw_question(&self, cx: &Cx, c: &mut Canvas, area: IRect, q: &BluetoothPairing) {
        let th = cx.th;
        let f = th.font(Font::Normal);
        let big = th.font(Font::Large);
        let w = if th.gray { area.w - 12 } else { th.px(380) };
        let text_w = w - 2 * th.pad;
        let field_h = if th.gray { th.line_h + 4 } else { th.px(36) };
        let gap = if th.gray { 2 } else { th.px(8) };
        let (text, extra) = match (q.pin_needed, q.passkey) {
            (true, _) => (tr::BT_TYPE_PIN, field_h + gap),
            (false, Some(_)) => (tr::BT_COMPARE, big.height() + gap),
            (false, None) => (tr::BT_ASKS, 0),
        };
        // Touch: the field is touched to type the PIN, and the answers are
        // buttons under the rest.
        let (field_h, extra) = if th.touch() && q.pin_needed { (th.touch_min, th.touch_min + gap) } else { (field_h, extra) };
        let buttons_h = if th.touch() { th.touch_min + 2 * gap } else { 0 };
        let lines = f.wrap(text, text_w).len() as i32;
        let h = (1 + lines) * th.line_h + extra + 2 * th.pad + gap + buttons_h;
        let inner = widgets::modal(c, th, area, w, h);
        if th.touch() {
            let (_, buttons) = inner.split_bottom(th.touch_min);
            let (refuse, accept) = buttons.split_left((buttons.w - gap) / 2);
            let accept = IRect { x: accept.x + gap, w: accept.w - gap, ..accept };
            let yes = if q.pin_needed { tr::OK } else { tr::BT_PAIR };
            widgets::button(c, th, refuse, tr::CANCEL, th.sk_bg, Some(th.sk_edge), th.sk_fg, Hit::Soft(SK_REFUSE));
            widgets::button(c, th, accept, yes, th.sel_bg, None, th.sel_fg, Hit::Soft(SK_ACCEPT));
        }
        let (head, rest) = inner.split_top(th.line_h);
        let (icon, title) = head.split_left(th.icon + th.pad);
        if let Some(i) = th.icon("headset") {
            c.icon_in(i, IRect { w: th.icon, ..icon }, th.accent);
        }
        let who = if q.name.is_empty() { &q.addr } else { &q.name };
        c.text_in(th.font(Font::Bold), title, &format!("{} {who}", tr::BT_PAIR_WITH), th.fg, HAlign::Left);
        let rest = IRect { y: rest.y + gap / 2, ..rest };
        let used = c.text_wrapped(f, rest, text, th.fg, HAlign::Left, th.line_h);
        let below = IRect::new(inner.x, rest.y + used + gap, inner.w, extra - gap);
        match (q.pin_needed, q.passkey) {
            (true, _) => {
                let field = IRect::new(below.x, below.y, below.w, field_h);
                widgets::text_field(c, th, field, &self.pin, self.pin.len(), InputMode::Numeric, true, false, "", Font::Normal);
                if th.touch() {
                    c.hit(field, Hit::Custom(HIT_PIN));
                }
            }
            (false, Some(p)) => c.text_in(big, below, &format!("{p:06}"), th.fg, HAlign::Center),
            (false, None) => {}
        }
    }
}

/// Touch: device number `i`, in its row `r`: its icon, its name with its
/// state under it, and at the right its button, and a paired headset's
/// way to its own screen.  A touch on the row elsewhere is [`Hit::Row`]
/// `i`.
fn touch_row(cx: &Cx, c: &mut Canvas, r: IRect, row: Row, i: usize) {
    let th = cx.th;
    let bt = &cx.st.bluetooth;
    c.hit(r, Hit::Row { list: 0, row: i as u32 });
    let (icon, rest) = r.split_left(th.icon + 2 * th.pad);
    let device = row.device();
    let (state, tint, buttons): (&str, Rgb, &[(u32, &str)]) = match row {
        Row::Paired(d) => {
            if let Some(ic) = th.icon("headset") {
                c.icon_in(ic, icon, th.fg);
            }
            if d.addr == bt.connected {
                (tr::BT_CONNECTED, th.good, &[(SK_DISCONNECT, tr::BT_DISCONNECT)])
            } else {
                (tr::BT_NOT_CONNECTED, th.secondary, &[(SK_CONNECT, tr::BT_CONNECT)])
            }
        }
        Row::Available(d) if d.addr == bt.pairing_with => (tr::BT_PAIRING, th.accent, &[]),
        // One pairing at a time.
        Row::Available(_) if !bt.pairing_with.is_empty() => ("", th.secondary, &[]),
        Row::Available(_) => ("", th.secondary, &[(SK_PAIR, tr::BT_PAIR)]),
    };
    // A paired headset's screen, at the right end (as a call log entry's
    // details are).
    let mut right = rest.right() - th.pad;
    if let Row::Paired(_) = row {
        let (_, more) = IRect { w: right - rest.x, ..rest }.split_right(th.back_w);
        if let Some(ic) = th.icon("info") {
            c.icon_in(ic, more, th.secondary);
        }
        c.hit(more, Hit::Custom(SK_DEVICE << 16 | i as u32));
        right = more.x;
    }
    // The buttons, from the right.
    let bf = th.font(Font::Bold);
    for &(id, text) in buttons.iter().rev() {
        let w = (bf.width(text) + th.px(32)).max(th.touch_min);
        let b = IRect::new(right - w, r.y + (r.h - th.touch_min) / 2, w, th.touch_min);
        widgets::button(c, th, b, text, th.sk_bg, Some(th.sk_edge), th.sk_fg, Hit::Custom(id << 16 | i as u32));
        right = b.x - th.pad;
    }
    // The name, and the state under it.
    let text = IRect::new(rest.x, r.y, right - rest.x, r.h);
    if state.is_empty() {
        c.text_in(th.font(Font::Normal), text, label(device), th.fg, HAlign::Left);
    } else {
        let (name, under) = text.split_top(r.h / 2 + th.px(4));
        c.text_in(th.font(Font::Normal), IRect { y: name.bottom() - th.line_h, h: th.line_h, ..name }, label(device), th.fg, HAlign::Left);
        c.text_in(th.font(Font::Small), IRect { h: th.line_h, ..under }, state, tint, HAlign::Left);
    }
    c.hline(r.x + th.px(16), r.bottom() - 1, r.w - th.px(24), th.border);
}

/// A message in the middle of `r`: a line, and a dimmer hint under it.
/// (Touch: [`menus::empty_state`].)
fn message(c: &mut Canvas, th: &Theme, r: IRect, text: &str, hint: &str) {
    if th.touch() {
        return menus::empty_state(c, th, r, text, hint);
    }
    let f = th.font(Font::Normal);
    let inner = r.inset_xy(th.pad * 3, 0);
    let hint_lines = if hint.is_empty() { 0 } else { f.wrap(hint, inner.w).len() as i32 };
    let y = r.y + (r.h - (1 + hint_lines) * th.line_h) / 2;
    c.text_in(th.font(Font::Bold), IRect::new(inner.x, y, inner.w, th.line_h), text, th.fg, HAlign::Center);
    let hint_r = IRect::new(inner.x, y + th.line_h, inner.w, hint_lines * th.line_h);
    c.text_wrapped(f, hint_r, hint, th.secondary, HAlign::Center, th.line_h);
}

/// "Searching" at the right of the title bar, followed by three dots that
/// light one after the other.
fn searching(c: &mut Canvas, th: &Theme, title: IRect, now_ms: u64) {
    let (dot, gap) = if th.gray { (3, 2) } else { (th.px(6), th.px(4)) };
    let dots_w = PULSE_DOTS as i32 * (dot + gap) - gap;
    let f = th.font(Font::Normal);
    let tw = f.width(tr::BT_SEARCHING);
    let x = title.right() - th.title_pad - dots_w - th.pad - tw;
    c.text_in(f, IRect::new(x, title.y, tw, title.h), tr::BT_SEARCHING, th.title_fg, HAlign::Left);
    let lit = now_ms / PULSE_MS % PULSE_DOTS;
    // On the text's baseline, as an ellipsis would be.
    let y = title.y + (title.h - f.height()) / 2 + f.ascent as i32 - dot;
    for k in 0..PULSE_DOTS {
        let r = IRect::new(x + tw + th.pad + k as i32 * (dot + gap), y, dot, dot);
        c.fill_round_aa(r, dot / 2, if k == lit { th.title_fg } else { mix(th.title_bg, th.title_fg, 3) });
    }
}

/// The text of an error as a sentence: its first letter capitalized.
fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

impl Screen for BluetoothScreen {
    fn name(&self) -> String {
        "bluetooth".into()
    }

    fn enter(&mut self, cx: &Cx) {
        if !self.opened {
            self.opened = true;
            let bt = &cx.st.bluetooth;
            if bt.present && !bt.discovering && bt.pairing.is_none() && bt.pairing_with.is_empty() {
                cx.post(Command::BluetoothDiscovery { on: true });
            }
        }
    }

    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        let bt = &cx.st.bluetooth;
        if cx.th.touch() {
            // The rows have their buttons, the question its own, the
            // search the one across the bottom, and the title bar the way
            // back.
            return Vec::new();
        }
        let blank = || soft(0, "");
        let back = soft(SK_BACK, tr::BACK);
        if !bt.present {
            return vec![blank(), blank(), blank(), back];
        }
        if let Some(q) = &bt.pairing {
            return if q.pin_needed {
                vec![soft(SK_ACCEPT, tr::OK), blank(), soft(SK_ERASE, tr::DELETE), soft(SK_REFUSE, tr::CANCEL)]
            } else {
                vec![soft(SK_ACCEPT, tr::BT_PAIR), blank(), blank(), soft(SK_REFUSE, tr::CANCEL)]
            };
        }
        let search = if bt.discovering { blank() } else { soft(SK_SEARCH, tr::BT_SEARCH) };
        let rows = rows(bt);
        match self.current(&rows).map(|i| rows[i]) {
            None => vec![search, blank(), blank(), back],
            Some(Row::Paired(d)) if d.addr == bt.connected => {
                vec![soft(SK_DISCONNECT, tr::BT_DISCONNECT), search, soft(SK_FORGET, tr::BT_FORGET), back]
            }
            Some(Row::Paired(_)) => vec![soft(SK_CONNECT, tr::BT_CONNECT), search, soft(SK_FORGET, tr::BT_FORGET), back],
            // One pairing at a time.
            Some(Row::Available(_)) if !bt.pairing_with.is_empty() => vec![blank(), search, blank(), back],
            Some(Row::Available(_)) => vec![soft(SK_PAIR, tr::BT_PAIR), search, blank(), back],
        }
    }

    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        let bt = &cx.st.bluetooth;
        let rows = rows(bt);
        let row = self.current(&rows).map(|i| rows[i]);
        match (id, row) {
            (SK_PAIR, Some(Row::Available(d))) if bt.pairing_with.is_empty() => {
                cx.post(Command::BluetoothDiscovery { on: false });
                cx.post(Command::BluetoothPair { addr: d.addr.clone() });
            }
            (SK_CONNECT, Some(Row::Paired(d))) => cx.post(Command::BluetoothConnect { addr: d.addr.clone() }),
            (SK_DISCONNECT, Some(Row::Paired(_))) => cx.post(Command::BluetoothDisconnect),
            (SK_FORGET, Some(Row::Paired(d))) => return Nav::Push(forget(cx, d)),
            (SK_DEVICE, Some(Row::Paired(d))) => return Nav::Push(Box::new(DeviceScreen { addr: d.addr.clone() })),
            (SK_SEARCH, _) if !bt.discovering => cx.post(Command::BluetoothDiscovery { on: true }),
            (SK_ACCEPT, _) => return self.answer(cx, true),
            (SK_REFUSE, _) => return self.answer(cx, false),
            (SK_ERASE, _) => {
                self.pin.pop();
            }
            (SK_BACK, _) => return Nav::Pop,
            _ => {}
        }
        Nav::Stay
    }

    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let bt = &cx.st.bluetooth;
        if let Some(q) = self.question(bt) {
            let pin = q.pin_needed;
            return match key {
                Key::Digit(d) if pin && d <= 9 => {
                    if self.pin.len() < PIN_MAX {
                        self.pin.push((b'0' + d) as char);
                    }
                    Nav::Stay
                }
                Key::Left if pin => self.softkey(cx, SK_ERASE),
                Key::Select => self.softkey(cx, SK_ACCEPT),
                Key::Cancel => self.softkey(cx, SK_REFUSE),
                Key::Up | Key::Down | Key::Left | Key::Right | Key::Digit(_) | Key::Star | Key::Pound => Nav::Stay,
                _ => Nav::Ignored,
            };
        }
        match key {
            Key::Up | Key::Down => {
                let rows = rows(bt);
                if let Some(i) = self.current(&rows) {
                    let mut st = widgets::ListState { selected: i, top: 0, drag: 0 };
                    st.step(if key == Key::Up { -1 } else { 1 }, rows.len(), true);
                    self.highlight(&rows, st.selected);
                }
                Nav::Stay
            }
            // Select is the first softkey's action.
            Key::Select | Key::Right => match self.softkeys(cx).first() {
                Some(&(id, _)) if id != 0 => self.softkey(cx, id),
                _ => Nav::Stay,
            },
            Key::Cancel | Key::Left => Nav::Pop,
            _ => Nav::Ignored,
        }
    }

    /// Touch: a device's row does what Select does to it, and its buttons
    /// what their softkeys do; the PIN field opens the PIN's entry.
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        let bt = &cx.st.bluetooth;
        let (action, i) = match hit {
            Hit::Custom(HIT_PIN) => {
                return match self.question(bt) {
                    Some(q) if q.pin_needed => Nav::Push(pin_entry(q.addr.clone(), &self.pin)),
                    _ => Nav::Stay,
                };
            }
            Hit::Custom(id) => (id >> 16, (id & 0xFFFF) as usize),
            Hit::Row { row, .. } => (0, row as usize),
            _ => return Nav::Ignored,
        };
        let rows = rows(bt);
        if bt.pairing.is_some() || i >= rows.len() {
            return Nav::Stay;
        }
        self.highlight(&rows, i);
        match if action == 0 { primary(bt, rows[i]) } else { action } {
            0 => Nav::Stay,
            id => self.softkey(cx, id),
        }
    }

    /// Touch: a finger scrolls the list, a line at a time.
    fn drag(&mut self, cx: &Cx, _from: Option<Hit>, _dx: i32, dy: i32) -> Nav {
        let bt = &cx.st.bluetooth;
        let lines = lines(&rows(bt));
        if bt.pairing.is_some() || lines.is_empty() {
            return Nav::Stay;
        }
        self.drag -= dy;
        loop {
            if self.drag > 0 && self.top + 1 < lines.len() && self.drag >= line_h(cx.th, &lines[self.top]) {
                self.drag -= line_h(cx.th, &lines[self.top]);
                self.top += 1;
            } else if self.drag < 0 && self.top > 0 && -self.drag >= line_h(cx.th, &lines[self.top - 1]) {
                self.drag += line_h(cx.th, &lines[self.top - 1]);
                self.top -= 1;
            } else {
                break;
            }
        }
        // Not beyond the first line or the last.
        if self.top == 0 {
            self.drag = self.drag.max(0);
        }
        if self.top + 1 >= lines.len() {
            self.drag = self.drag.min(0);
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let bt = &cx.st.bluetooth;
        c.fill(area, th.bg);
        let (title, mut body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, tr::BLUETOOTH);
        if !bt.present {
            if th.touch() {
                let hint = if bt.error.is_empty() {
                    tr::BT_NO_CONTROLLER_HINT.to_string()
                } else {
                    format!("{}\n\n{}", tr::BT_NO_CONTROLLER_HINT, sentence(&bt.error))
                };
                message(c, th, body, tr::BT_NO_CONTROLLER_TOUCH, &hint);
            } else {
                message(c, th, body, tr::BT_NO_CONTROLLER, &sentence(&bt.error));
            }
            return;
        }
        if bt.discovering {
            searching(c, th, title, cx.now_ms);
        }
        // Touch: the search, across the bottom; while one is under way the
        // button says so and does nothing.
        if th.touch() {
            let (rest, buttons) = menus::bottom_buttons(th, body, 1);
            if bt.discovering {
                menus::action_button(c, th, buttons[0], tr::BT_SEARCHING_BUTTON, menus::Role::Secondary, Hit::Nothing);
            } else {
                menus::action_button(c, th, buttons[0], tr::BT_SEARCH_DEVICES, menus::Role::Secondary, Hit::Soft(SK_SEARCH));
            }
            body = rest;
        }
        // Why the last pairing failed, in a band at the bottom.
        let mut list = body;
        if !bt.error.is_empty() {
            let (rest, band) = body.split_bottom(th.line_h + if th.gray { 1 } else { th.px(6) });
            list = rest;
            c.hline(band.x, band.y, band.w, th.border);
            let (icon, text) = band.inset_xy(th.title_pad, 0).split_left(th.icon + th.pad);
            if let Some(i) = th.icon("warning") {
                c.icon_in(i, IRect { w: th.icon, ..icon }, th.warn);
            }
            c.text_in(th.font(Font::Normal), text, &sentence(&bt.error), th.fg, HAlign::Left);
        }
        let rows = rows(bt);
        if !rows.is_empty() {
            self.draw_list(cx, c, widgets::body_rows(th, list), &rows);
        } else if bt.discovering {
            message(c, th, list, tr::BT_LOOKING, tr::BT_PAIRING_MODE);
        } else {
            message(c, th, list, tr::BT_NONE_FOUND, if th.touch() { tr::BT_PAIRING_MODE_TAP } else { tr::BT_PAIRING_MODE_SEARCH });
        }
        if let Some(q) = self.question(bt) {
            // Touch: the question is answered, and nothing else is touched
            // (the back button too would answer it).
            if th.touch() {
                c.hit(area, Hit::Nothing);
            }
            self.draw_question(cx, c, body, q);
        }
    }

    fn deadline(&self, cx: &Cx) -> Option<u64> {
        // The next step of the searching indicator.
        cx.st.bluetooth.discovering.then(|| (cx.now_ms / PULSE_MS + 1) * PULSE_MS)
    }
}

/// The question asked before the paired device `d` is forgotten, and the
/// forgetting.
fn forget(cx: &Cx, d: &BluetoothDevice) -> Box<dyn Screen> {
    let a = menus::Ask {
        question: format!("{} {}?", tr::BT_FORGET, label(d)),
        title: tr::BT_FORGET_TITLE,
        sentence: format!("{} {}", label(d), tr::BT_FORGET_SENTENCE),
        action: tr::BT_FORGET,
        destructive: true,
    };
    let addr = d.addr.clone();
    menus::ask(cx, a, move |cx| {
        cx.post(Command::BluetoothUnpair { addr: addr.clone() });
        Nav::Pop
    })
}

/// Touch: a paired headset's own screen: whether it is connected, and
/// its address; a button that connects or disconnects it; and at the
/// bottom, apart from the rest, a red one that forgets it (after asking).
/// The screen goes once the headset is no longer paired.
struct DeviceScreen {
    addr: String,
}

impl DeviceScreen {
    fn device<'a>(&self, bt: &'a Bluetooth) -> Option<&'a BluetoothDevice> {
        bt.paired.iter().find(|d| d.addr == self.addr)
    }
}

impl Screen for DeviceScreen {
    fn name(&self) -> String {
        "bluetooth-device".into()
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        let bt = &cx.st.bluetooth;
        let Some(d) = self.device(bt) else { return Nav::Stay };
        match id {
            SK_CONNECT => cx.post(Command::BluetoothConnect { addr: d.addr.clone() }),
            SK_DISCONNECT if bt.connected == d.addr => cx.post(Command::BluetoothDisconnect),
            SK_FORGET => return Nav::Push(forget(cx, d)),
            _ => {}
        }
        Nav::Stay
    }
    fn key(&mut self, _cx: &Cx, key: Key) -> Nav {
        if key == Key::Cancel { Nav::Pop } else { Nav::Ignored }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let bt = &cx.st.bluetooth;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        let Some(d) = self.device(bt) else {
            widgets::title_bar(c, th, title, tr::BLUETOOTH);
            return;
        };
        widgets::title_bar(c, th, title, label(d));
        let connected = bt.connected == d.addr;
        // What it is, as the status pages say it.
        let rows: [(&str, &str, Rgb); 2] = [
            (tr::STATUS, if connected { tr::BT_CONNECTED } else { tr::BT_NOT_CONNECTED }, if connected { th.good } else { th.secondary }),
            (tr::BT_ADDRESS, &d.addr, th.secondary),
        ];
        let row_h = th.row_h * 3 / 4;
        for (i, (k, v, tint)) in rows.iter().enumerate() {
            let r = IRect::new(body.x + th.title_pad, body.y + i as i32 * row_h, body.w - 2 * th.title_pad, row_h);
            widgets::key_value(c, th, IRect { h: r.h - 1, ..r }, k, v, th.fg, *tint);
            c.hline(r.x, r.bottom() - 1, r.w, th.border);
        }
        // Forget at the very bottom, and the connection above it with
        // room between.
        let (rest, forget) = menus::bottom_buttons(th, body, 1);
        let (_, connect) = menus::bottom_buttons(th, IRect { h: rest.h - th.row_h / 2, ..rest }, 1);
        let (id, text) = if connected { (SK_DISCONNECT, tr::BT_DISCONNECT) } else { (SK_CONNECT, tr::BT_CONNECT) };
        menus::action_button(c, th, connect[0], text, menus::Role::Primary, Hit::Soft(id));
        menus::action_button(c, th, forget[0], tr::BT_FORGET_DEVICE, menus::Role::Destructive, Hit::Soft(SK_FORGET));
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        // Forgotten (here or elsewhere): back to the list.
        self.device(&cx.st.bluetooth).is_none().then_some(cx.now_ms)
    }
    fn tick(&mut self, cx: &Cx) -> Nav {
        if self.device(&cx.st.bluetooth).is_none() { Nav::Pop } else { Nav::Stay }
    }
}
