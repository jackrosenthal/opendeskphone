//! The idle (home) screen: clock, status, and line-key tabs.
//!
//! On the touch skin (D80) the screen is upright and has no keys beside
//! it: from the top, the status icons, the clock and the date, an app's
//! idle view, the phone's lines as tiles (a touch on one is its line key),
//! and a button that opens the dialer; the softkey buttons are below.

use super::*;
use crate::gfx::Rgb;
use crate::IdleSoftkey;
use opendeskphone_hal::Blinker;
use opendeskphone_state::{AccountId, Accounts, BlfState, Blink, LampColor, LampState, LineKeyBinding, Network, Registration};

pub(crate) struct Idle;

/// One line-key tab as drawn.
pub(crate) struct Tab {
    pub label: String,
    pub icon: &'static str,
}

/// The line-key tabs, from the key state, or from the accounts while the
/// call core has not published keys yet.
pub(crate) fn line_tabs(cx: &Cx) -> Vec<Tab> {
    let st = cx.st;
    let account_tab = |id: AccountId, label: &str, active: bool| {
        let acct = st.accounts.get(id);
        let reg = acct.map(|a| &a.registration);
        let label = if !label.is_empty() {
            label.to_string()
        } else {
            acct.map(|a| if a.label.is_empty() { a.user.clone() } else { a.label.clone() }).unwrap_or_default()
        };
        Tab {
            label,
            icon: match reg {
                _ if active => "phone",
                // Stock: a person while idle, a handset while in a call.
                Some(Registration::Registered { .. }) => "contacts",
                Some(Registration::Failed { .. }) => "warning",
                _ => "unregistered",
            },
        }
    };
    if st.keys.line_keys.is_empty() {
        return st.accounts.accounts.iter().map(|a| account_tab(a.id, "", false)).collect();
    }
    st.keys
        .line_keys
        .iter()
        .take(cx.model.lines() as usize)
        .map(|k| match &k.binding {
            LineKeyBinding::Account(id) => account_tab(*id, &k.label, !k.calls.is_empty()),
            LineKeyBinding::Blf(entry) => {
                let e = st.presence.get(entry);
                let state = e.map(|e| e.state).unwrap_or_default();
                Tab {
                    label: if k.label.is_empty() { e.map(|e| e.label.clone()).unwrap_or_default() } else { k.label.clone() },
                    icon: match state {
                        BlfState::Ringing | BlfState::OnHoldRinging => "ring",
                        BlfState::InUse => "phone",
                        BlfState::OnHold => "hold",
                        BlfState::Idle => "registered",
                        _ => "unregistered",
                    },
                }
            }
            LineKeyBinding::Unused => Tab { label: String::new(), icon: "" },
        })
        .collect()
}

/// Status icons, left to right: (icon, count).  Stock shows the missed
/// call count at the top left of the idle screen; the others follow it.
pub(crate) fn status_icons(cx: &Cx) -> Vec<(&'static str, u32)> {
    let st = cx.st;
    let mut v = Vec::new();
    if st.call_log.missed_unseen > 0 {
        v.push(("phone-missed", st.call_log.missed_unseen));
    }
    if st.mwi.any_waiting() {
        v.push(("voicemail", st.mwi.total_new()));
    }
    if let Some(icon) = service_icon(&st.network, &st.accounts) {
        v.push((icon, 0));
    }
    if st.features.dnd {
        v.push(("dnd", 0));
    }
    if st.features.forward.any() {
        v.push(("forward", 0));
    }
    if st.alarms.icon {
        v.push(("alarm", 0));
    }
    if st.audio.headset_mode {
        v.push(("headset", 0));
    }
    if st.audio.muted {
        v.push(("mute", 0));
    }
    v
}

/// The status icon for why the phone cannot take calls, if it cannot.
///
/// `net-down` means the Ethernet link itself is down: no carrier on the
/// LAN jack.  `phone-unregistered` (a handset with a cross through it)
/// means the link is up, the phone has accounts, and none of them is
/// registered, whatever the reason (no address yet, the server does not
/// answer, the registration was refused).  With the link down only
/// `net-down` shows, since nothing can register without it.
///
/// A phone with no accounts (one used without a server), or with every
/// account switched off, shows neither with the link up: it has nothing
/// to register, so "not registered" would report a fault that is not one.
/// The line-key tabs show each account's own registration.
pub(crate) fn service_icon(network: &Network, accounts: &Accounts) -> Option<&'static str> {
    if !network.lan.up {
        return Some("net-down");
    }
    let mut enabled = accounts.accounts.iter().filter(|a| a.registration != Registration::Disabled).peekable();
    if enabled.peek().is_some() && !enabled.any(|a| a.registration.is_registered()) {
        return Some("phone-unregistered");
    }
    None
}

