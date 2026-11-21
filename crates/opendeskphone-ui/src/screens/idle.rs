//! The idle (home) screen: clock, status, and line-key tabs.
//!
//! On the touch skin (D80) the screen is upright and has no keys beside
//! it.  From the top: the status icons (which are looked at, not touched:
//! the corner is where every other screen has its back button) with the
//! button for the ringer's volume at their right, and the clock and the
//! date.  From the bottom: the home buttons (the configured softkeys, each
//! with its icon), the button that opens the dialer, the phone's lines as
//! tiles, an app's idle view, and, in words, what the user has to know
//! before the next call: that the phone will not ring (Do Not Disturb,
//! forwarding) or cannot call (no network, no account), and what was
//! missed (calls, voicemail).

use super::*;
use crate::gfx::Rgb;
use super::dialpad::Look;
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
        c.icon(icon, x, r.y + (r.h - icon.height()) / 2, fg);
        x += icon.width();
        if count > 0 {
            let s = count.to_string();
            x += gap;
            x += c.text(f, x, r.y + (r.h - f.height()) / 2 + f.ascent as i32, &s, fg);
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
        if th.touch() {
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
    if th.touch() { th.width } else { th.idle_tab_x }
}

// ----------------------------------------------------------------- touch

/// Touch: the regions of the idle screen's own ([`Hit::Custom`]): the
/// button that opens the dialer, the button for the ringer's volume, the
/// buttons that turn Do Not Disturb and forwarding off, the missed calls,
/// the tile of a phone with no account, and, from `HIT_LINE`, the free
/// line whose line key is `Key::Line(n)`, as `HIT_LINE + n`.
const HIT_NEW_CALL: u32 = 1;
const HIT_VOLUME: u32 = 2;
const HIT_DND_OFF: u32 = 3;
const HIT_FORWARD_OFF: u32 = 4;
const HIT_MISSED: u32 = 5;
const HIT_NO_ACCOUNT: u32 = 6;
const HIT_LINE: u32 = 100;

/// Touch: a home button is the softkey action `HOME` and its place in
/// the configured softkeys, from 1 ([`Hit::Soft`]).  (Not the place
/// alone: the theme selector shows the idle screen above its own
/// softkeys, whose actions are numbered from 1 too.)
const HOME: u32 = 1000;

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
    /// An account's line with no call on it: the account.
    pub free: Option<AccountId>,
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
                free: Some(a.id),
            })
            .collect();
    }
    let keys = st.keys.line_keys.iter().zip(tabs);
    keys.filter_map(|(k, tab)| {
        let mut free = None;
        let status = match &k.binding {
            LineKeyBinding::Account(id) => match k.calls.first().and_then(|call| st.calls.get(*call)) {
                Some(call) => calls::call_parts(call, cx.now_ms).0,
                None => {
                    free = k.calls.is_empty().then_some(*id);
                    registration(*id).to_string()
                }
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
        Some(Line { n: k.index + 1, label: tab.label, status, icon: tab.icon, lamp: k.lamp, free })
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
/// A touch on it is its line key; on an account's line with no call, it
/// opens the dialer for that account, as the button below does for the
/// default one (nothing goes off-hook until the number is called).
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
    let free = line.free.is_some() && cx.st.calls.is_idle() && !cx.off_hook;
    c.hit(r, if free { Hit::Custom(HIT_LINE + u32::from(line.n)) } else { Hit::Key(Key::Line(line.n)) });
}

/// The Messages key, and the touch skin's voicemail button and card: the
/// voicemail app where it can reach the server (it is installed, and has
/// its application account), and otherwise a call to the voicemail number
/// of the default account, or of the first account that has one.  A phone
/// with neither says so.
pub(crate) fn open_voicemail(cx: &Cx) {
    let st = cx.st;
    if cx.cfg.voicemail_app_account && st.apps.installed.iter().any(|a| a.name == "voicemail") {
        cx.post(Command::LaunchApp { name: "voicemail".into() });
        return;
    }
    let has_number = |id: &AccountId| st.accounts.get(*id).is_some_and(|a| !a.voicemail_number.is_empty());
    let account = st.accounts.default_account.filter(has_number).or_else(|| st.accounts.accounts.iter().map(|a| a.id).find(has_number));
    match account {
        Some(account) => cx.post(Command::DialVoicemail { account: Some(account) }),
        None if calls::no_account_notice(cx) => {}
        None => cx.post(Command::Notify {
            level: opendeskphone_state::NoticeLevel::Info,
            title: tr::VOICEMAIL_TITLE.into(),
            text: tr::NO_VOICEMAIL_NUMBER.into(),
            timeout_ms: 6000,
        }),
    }
}

/// The label of an idle softkey; none for one the phone does not show.
fn softkey_label(cx: &Cx, key: &IdleSoftkey) -> Option<String> {
    let touch = cx.th.touch();
    let label = match key {
        // Stock: "Missed Calls" while there are unseen missed calls.
        // (Touch: the missed calls have a card of their own.)
        IdleSoftkey::CallLog if cx.st.call_log.missed_unseen > 0 && !touch => tr::MISSED_CALLS_KEY,
        IdleSoftkey::CallLog => tr::CALL_LOG,
        IdleSoftkey::Contacts => tr::CONTACTS,
        // (Touch: named for what it opens; "Status" is the presence app.)
        IdleSoftkey::Status if touch => tr::ABOUT,
        IdleSoftkey::Status => tr::STATUS,
        IdleSoftkey::Menu => tr::MENU,
        IdleSoftkey::Apps => tr::APPS,
        // Touch: the button names the thing, and is lit while it is on.
        IdleSoftkey::Dnd if touch => tr::DO_NOT_DISTURB,
        IdleSoftkey::Dnd if cx.st.features.dnd => tr::DND_OFF,
        IdleSoftkey::Dnd => tr::DND,
        IdleSoftkey::Forward => tr::FORWARD,
        IdleSoftkey::Redial => tr::REDIAL,
        IdleSoftkey::Voicemail => tr::VOICEMAIL_TITLE,
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
    Some(label.to_string())
}

/// Touch: the icon of a home button (one of the skin's; none if empty),
/// and how the button looks.
fn home_button(cx: &Cx, key: &IdleSoftkey) -> (String, Look) {
    let icon = match key {
        IdleSoftkey::CallLog => "calllog",
        IdleSoftkey::Contacts => "contacts",
        IdleSoftkey::Status => "info",
        IdleSoftkey::Menu => "more",
        IdleSoftkey::Apps => "apps",
        IdleSoftkey::Dnd => "dnd",
        IdleSoftkey::Forward => "forward",
        IdleSoftkey::Redial => "phone-out",
        IdleSoftkey::Voicemail => "voicemail",
        IdleSoftkey::Theme => "brightness",
        IdleSoftkey::App(name) => {
            let own = cx.st.apps.installed.iter().find(|a| a.name == *name).map(|a| a.icon.as_str()).unwrap_or("");
            if cx.th.icon(own).is_some() { own } else { "apps" }
        }
    };
    let look = match key {
        IdleSoftkey::Dnd if cx.st.features.dnd => Look::On,
        IdleSoftkey::Redial if calls::last_placed(cx).is_none() => Look::Off,
        _ => Look::Plain,
    };
    (icon.to_string(), look)
}

/// Touch: the rows the home buttons are in, and how many are in each: at
/// most four to a row, and the rows as even as they can be (seven are
/// four over three).
fn home_rows(n: usize) -> Vec<usize> {
    let rows = n.div_ceil(4);
    (0..rows).map(|row| (n + rows - 1 - row) / rows).collect()
}

/// Touch: rectangles side by side across `r`, `gap` apart, one for each
/// of `needs`: of one width, unless one needs more than that; such a one
/// has the width it needs, and the others share what is left.
fn home_columns(r: IRect, needs: &[i32], gap: i32) -> Vec<IRect> {
    let n = needs.len().max(1) as i32;
    let room = r.w - (n - 1) * gap;
    // The widest first: each takes its need or an even share of what the
    // wider ones left, whichever is more.
    let mut order: Vec<usize> = (0..needs.len()).collect();
    order.sort_by_key(|i| -needs[*i]);
    let mut widths = vec![0; needs.len()];
    let (mut left, mut count) = (room, n);
    for i in order {
        let w = needs[i].max(left / count).min(left);
        widths[i] = w;
        left -= w;
        count -= 1;
    }
    let mut x = r.x;
    let cells = widths.into_iter().map(|w| {
        let cell = IRect::new(x, r.y, w, r.h);
        x += w + gap;
        cell
    });
    cells.collect()
}

/// Touch: something the idle screen says in words.
enum Note {
    /// The phone cannot call or be called: a warning, with its icon.
    Down(&'static str, String),
    /// The phone will not ring: a warning, with a button that ends it.
    Quiet(&'static str, String, u32),
    /// Something was missed: its icon (red for a missed call), what, and
    /// more about it; a touch on it is `Hit`.
    Missed(&'static str, bool, String, String, Hit),
}

/// Touch: the text a warning is written in: black or white, whichever is
/// read better on the theme's warning color.
pub(crate) fn on_warn(th: &Theme) -> Rgb {
    let (black, white) = (0x000001, 0xFFFFFF);
    if crate::skin::contrast(th.warn, black) >= crate::skin::contrast(th.warn, white) { black } else { white }
}

impl Idle {
    /// Touch: what the idle screen says in words, the most important
    /// first.
    fn notes(cx: &Cx) -> Vec<Note> {
        let st = cx.st;
        let mut v = Vec::new();
        if !st.network.lan.up {
            v.push(Note::Down("net-down", tr::NO_LINK.to_string()));
        }
        if st.features.dnd {
            v.push(Note::Quiet("dnd", tr::DND_IS_ON.to_string(), HIT_DND_OFF));
        }
        if !st.features.forward.always.is_empty() {
            v.push(Note::Quiet("forward", format!("{} {}", tr::FORWARDING_ALL_TO, st.features.forward.always), HIT_FORWARD_OFF));
        }
        let missed = st.call_log.missed_unseen;
        if missed > 0 {
            let what = if missed == 1 { tr::ONE_MISSED_CALL.to_string() } else { format!("{missed} {}", tr::N_MISSED_CALLS) };
            // Who called last, and when: the time alone for a call of
            // today.
            let last = st.call_log.entries.iter().find(|e| e.kind == opendeskphone_state::CallLogKind::Missed);
            let more = last.map(|e| {
                let (now, then) = (cx.civil(), crate::time::Civil::from_unix(e.start_unix, cx.cfg.utc_offset_secs));
                let h24 = cx.cfg.time_24h;
                let when = if (now.year, now.month, now.day) == (then.year, then.month, then.day) {
                    format!("{} {}", then.time(h24), then.meridiem(h24)).trim_end().to_string()
                } else {
                    then.short(h24)
                };
                let who = if e.party.display().is_empty() { tr::UNKNOWN_CALLER } else { e.party.display() };
                format!("{who}, {when}")
            });
            v.push(Note::Missed("phone-missed", true, what, more.unwrap_or_default(), Hit::Custom(HIT_MISSED)));
        }
        if st.mwi.any_waiting() {
            let what = match st.mwi.total_new() {
                0 => tr::NEW_VOICEMAIL.to_string(),
                1 => tr::ONE_NEW_VOICEMAIL.to_string(),
                n => format!("{n} {}", tr::N_NEW_VOICEMAILS),
            };
            v.push(Note::Missed("voicemail", false, what, tr::TAP_TO_LISTEN.to_string(), Hit::Key(Key::Messages)));
        }
        v
    }

    /// Touch: a note, in `r`.
    fn draw_note(cx: &Cx, c: &mut Canvas, r: IRect, note: &Note) {
        let th = cx.th;
        let rad = th.px(6);
        let (ic, rest) = IRect { x: r.x + th.pad, w: r.w - 2 * th.pad, ..r }.split_left(th.icon + th.pad);
        let ic = IRect { w: th.icon, ..ic };
        match note {
            Note::Down(icon, text) | Note::Quiet(icon, text, _) => {
                let fg = on_warn(th);
                c.fill_round_aa(r, rad, th.warn);
                if let Some(i) = th.icon(icon) {
                    c.icon_in(i, ic, fg);
                }
                let mut text_r = rest;
                if let Note::Quiet(_, _, hit) = note {
                    // The button that ends it, at the right: as tall as
                    // the banner, so that it is not missed.
                    let w = th.font(Font::Bold).width(tr::TURN_OFF) + 2 * th.title_pad;
                    let (left, button) = r.split_right(w);
                    dialpad::control(c, th, button.inset(th.px(2)), "", tr::TURN_OFF, Look::Plain, Hit::Custom(*hit));
                    text_r = IRect { w: left.right() - th.pad - rest.x, ..rest };
                }
                // The larger type while it fits.
                let f = if th.font(Font::Title).width(text) <= text_r.w { th.font(Font::Title) } else { th.font(Font::Normal) };
                c.text_in(f, text_r, text, fg, HAlign::Left);
            }
            Note::Missed(icon, red, what, more, hit) => {
                c.fill_round_aa(r, rad, th.tab_bg);
                if let Some(i) = th.icon(icon) {
                    c.icon_in(i, ic, if *red { th.error } else { th.tab_fg });
                }
                let (text, chevron) = rest.split_right(th.icon);
                if let Some(i) = th.icon("right") {
                    c.icon_in(i, chevron, th.secondary);
                }
                let text = IRect { w: text.w - th.pad, ..text.inset_xy(0, th.px(4)) };
                if more.is_empty() {
                    c.text_in(th.font(Font::Bold), text, what, th.tab_fg, HAlign::Left);
                } else {
                    let (top, bottom) = text.split_top(text.h / 2);
                    c.text_in(th.font(Font::Bold), top, what, th.tab_fg, HAlign::Left);
                    c.text_in(th.font(Font::Small), bottom, more, th.secondary, HAlign::Left);
                }
                c.hit(r, *hit);
            }
        }
    }

    /// The touch skin's idle screen.  What is touched is laid out from
    /// the bottom; the clock has the middle of what is left.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let m = dialpad::MARGIN;
        let inner = IRect::new(area.x + m, area.y, area.w - 2 * m, area.h);
        let gap = th.pad;

        // The status icons, and at their right the ringer's volume: the
        // volume keys' popup, on a phone with no volume keys.
        let volume = IRect::new(area.right() - th.px(6) - th.back_w, area.y, th.back_w, th.header_h);
        draw_status_icons(cx, c, IRect::new(inner.x - th.px(8), area.y, volume.x - inner.x, th.header_h), th.wallpaper_fg);
        if let Some(icon) = th.icon("speaker") {
            c.icon_in(icon, volume, th.wallpaper_fg);
        }
        c.hit(volume, Hit::Custom(HIT_VOLUME));

        // The clock's and the date's height, and the lines'.
        let big = th.clock_font();
        let sub = th.date_font();
        let digits = big.glyph('0').map(|g| g.top as i32).unwrap_or(big.ascent as i32);
        let block = digits + th.pad + sub.height();
        let clock_top = area.y + th.header_h;
        let lines = lines(cx);
        let (tile_gap, tile_h) = (th.px(6), th.px(56));
        let tiles_h = lines.len().div_ceil(2).max(1) as i32 * (tile_h + tile_gap) + gap;

        // The home buttons: the configured softkeys, each with its icon.
        // (Not where the screen is shown in less than its height and the
        // clock would have no room: the theme selector shows it so.)
        let mut keys: Vec<(u32, &IdleSoftkey, String)> = self.keys(cx);
        let mut bottom = area.bottom() - m;
        let (cell_h, call_h) = (th.px(58), th.px(52));
        let home_h = home_rows(keys.len()).len() as i32 * (cell_h + gap);
        if clock_top + block + gap + tiles_h + call_h + gap + home_h > bottom {
            keys = Vec::new();
        }
        let rows = home_rows(keys.len());
        let mut next = keys.iter();
        let top = bottom - rows.len() as i32 * (cell_h + gap) + gap;
        for (row, n) in rows.iter().enumerate() {
            let r = IRect::new(inner.x, top + row as i32 * (cell_h + gap), inner.w, cell_h);
            // A label is never shrunk to fit: a button whose label is
            // wider than an even share of the row has the width it
            // needs, and the others share the rest.
            let row: Vec<&(u32, &IdleSoftkey, String)> = next.by_ref().take(*n).collect();
            let needs: Vec<i32> = row.iter().map(|(_, _, label)| th.font(Font::Bold).width(label) + 2 * th.pad).collect();
            for (cell, (id, key, label)) in home_columns(r, &needs, gap).into_iter().zip(row) {
                let (icon, look) = home_button(cx, key);
                dialpad::control(c, th, cell, &icon, label, look, Hit::Soft(HOME + *id));
            }
        }
        if !rows.is_empty() {
            bottom = top - gap;
        }

        let call = IRect::new(inner.x, bottom - call_h, inner.w, call_h);
        dialpad::control(c, th, call, dialpad::KEYPAD, tr::NEW_CALL, Look::Go, Hit::Custom(HIT_NEW_CALL));
        bottom = call.y - gap;

        if !lines.is_empty() {
            // Two to a row, in the order of the line keys.
            let rows = lines.len().div_ceil(2) as i32;
            let top = bottom - rows * tile_h - (rows - 1) * tile_gap;
            let w = (inner.w - tile_gap) / 2;
            for (i, line) in lines.iter().enumerate() {
                let (col, row) = ((i % 2) as i32, (i / 2) as i32);
                line_tile(cx, c, IRect::new(inner.x + col * (w + tile_gap), top + row * (tile_h + tile_gap), w, tile_h), line);
            }
            bottom = top - gap;
        } else if cx.st.accounts.accounts.is_empty() {
            // Where the lines would be: why the phone cannot call, and
            // the way to its setup.
            let r = IRect::new(inner.x, bottom - tile_h, inner.w, tile_h);
            c.fill_round_aa(r, th.px(6), th.tab_bg);
            let (ic, rest) = IRect { x: r.x + th.pad, w: r.w - 2 * th.pad, ..r }.split_left(th.icon + th.pad);
            if let Some(i) = th.icon("warning") {
                c.icon_in(i, IRect { w: th.icon, ..ic }, th.warn);
            }
            let (text, chevron) = rest.split_right(th.icon);
            if let Some(i) = th.icon("right") {
                c.icon_in(i, chevron, th.secondary);
            }
            let (first, second) = text.inset_xy(0, th.px(6)).split_top((tile_h - th.px(12)) / 2);
            c.text_in(th.font(Font::Bold), first, tr::NO_PHONE_ACCOUNT, th.tab_fg, HAlign::Left);
            c.text_in(th.font(Font::Small), second, tr::TAP_TO_SET_UP, th.secondary, HAlign::Left);
            c.hit(r, Hit::Custom(HIT_NO_ACCOUNT));
            bottom = r.y - gap;
        }

        if let Some(view) = cx.idle_view {
            let app = IRect::new(area.x + (area.w - idle_app_w(th)) / 2, bottom - idle_app_h(th), idle_app_w(th), idle_app_h(th));
            let mut tree = view.batch_quiet();
            crate::appview_impl::render_over(&mut tree, c, th, app, cx.now_ms, th.wallpaper_fg);
            bottom = app.y;
        }

        // What is said in words, directly above what is touched: as many
        // of the notes as leave the clock its room, the first ones first.
        let notes = Self::notes(cx);
        let note_h = th.row_h;
        let room = ((bottom - clock_top - block - gap) / (note_h + tile_gap)).max(0) as usize;
        let shown = &notes[..notes.len().min(room)];
        let top = bottom - shown.len() as i32 * (note_h + tile_gap);
        for (i, note) in shown.iter().enumerate() {
            Self::draw_note(cx, c, IRect::new(inner.x, top + i as i32 * (note_h + tile_gap), inner.w, note_h), note);
        }
        if !shown.is_empty() {
            bottom = top - gap + tile_gap;
        }

        // The clock and the date, in the middle of what is left under the
        // status icons.
        let civil = cx.civil();
        let clock_set = cx.st.system.time_synced || cx.unix > 1_600_000_000;
        let time = if clock_set { civil.time(cx.cfg.time_24h) } else { "--:--".to_string() };
        let mer = if clock_set { civil.meridiem(cx.cfg.time_24h) } else { "" };
        let base = clock_top + ((bottom - clock_top - block) / 2).max(0) + digits;
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

    /// The configured softkeys the phone shows ([`UiConfig::idle_softkeys`]):
    /// an action's id (its place in that list, from 1), the softkey, and
    /// its label.
    fn keys<'a>(&self, cx: &'a Cx) -> Vec<(u32, &'a IdleSoftkey, String)> {
        let keys = cx.cfg.idle_softkeys.iter().enumerate();
        keys.filter_map(|(i, key)| Some((i as u32 + 1, key, softkey_label(cx, key)?))).collect()
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
        // Touch: the screen draws them itself, as buttons with icons.
        if cx.th.touch() {
            return Vec::new();
        }
        self.keys(cx).into_iter().map(|(id, _, label)| (id, label)).collect()
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        let id = if id > HOME { id - HOME } else { id };
        let Some(key) = (id as usize).checked_sub(1).and_then(|i| cx.cfg.idle_softkeys.get(i)) else {
            return Nav::Stay;
        };
        let touch = cx.th.touch();
        match key {
            // Touch: with missed calls not yet looked at, the log opens
            // on them.
            IdleSoftkey::CallLog if touch && cx.st.call_log.missed_unseen > 0 => Nav::Push(menus::call_log_missed()),
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
                calls::redial(cx);
                Nav::Stay
            }
            IdleSoftkey::Voicemail => {
                open_voicemail(cx);
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
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        let dialer = |account| Nav::Push(Box::new(calls::Dialer::new(calls::Purpose::Call, account)));
        match hit {
            // The dialer, with nothing off-hook: the number is typed and
            // then called.
            Hit::Custom(HIT_NEW_CALL) => dialer(None),
            // The volume keys' popup: the ringer's volume, while nothing
            // else sounds.
            Hit::Custom(HIT_VOLUME) => {
                cx.post(Command::ShowScreen { name: "volume".into() });
                Nav::Stay
            }
            Hit::Custom(HIT_DND_OFF) => {
                cx.post(Command::SetDnd { on: false });
                Nav::Stay
            }
            Hit::Custom(HIT_FORWARD_OFF) => {
                let kind = opendeskphone_state::ForwardKind::Always;
                cx.post(Command::SetForward { kind, target: String::new(), no_answer_secs: None });
                Nav::Stay
            }
            Hit::Custom(HIT_MISSED) => Nav::Push(menus::call_log_missed()),
            // A phone used without a server goes back to its setup; one
            // that has a server shows what it was given.
            Hit::Custom(HIT_NO_ACCOUNT) => {
                if cx.st.provisioning.phase == opendeskphone_state::ProvisionPhase::Standalone {
                    cx.post(Command::StartProvisioning { server: None });
                    Nav::Stay
                } else {
                    Nav::Push(menus::accounts())
                }
            }
            // A free line: the dialer, for that line's account.
            Hit::Custom(n) if n > HIT_LINE => {
                let line = lines(cx).into_iter().find(|l| u32::from(l.n) == n - HIT_LINE);
                line.and_then(|l| l.free).map_or(Nav::Stay, |account| dialer(Some(account)))
            }
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        if th.touch() {
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
        let lamps = if cx.th.touch() { lamps_deadline(&lines(cx), cx.now_ms) } else { None };
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
