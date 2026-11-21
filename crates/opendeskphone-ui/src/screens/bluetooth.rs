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

/// The longest PIN Bluetooth takes.
const PIN_MAX: usize = 16;
/// The dots of the searching indicator, and how long each is lit.  (A
/// frame of this screen takes the D65 nearly 20 ms: measured, a step every
/// 100 ms kept the processor 18 % busy while a search ran, one every
/// 300 ms 6 %.)
const PULSE_DOTS: u64 = 3;
const PULSE_MS: u64 = 300;

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
    Box::new(BluetoothScreen { opened: false, selected: String::new(), index: 0, top: 0, pin: String::new(), pin_for: String::new() })
}

struct BluetoothScreen {
    opened: bool,
    /// The highlighted device, by address, so that the highlight stays on
    /// it as devices are found and the list reorders; and its place, for
    /// when it goes.
    selected: String,
    index: usize,
    /// The first line of the list in view.
    top: usize,
    /// The PIN being typed, and the device that asked for it.
    pin: String,
    pin_for: String,
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
                return Nav::Stay;
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
        let heading_h = if th.gray { th.line_h } else { th.px(26) };
        let height = |l: &Line| if matches!(l, Line::Heading(_)) { heading_h } else { th.row_h };
        let current = self.current(rows);
        if let Some(i) = current {
            self.highlight(rows, i);
        }
        // Scroll so that the highlighted row is in view, with its group's
        // heading when there is room, and no room is left below the end.
        let sel = lines.iter().position(|l| Some(*l) == current.map(Line::Device)).unwrap_or(0);
        let first = if sel > 0 && matches!(lines[sel - 1], Line::Heading(_)) { sel - 1 } else { sel };
        self.top = self.top.min(first).min(lines.len().saturating_sub(1));
        while self.top < sel && lines[self.top..=sel].iter().map(height).sum::<i32>() > body.h {
            self.top += 1;
        }
        while self.top > 0 && lines[self.top - 1..].iter().map(height).sum::<i32>() <= body.h {
            self.top -= 1;
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
        let lines = f.wrap(text, text_w).len() as i32;
        let h = (1 + lines) * th.line_h + extra + 2 * th.pad + gap;
        let inner = widgets::modal(c, th, area, w, h);
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
            }
            (false, Some(p)) => c.text_in(big, below, &format!("{p:06}"), th.fg, HAlign::Center),
            (false, None) => {}
        }
    }
}

/// A message in the middle of `r`: a line, and a dimmer hint under it.
fn message(c: &mut Canvas, th: &Theme, r: IRect, text: &str, hint: &str) {
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
            (SK_FORGET, Some(Row::Paired(d))) => {
                let addr = d.addr.clone();
                return Nav::Push(Confirm::boxed(format!("{} {}?", tr::BT_FORGET, label(d)), move |cx| {
                    cx.post(Command::BluetoothUnpair { addr: addr.clone() });
                    Nav::Pop
                }));
            }
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

    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let bt = &cx.st.bluetooth;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, tr::BLUETOOTH);
        if !bt.present {
            message(c, th, body, tr::BT_NO_CONTROLLER, &sentence(&bt.error));
            return;
        }
        if bt.discovering {
            searching(c, th, title, cx.now_ms);
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
            message(c, th, list, tr::BT_NONE_FOUND, tr::BT_PAIRING_MODE_SEARCH);
        }
        if let Some(q) = self.question(bt) {
            self.draw_question(cx, c, body, q);
        }
    }

    fn deadline(&self, cx: &Cx) -> Option<u64> {
        // The next step of the searching indicator.
        cx.st.bluetooth.discovering.then(|| (cx.now_ms / PULSE_MS + 1) * PULSE_MS)
    }
}