/// Draws the status icons in the header band `r`, from its left edge.
pub(crate) fn draw_status_icons(cx: &Cx, c: &mut Canvas, r: IRect, fg: Rgb) {
    let th = cx.th;
    let f = th.font(Font::Normal);
    // Stock: the first icon starts at x 8 (color); the count follows 8
    // pixels after it.
    let (start, gap, after) = if th.gray { (3, 2, 5) } else { (th.px(8), th.px(8), th.px(12)) };
    let mut x = r.x + start;
    let old = c.push_clip(r);
    for (name, count) in status_icons(cx) {
        let Some(icon) = th.icon(name) else { continue };
        let x0 = x;
        c.icon(icon, x, r.y + (r.h - icon.height()) / 2, fg);
        x += icon.width();
        if count > 0 {
            let s = count.to_string();
            x += gap;
            x += c.text(f, x, r.y + (r.h - f.height()) / 2 + f.ascent as i32, &s, fg);
        }
        // Touch: the message-waiting icon is the Messages key of a phone
        // that has one.
        if th.touch && name == "voicemail" {
            let w = (x - x0 + 2 * gap).max(th.touch_min);
            c.hit(IRect::new(((x0 + x - w) / 2).max(0), r.y, w, r.h.max(th.touch_min)), Hit::Key(Key::Messages));
        }
        x += after;
    }
    c.restore_clip(old);
}

/// Draws the line-key tabs down the right edge, from `x`.  Tab `i` is at
/// [`Theme::tab_rect_from`]`(x, i)`: a fixed pitch from the top of the
/// screen, so that each label sits beside its physical key.
pub(crate) fn draw_tabs(cx: &Cx, c: &mut Canvas, x: i32) {
    let th = cx.th;
    let f = th.font(Font::Normal);
    let edge = if th.gray { th.fg } else { 0x000001 };
    for (i, tab) in line_tabs(cx).iter().enumerate() {
        let r = th.tab_rect_from(x, i);
        if r.bottom() > th.soft_y {
            break;
        }
        if tab.label.is_empty() && tab.icon.is_empty() {
            continue;
        }
        // Touching a line does what its line key does on a phone that
        // has one.
        if th.touch {
            c.hit(r, crate::touch::Hit::Key(Key::Line(i as u8 + 1)));
        }
        // The tab is open to the right edge of the screen and to the tab
        // above; its left and bottom edges are drawn, with the lower-left
        // corner cut (color) or rounded (gray) as on stock.
        let bottom = r.h - 1;
        for j in 0..r.h {
            // Border offset from the tab's left edge on this row, and how
            // many border pixels it has there.
            let (off, run) = if th.gray {
                match bottom - j {
                    0 => (7, r.w),
                    1 => (5, 2),
                    2 => (3, 2),
                    3 => (2, 1),
                    4 | 5 => (1, 1),
                    _ => (0, 1),
                }
            } else {
                match bottom - j {
                    0 => (4, r.w),
                    k @ 1..=3 => (3 - k, 2),
                    _ => (0, 1),
                }
            };
            let y = r.y + j;
            c.span(y, r.x + off, (r.x + off + run).min(r.right()), edge);
            if j != bottom {
                c.span(y, r.x + off + run, r.right(), th.tab_bg);
            }
        }
        let (left, right) = if th.gray { (6, 5) } else { (th.px(14), th.px(9)) };
        let mut text = IRect::new(r.x + left, r.y, r.w - left - right, r.h - 1);
        if let Some(icon) = th.icon(tab.icon) {
            let ir = IRect::new(r.right() - right - icon.width(), r.y, icon.width(), r.h - 1);
            c.icon_in(icon, ir, th.tab_fg);
            text.w -= icon.width() + 4;
        }
        c.text_in(f, text, &tab.label, th.tab_fg, HAlign::Left);
    }
}

/// The header band `r` on gray skins: black, left of the tab column, with
/// `text` (a call state) or the status icons.
pub(crate) fn gray_header(cx: &Cx, c: &mut Canvas, r: IRect, text: Option<&str>) {
    let th = cx.th;
    c.fill(r, th.title_bg);
    match text {
        Some(t) => c.text_in(
            th.font(Font::Normal),
            IRect { x: r.x + th.title_pad, w: r.w - 2 * th.title_pad, ..r },
            t,
            th.title_fg,
            HAlign::Left,
        ),
        None => draw_status_icons(cx, c, r, th.title_fg),
    }
}

/// Where the idle clock goes: the time's right edge and baseline, the
/// meridiem's left edge, and the date's center and baseline.  Color values
/// are the stock positions (digits at x 68..222, y 89..152; meridiem from
/// x 242; date centered on x 148 with its baseline at y 196).
pub(crate) struct ClockLayout {
    pub time_right: i32,
    pub time_base: i32,
    pub mer_x: i32,
    pub date_cx: i32,
    pub date_base: i32,
}

pub(crate) fn clock_layout(th: &Theme, area: IRect) -> ClockLayout {
    if !th.gray {
        return ClockLayout { time_right: 228, time_base: 152, mer_x: 240, date_cx: 148, date_base: 196 };
    }
    // Gray: no stock clock to match (stock shows a logo with a small time
    // line under it), so the block is centered in the area left of the
    // tabs.
    let big = th.clock_font();
    let digits = big.glyph('0').map(|g| g.top as i32).unwrap_or(big.ascent as i32);
    let block = digits + 4 + th.line_h;
    let top = area.y + ((area.h - block) / 2).max(0);
    let cx = area.x + area.w / 2;
    ClockLayout {
        time_right: cx, // centered: see `Idle::draw`
        time_base: top + digits,
        mer_x: 0,
        date_cx: cx,
        date_base: top + digits + 4 + (th.line_h + th.font(Font::Normal).ascent as i32) / 2,
    }
}

/// Height of the idle-screen area offered to an app's idle view.
pub(crate) fn idle_app_h(th: &Theme) -> i32 {
    if th.gray { 2 * th.line_h } else { th.px(36) }
}

/// Width of the idle-screen area offered to an app's idle view: the
/// width left of the tabs, and on the touch skin, which has its lines
/// below, the screen's.
pub(crate) fn idle_app_w(th: &Theme) -> i32 {
    if th.touch { th.width } else { th.idle_tab_x }
}

// ----------------------------------------------------------------- touch

/// Touch: the region of the idle screen's that opens the dialer.
const HIT_NEW_CALL: u32 = 1;

/// One of the phone's lines as the touch skin shows it.
pub(crate) struct Line {
    /// Its line key is `Key::Line(n)`.
    pub n: u8,
    pub label: String,
    /// What the line is doing, in words.
    pub status: String,
    pub icon: &'static str,
    /// What the lamp of its line key shows on a model that has one.
    pub lamp: LampState,
}

/// The lines that are in use (an account or a watched contact), from the
/// key state, or from the accounts while the call core has not published
/// keys yet.
pub(crate) fn lines(cx: &Cx) -> Vec<Line> {
    let st = cx.st;
    let registration = |id: AccountId| match st.accounts.get(id).map(|a| &a.registration) {
        Some(Registration::Registered { .. }) => tr::REGISTERED,
        Some(Registration::Registering) => tr::REGISTERING,
        Some(Registration::Disabled) => tr::DISABLED,
        _ => tr::NOT_REGISTERED,
    };
    let tabs = line_tabs(cx);
    if st.keys.line_keys.is_empty() {
        let accounts = st.accounts.accounts.iter().zip(tabs).enumerate();
        return accounts
            .map(|(i, (a, tab))| Line {
                n: i as u8 + 1,
                label: tab.label,
                status: registration(a.id).to_string(),
                icon: tab.icon,
                lamp: LampState::OFF,
            })
            .collect();
    }
    let keys = st.keys.line_keys.iter().zip(tabs);
    keys.filter_map(|(k, tab)| {
        let status = match &k.binding {
            LineKeyBinding::Account(id) => match k.calls.first().and_then(|call| st.calls.get(*call)) {
                Some(call) => calls::call_parts(call, cx.now_ms).0,
                None => registration(*id).to_string(),
            },
            LineKeyBinding::Blf(entry) => {
                let e = st.presence.get(entry);
                match e.map(|e| e.state).unwrap_or_default() {
                    BlfState::Ringing | BlfState::OnHoldRinging => tr::RINGING.to_string(),
                    BlfState::InUse => tr::BUSY.to_string(),
                    BlfState::OnHold => tr::ON_HOLD.to_string(),
                    BlfState::Idle => match e.map(|e| e.status_text.as_str()) {
                        Some(text) if !text.is_empty() => text.to_string(),
                        _ => tr::LINE_AVAILABLE.to_string(),
                    },
                    _ => tr::LINE_UNAVAILABLE.to_string(),
                }
            }
            LineKeyBinding::Unused => return None,
        };
        Some(Line { n: k.index + 1, label: tab.label, status, icon: tab.icon, lamp: k.lamp })
    })
    .collect()
}

/// The color a lamp shows at `now_ms`: none while it is off, or in the
/// dark part of its blinking.
fn lamp_color(th: &Theme, lamp: LampState, now_ms: u64) -> Option<Rgb> {
    let color = match lamp.color {
        LampColor::Off => return None,
        LampColor::Red => th.error,
        LampColor::Green => th.good,
        LampColor::Amber => th.warn,
    };
    Blinker::phase_on(lamp.blink, now_ms).then_some(color)
}

/// When the lamps drawn for these lines next change (they blink as
/// [`Blinker::phase_on`] has it).
pub(crate) fn lamps_deadline(lines: &[Line], now_ms: u64) -> Option<u64> {
    let next = |period: u64| now_ms - now_ms % period + period;
    let lit = lines.iter().filter(|l| l.lamp.color != LampColor::Off);
    lit.filter_map(|l| match l.lamp.blink {
        Blink::Solid => None,
        Blink::Slow => Some(next(500)),
        Blink::Fast => Some(next(250)),
        Blink::Wink if now_ms % 2000 < 125 => Some(now_ms - now_ms % 2000 + 125),
        Blink::Wink => Some(next(2000)),
    })
    .min()
}

/// The lamp of a line, as a bar in `r`: in the lamp's color while it is
/// lit, and a dim one while it is not.
fn lamp_bar(cx: &Cx, c: &mut Canvas, r: IRect, lamp: LampState) {
    let th = cx.th;
    let color = lamp_color(th, lamp, cx.now_ms).unwrap_or(crate::gfx::mix(th.tab_bg, th.tab_fg, 1));
    c.fill_round_aa(r, r.w.min(r.h) / 2, color);
}

/// Touch: a line as a tile in `r`, with its lamp, icon, label, and state.
/// A touch on it is its line key.
fn line_tile(cx: &Cx, c: &mut Canvas, r: IRect, line: &Line) {
    let th = cx.th;
    c.fill_round_aa(r, th.px(6), th.tab_bg);
    lamp_bar(cx, c, IRect::new(r.x + th.px(5), r.y + th.pad, th.px(4), r.h - 2 * th.pad), line.lamp);
    let mut text = IRect::new(r.x + th.px(14), r.y + th.px(6), r.w - th.px(14) - th.pad, r.h - th.px(12));
    if let Some(icon) = th.icon(line.icon) {
        let (ic, rest) = text.split_left(icon.width() + th.pad);
        c.icon_in(icon, IRect { w: icon.width(), ..ic }, th.tab_fg);
        text = rest;
    }
    let (top, bottom) = text.split_top(text.h / 2);
    c.text_in(th.font(Font::Bold), top, &line.label, th.tab_fg, HAlign::Left);
    c.text_in(th.font(Font::Small), bottom, &line.status, th.secondary, HAlign::Left);
    c.hit(r, Hit::Key(Key::Line(line.n)));
}

/// Touch: the lines side by side across `r`, each with its label over
/// its lamp (the call screen, where there is no room for the tiles).  A
/// touch on one is its line key.
pub(crate) fn lines_strip(cx: &Cx, c: &mut Canvas, r: IRect, lines: &[Line]) {
    let th = cx.th;
    for (line, cell) in lines.iter().zip(dialpad::columns(r, lines.len(), th.px(4))) {
        c.fill_round_aa(cell, th.px(6), th.tab_bg);
        let (label, lamp) = cell.inset_xy(th.px(4), th.px(4)).split_bottom(th.px(8));
        c.text_in(th.font(Font::Normal), label, &line.label, th.tab_fg, HAlign::Center);
        lamp_bar(cx, c, IRect::new(lamp.x + th.px(4), lamp.y + th.px(2), lamp.w - th.px(8), th.px(3)), line.lamp);
        c.hit(cell, Hit::Key(Key::Line(line.n)));
    }
}

impl Idle {
    /// The touch skin's idle screen.  What is touched is laid out from
    /// the bottom, above the softkey buttons (however many rows of them
    /// there are); the clock has the middle of what is left.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let m = dialpad::MARGIN;
        let inner = IRect::new(area.x + m, area.y, area.w - 2 * m, area.h);
        draw_status_icons(cx, c, IRect::new(inner.x - th.px(8), area.y, inner.w, th.header_h), th.wallpaper_fg);

        let call = IRect::new(inner.x, area.bottom() - th.pad - th.px(52), inner.w, th.px(52));
        dialpad::control(c, th, call, "keypad", tr::NEW_CALL, dialpad::Look::Go, Hit::Custom(HIT_NEW_CALL));
        let mut bottom = call.y - th.pad;

        let lines = lines(cx);
        if !lines.is_empty() {
            // Two to a row, in the order of the line keys.
            let (gap, h) = (th.px(6), th.px(56));
            let rows = lines.len().div_ceil(2) as i32;
            let top = bottom - rows * h - (rows - 1) * gap;
            let w = (inner.w - gap) / 2;
            for (i, line) in lines.iter().enumerate() {
                let (col, row) = ((i % 2) as i32, (i / 2) as i32);
                line_tile(cx, c, IRect::new(inner.x + col * (w + gap), top + row * (h + gap), w, h), line);
            }
            bottom = top - th.pad;
        }

        if let Some(view) = cx.idle_view {
            let app = IRect::new(area.x + (area.w - idle_app_w(th)) / 2, bottom - idle_app_h(th), idle_app_w(th), idle_app_h(th));
            let mut tree = view.batch_quiet();
            crate::appview_impl::render_over(&mut tree, c, th, app, cx.now_ms, th.wallpaper_fg);
            bottom = app.y;
        }

        // The clock and the date, in the middle of what is left under the
        // status icons.
        let civil = cx.civil();
        let big = th.clock_font();
        let sub = th.date_font();
        let clock_set = cx.st.system.time_synced || cx.unix > 1_600_000_000;
        let time = if clock_set { civil.time(cx.cfg.time_24h) } else { "--:--".to_string() };
        let mer = if clock_set { civil.meridiem(cx.cfg.time_24h) } else { "" };
        let digits = big.glyph('0').map(|g| g.top as i32).unwrap_or(big.ascent as i32);
        let top = area.y + th.header_h;
        let block = digits + th.pad + sub.height();
        let base = top + ((bottom - top - block) / 2).max(0) + digits;
        let mid = area.x + area.w / 2;
        let gap = th.px(4);
        let tw = big.width(&time);
        let mw = if mer.is_empty() { 0 } else { th.meridiem_font().width(mer) + gap };
        let x = mid - (tw + mw) / 2;
        c.text(big, x, base, &time, th.wallpaper_fg);
        if !mer.is_empty() {
            c.text(th.meridiem_font(), x + tw + gap, base, mer, th.wallpaper_fg);
        }
        if clock_set {
            let date = civil.date();
            c.text(sub, mid - sub.width(&date) / 2, base + th.pad + sub.ascent as i32, &date, th.wallpaper_fg);
        }
    }
}

impl Screen for Idle {
    fn name(&self) -> String {
        "idle".into()
    }
    fn wallpaper(&self) -> bool {
        true
    }
    fn motion(&self) -> bool {
        true
    }
    /// The configured softkeys ([`UiConfig::idle_softkeys`]); an action's
    /// id is its place in that list, from 1.
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        let keys = cx.cfg.idle_softkeys.iter().enumerate().filter_map(|(i, key)| {
            let label = match key {
                // Stock: "Missed Calls" while there are unseen missed calls.
                IdleSoftkey::CallLog if cx.st.call_log.missed_unseen > 0 => tr::MISSED_CALLS_KEY,
                IdleSoftkey::CallLog => tr::CALL_LOG,
                IdleSoftkey::Contacts => tr::CONTACTS,
                IdleSoftkey::Status => tr::STATUS,
                IdleSoftkey::Menu => tr::MENU,
                IdleSoftkey::Apps => tr::APPS,
                IdleSoftkey::Dnd if cx.st.features.dnd => tr::DND_OFF,
                IdleSoftkey::Dnd => tr::DND,
                IdleSoftkey::Forward => tr::FORWARD,
                IdleSoftkey::Redial => tr::REDIAL,
                // Only the color models have themes.
                IdleSoftkey::Theme if cx.th.gray => return None,
                IdleSoftkey::Theme => tr::THEME,
                IdleSoftkey::App(name) => {
                    // An app the phone does not have, or one that is
                    // switched off, has no softkey.
                    let app = cx.st.apps.installed.iter().find(|a| a.name == *name)?;
                    if app.display_name.is_empty() { name } else { &app.display_name }
                }
            };
            Some(soft(i as u32 + 1, label))
        });
        keys.collect()
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        let Some(key) = (id as usize).checked_sub(1).and_then(|i| cx.cfg.idle_softkeys.get(i)) else {
            return Nav::Stay;
        };
        match key {
            IdleSoftkey::CallLog => Nav::Push(menus::call_log()),
            IdleSoftkey::Contacts => {
                cx.post(Command::LaunchApp { name: "contacts".into() });
                Nav::Stay
            }
            IdleSoftkey::Status => Nav::Push(menus::about()),
            IdleSoftkey::Menu => Nav::Push(menus::main_menu()),
            IdleSoftkey::Apps => Nav::Push(menus::launcher()),
            IdleSoftkey::Dnd => {
                cx.post(Command::SetDnd { on: !cx.st.features.dnd });
                Nav::Stay
            }
            IdleSoftkey::Forward => Nav::Push(menus::forward_menu()),
            IdleSoftkey::Redial => {
                cx.post(Command::Redial);
                Nav::Stay
            }
            IdleSoftkey::Theme if cx.th.gray => Nav::Stay,
            IdleSoftkey::Theme => Nav::Push(menus::themes(cx.cfg)),
            IdleSoftkey::App(name) => {
                cx.post(Command::LaunchApp { name: name.clone() });
                Nav::Stay
            }
        }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match key {
            Key::Digit(_) | Key::Star | Key::Pound => {
                let mut d = calls::Dialer::new(calls::Purpose::Call, None);
                d.type_char(cx, key.dial_char().unwrap_or('0'));
                Nav::Push(Box::new(d))
            }
            Key::Select => Nav::Push(menus::main_menu()),
            Key::Up | Key::Down => Nav::Push(menus::call_log()),
            _ => Nav::Ignored,
        }
    }
    fn tap(&mut self, _cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            // The dialer, with nothing off-hook: the number is typed and
            // then called.
            Hit::Custom(HIT_NEW_CALL) => Nav::Push(Box::new(calls::Dialer::new(calls::Purpose::Call, None))),
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        if th.touch {
            return self.draw_touch(cx, c, area);
        }
        // The background (wallpaper) is already painted.
        let header = th.header_rect_to(th.idle_tab_x);
        if th.gray {
            gray_header(cx, c, header, None);
        } else {
            draw_status_icons(cx, c, header, th.wallpaper_fg);
        }
        draw_tabs(cx, c, th.idle_tab_x);

        // The clock area: left of the tabs, below the header band.
        let mut main = IRect::new(area.x, th.header_h, th.idle_tab_x - area.x, area.bottom() - th.header_h);
        if let Some(view) = cx.idle_view {
            let (rest, app) = main.split_bottom(idle_app_h(th));
            let mut tree = view.batch_quiet();
            if th.gray {
                crate::appview_impl::render(&mut tree, c, th, app, cx.now_ms);
            } else {
                // Over the wallpaper: the app's widgets without a fill,
                // text in the wallpaper's text color where the app sets
                // none.
                crate::appview_impl::render_over(&mut tree, c, th, app, cx.now_ms, th.wallpaper_fg);
            }
            main = rest;
        }
        let civil = cx.civil();
        let big = th.clock_font();
        let sub = th.date_font();
        let clock_set = cx.st.system.time_synced || cx.unix > 1_600_000_000;
        let time = if clock_set { civil.time(cx.cfg.time_24h) } else { "--:--".to_string() };
        let mer = if clock_set { civil.meridiem(cx.cfg.time_24h) } else { "" };
        let lay = clock_layout(th, main);
        let tw = big.width(&time);
        if th.gray {
            let mw = if mer.is_empty() { 0 } else { sub.width(mer) + 3 };
            let x = lay.date_cx - (tw + mw) / 2;
            c.text(big, x, lay.time_base, &time, th.wallpaper_fg);
            if !mer.is_empty() {
                c.text(sub, x + tw + 3, lay.time_base, mer, th.wallpaper_fg);
            }
        } else {
            // With a meridiem the time ends where stock's does; without
            // one it is centered over the date.
            let x = if mer.is_empty() { lay.date_cx - tw / 2 } else { lay.time_right - tw };
            c.text(big, x, lay.time_base, &time, th.wallpaper_fg);
            if !mer.is_empty() {
                c.text(th.meridiem_font(), lay.mer_x, lay.time_base, mer, th.wallpaper_fg);
            }
        }
        if clock_set {
            let date = civil.date();
            c.text(sub, lay.date_cx - sub.width(&date) / 2, lay.date_base, &date, th.wallpaper_fg);
        }
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        // Repaint at the next minute boundary.
        let into = cx.unix % 60;
        let minute = cx.now_ms + (60 - into) * 1000 + 50;
        // Touch: and when a line's lamp next blinks.
        let lamps = if cx.th.touch { lamps_deadline(&lines(cx), cx.now_ms) } else { None };
        Some(lamps.map_or(minute, |at| at.min(minute)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_state::{Account, ActiveHost, NetPhase, PortLink};

    fn network(link: bool, phase: NetPhase) -> Network {
        Network { phase, lan: PortLink { up: link, speed: if link { 100 } else { 0 }, full_duplex: link }, ..Default::default() }
    }

    fn accounts(regs: &[Registration]) -> Accounts {
        let mut a = Accounts::default();
        for (i, reg) in regs.iter().enumerate() {
            let mut acct = Account::new(AccountId(i as u8));
            acct.registration = reg.clone();
            a.accounts.push(acct);
        }
        a
    }

    fn registered() -> Registration {
        Registration::Registered { host: ActiveHost::Primary, expires_mono_ms: u64::MAX }
    }

    fn failed() -> Registration {
        Registration::Failed { code: 403, reason: "Forbidden".into(), retry_mono_ms: 0 }
    }

    #[test]
    fn link_down_shows_only_network_down() {
        for regs in [vec![], vec![Registration::Unregistered], vec![failed()], vec![registered()], vec![Registration::Disabled]] {
            for phase in [NetPhase::Down, NetPhase::Configuring, NetPhase::Up, NetPhase::Failed] {
                assert_eq!(service_icon(&network(false, phase), &accounts(&regs)), Some("net-down"), "{regs:?} {phase:?}");
            }
        }
    }

    #[test]
    fn link_up_and_no_account_registered_shows_the_crossed_handset() {
        // No address yet (the link is up; DHCP or discovery is running), an
        // address that failed, and an address with the registration not
        // (yet) made or refused.
        for phase in [NetPhase::Down, NetPhase::Configuring, NetPhase::Failed, NetPhase::Up] {
            for regs in [
                vec![Registration::Unregistered],
                vec![Registration::Registering],
                vec![failed()],
                vec![failed(), Registration::Unregistered],
                vec![Registration::Disabled, Registration::Registering],
            ] {
                assert_eq!(service_icon(&network(true, phase), &accounts(&regs)), Some("phone-unregistered"), "{regs:?} {phase:?}");
            }
        }
    }

    #[test]
    fn one_registered_account_shows_neither() {
        let up = network(true, NetPhase::Up);
        assert_eq!(service_icon(&up, &accounts(&[registered()])), None);
        assert_eq!(service_icon(&up, &accounts(&[failed(), registered()])), None);
        assert_eq!(service_icon(&up, &accounts(&[Registration::Disabled, registered()])), None);
    }

    #[test]
    fn nothing_to_register_shows_neither_with_the_link_up() {
        for phase in [NetPhase::Configuring, NetPhase::Up] {
            assert_eq!(service_icon(&network(true, phase), &accounts(&[])), None);
            assert_eq!(service_icon(&network(true, phase), &accounts(&[Registration::Disabled])), None);
        }
    }
}
