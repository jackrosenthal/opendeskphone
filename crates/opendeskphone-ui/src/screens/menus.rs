//! Menus built from the generic screens: main menu, call log, app
//! launcher, forward and DND, settings, status pages, and paging.

use super::*;
use opendeskphone_state::{
    AddrSource, AudioPath, CallLogKind, DspStatus, ForwardKind, Registration, TrialBoot, VolumeTarget,
};

fn set(cx: &Cx, id: &str, value: impl Into<String>) {
    cx.post(Command::SetSettings { settings: vec![(id.to_string(), value.into())] });
}

pub(crate) fn main_menu() -> Box<dyn Screen> {
    Menu::boxed("menu", tr::MAIN_MENU, |cx| {
        // Touch: the words of the home screen's buttons ("Apps"), and
        // "Status" kept for presence.
        let touch = cx.th.touch();
        let dnd = cx.st.features.dnd;
        let mut v = vec![
            Item::new(tr::CALL_LOG, |_| Nav::Push(call_log())).icon("calllog"),
            Item::new(if touch { tr::APPS } else { tr::APPLICATIONS }, |_| Nav::Push(launcher())).icon("apps"),
            Item::new(tr::CALL_FORWARD, |_| Nav::Push(forward_menu())).icon("forward"),
            Item::new(tr::DO_NOT_DISTURB, |cx| {
                cx.post(Command::SetDnd { on: !cx.st.features.dnd });
                Nav::Stay
            })
            .value(if dnd { tr::ON } else { tr::OFF })
            .switch(dnd)
            .icon("dnd"),
            Item::new(tr::PAGING_TITLE, |_| Nav::Push(paging())).icon("page"),
            Item::new(tr::SETTINGS, |_| Nav::Push(settings())).icon("settings"),
            Item::new(if touch { tr::PHONE_STATUS } else { tr::STATUS }, move |_| Nav::Push(status_menu(touch))).icon("info"),
        ];
        if cx.st.system.model.bluetooth() {
            v.insert(5, Item::new(tr::BLUETOOTH, |_| Nav::Push(bluetooth::screen())).icon("headset"));
        }
        v
    })
}

// -------------------------------------------------------------- call log

/// The call log.  Left and Right step through the filters (all, missed,
/// received, placed); Select dials the selected entry.
///
/// Touch: the filters are tabs under the title bar, a tap on an entry
/// dials it, and the button at the right of an entry shows its details
/// (from which it can be dialed too).  An entry of today shows its time
/// only.  The button across the bottom asks before it clears the log.
struct CallLogScreen {
    filter: Option<CallLogKind>,
    st: ListState,
    seen: bool,
}

pub(crate) fn call_log() -> Box<dyn Screen> {
    Box::new(CallLogScreen { filter: None, st: ListState::default(), seen: false })
}

/// The call log, opened on its missed calls.
pub(crate) fn call_log_missed() -> Box<dyn Screen> {
    Box::new(CallLogScreen { filter: Some(CallLogKind::Missed), st: ListState::default(), seen: false })
}

/// The filters, in the order Left and Right step through them.
const LOG_FILTERS: [Option<CallLogKind>; 4] =
    [None, Some(CallLogKind::Missed), Some(CallLogKind::Received), Some(CallLogKind::Placed)];

/// Touch: the regions of the call log: the tab of filter `i` is
/// `LOG_TAB + i`, the details button of entry `i` `LOG_DETAILS + i`.
const LOG_TAB: u32 = 1;
const LOG_DETAILS: u32 = 0x1_0000;

/// Touch: the region of the button that clears the log (the softkey's
/// action).
const LOG_CLEAR: u32 = 3;

fn filter_name(filter: Option<CallLogKind>) -> &'static str {
    match filter {
        None => tr::ALL,
        Some(CallLogKind::Missed) => tr::MISSED,
        Some(CallLogKind::Received) => tr::RECEIVED,
        Some(CallLogKind::Placed) => tr::PLACED,
    }
}

/// Touch: what an empty log says under the tab of `filter`.
fn no_calls(filter: Option<CallLogKind>) -> &'static str {
    match filter {
        None => tr::NO_CALLS,
        Some(CallLogKind::Missed) => tr::NO_MISSED_CALLS,
        Some(CallLogKind::Received) => tr::NO_RECEIVED_CALLS,
        Some(CallLogKind::Placed) => tr::NO_PLACED_CALLS,
    }
}

/// The time of day of `civil`, with AM or PM in 12-hour mode (`5:11 PM`).
fn time_of_day(civil: &crate::time::Civil, h24: bool) -> String {
    let m = civil.meridiem(h24);
    if m.is_empty() { civil.time(h24) } else { format!("{} {m}", civil.time(h24)) }
}

/// The question asked before the log is cleared.
fn clear_log_ask() -> Ask {
    Ask {
        question: tr::CLEAR_LOG_Q.into(),
        title: tr::CLEAR_LOG,
        sentence: tr::CLEAR_LOG_SENTENCE.into(),
        action: tr::CLEAR_LOG,
        destructive: true,
    }
}

/// Dials a call log entry.
fn dial_entry(cx: &Cx, e: &opendeskphone_state::CallLogEntry) -> Nav {
    if e.party.number.is_empty() {
        return Nav::Stay;
    }
    cx.post(Command::Dial {
        target: e.party.number.clone(),
        account: Some(e.account),
        path: cx.off_hook.then_some(AudioPath::Handset),
    });
    Nav::Home
}

impl CallLogScreen {
    fn rows<'a>(&self, cx: &'a Cx) -> Vec<&'a opendeskphone_state::CallLogEntry> {
        cx.st.call_log.entries.iter().filter(|e| self.filter.is_none_or(|k| e.kind == k)).collect()
    }
    fn dial(&self, cx: &Cx) -> Nav {
        match self.rows(cx).get(self.st.selected) {
            Some(e) => dial_entry(cx, e),
            None => Nav::Stay,
        }
    }

    /// Touch: the tabs of the filters, across `r`.
    fn draw_tabs(&self, c: &mut Canvas, th: &Theme, r: IRect) {
        c.fill(r, th.bg);
        let n = LOG_FILTERS.len() as i32;
        let f = th.font(Font::Bold);
        for (i, filter) in LOG_FILTERS.iter().enumerate() {
            let i = i as i32;
            let tab = IRect::new(r.x + r.w * i / n, r.y, r.w * (i + 1) / n - r.w * i / n, r.h);
            let on = *filter == self.filter;
            // The filter shown: its name in the accent, underlined.
            let bar = th.px(3);
            c.hline(tab.x, tab.bottom() - 1, tab.w, th.border);
            if on {
                c.fill(IRect::new(tab.x, tab.bottom() - bar, tab.w, bar), th.accent);
            }
            c.text_in(f, tab, filter_name(*filter), if on { th.accent } else { th.secondary }, HAlign::Center);
            c.hit(tab, Hit::Custom(LOG_TAB + i as u32));
        }
    }
}

impl Screen for CallLogScreen {
    fn list_state(&mut self, cx: &Cx) -> Option<(&mut ListState, usize)> {
        let len = self.rows(cx).len();
        Some((&mut self.st, len))
    }
    fn name(&self) -> String {
        "call_log".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: an entry is touched to dial it, its button shows its
        // details, the button across the bottom clears the log, and the
        // title bar has the way back.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::DIAL), soft(2, tr::DETAILS), soft(3, tr::CLEAR), soft(4, tr::BACK)]
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Custom(id) if (LOG_TAB..LOG_TAB + LOG_FILTERS.len() as u32).contains(&id) => {
                let filter = LOG_FILTERS[(id - LOG_TAB) as usize];
                if filter != self.filter {
                    self.filter = filter;
                    self.st = ListState::default();
                }
                Nav::Stay
            }
            Hit::Custom(id) if id >= LOG_DETAILS => {
                self.st.selected = (id - LOG_DETAILS) as usize;
                self.softkey(cx, 2)
            }
            // A row: selected and dialed (Select).
            _ => Nav::Ignored,
        }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 => self.dial(cx),
            2 => {
                let Some(e) = self.rows(cx).get(self.st.selected).map(|e| (*e).clone()) else { return Nav::Stay };
                let touch = cx.th.touch();
                let dial = e.clone();
                let info = Info::boxed("call_log_entry", tr::DETAILS, move |cx| {
                    let civil = crate::time::Civil::from_unix(e.start_unix, cx.cfg.utc_offset_secs);
                    vec![
                        (e.party.display().to_string(), String::new()),
                        (String::new(), e.party.number.clone()),
                        (
                            match e.kind {
                                CallLogKind::Missed => tr::MISSED,
                                CallLogKind::Received => tr::RECEIVED,
                                CallLogKind::Placed => tr::PLACED,
                            }
                            .to_string(),
                            format!("{} {}", civil.date(), time_of_day(&civil, cx.cfg.time_24h)),
                        ),
                        (tr::DURATION.to_string(), crate::time::duration(e.duration_secs as u64)),
                    ]
                });
                if touch {
                    Nav::Push(Box::new(EntryDetails { info, entry: dial }))
                } else {
                    Nav::Push(info)
                }
            }
            LOG_CLEAR => Nav::Push(ask(cx, clear_log_ask(), |cx| {
                cx.post(Command::ClearCallLog);
                Nav::Pop
            })),
            _ => Nav::Pop,
        }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let len = self.rows(cx).len();
        match key {
            Key::Up => {
                self.st.step(-1, len, true);
            }
            Key::Down => {
                self.st.step(1, len, true);
            }
            Key::Left | Key::Right => {
                // Cycle All -> Missed -> Received -> Placed.
                let order = [None, Some(CallLogKind::Missed), Some(CallLogKind::Received), Some(CallLogKind::Placed)];
                let i = order.iter().position(|f| *f == self.filter).unwrap_or(0) as i32;
                let d = if key == Key::Left { -1 } else { 1 };
                self.filter = order[(i + d).rem_euclid(4) as usize];
                self.st = ListState::default();
            }
            Key::Select => return self.dial(cx),
            Key::Cancel => return Nav::Pop,
            _ => return Nav::Ignored,
        }
        Nav::Stay
    }
    fn hook(&mut self, cx: &Cx, off_hook: bool) -> Nav {
        // Lifting the handset dials the selected entry.  (Touch: there is
        // no selected entry to see; the phone does what it does anywhere.)
        if off_hook && !self.rows(cx).is_empty() && !cx.th.touch() { self.dial(cx) } else { Nav::Ignored }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        if !self.seen {
            self.seen = true;
            if cx.st.call_log.missed_unseen > 0 {
                cx.post(Command::MarkMissedSeen);
            }
        }
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, mut body) = area.split_top(th.title_h);
        if th.touch() {
            widgets::title_bar(c, th, title, tr::CALL_LOG);
            let (tabs, rest) = body.split_top(th.row_h);
            self.draw_tabs(c, th, tabs);
            body = rest;
            // The log is cleared from any tab, while it has anything.
            if !cx.st.call_log.entries.is_empty() {
                let (rest, buttons) = bottom_buttons(th, body, 1);
                widgets::button_styled(c, th, buttons[0], tr::CLEAR_LOG, widgets::ButtonStyle::Secondary, Hit::Soft(LOG_CLEAR));
                body = rest;
            }
        } else {
            widgets::title_bar(c, th, title, &format!("{}  \u{2013}  {}", tr::CALL_LOG, filter_name(self.filter)));
        }
        let rows = self.rows(cx);
        if rows.is_empty() {
            if th.touch() {
                empty_state(c, th, body, no_calls(self.filter), tr::NO_CALLS_HINT);
            } else {
                c.text_in(th.font(Font::Normal), body, tr::NO_CALLS, th.secondary, HAlign::Center);
            }
            return;
        }
        let h24 = cx.cfg.time_24h;
        let off = cx.cfg.utc_offset_secs;
        if th.touch() {
            let today = cx.civil();
            widgets::list(c, th, body, rows.len(), &mut self.st, th.row_h, false, |c, r, i, _, fg| {
                touch_log_row(c, th, r, rows[i], i, fg, h24, off, &today);
            });
            return;
        }
        widgets::list(c, th, widgets::body_rows(th, body), rows.len(), &mut self.st, th.row_h, true, |c, r, i, sel, fg| {
            let e = rows[i];
            let (ic, rest) = r.split_left(th.icon + 2 * th.pad);
            let icon = match e.kind {
                CallLogKind::Missed => "phone-missed",
                CallLogKind::Received => "phone-in",
                CallLogKind::Placed => "phone-out",
            };
            if let Some(ico) = th.icon(icon) {
                let tint = if sel || th.gray {
                    fg
                } else if e.kind == CallLogKind::Missed {
                    th.error
                } else {
                    th.secondary
                };
                c.icon_in(ico, ic, tint);
            }
            let when = crate::time::Civil::from_unix(e.start_unix, off).short(h24);
            let name = if e.party.display().is_empty() { tr::UNKNOWN_CALLER } else { e.party.display() };
            widgets::key_value(c, th, IRect { w: rest.w - th.pad, ..rest }, name, &when, fg, if sel { fg } else { th.secondary });
        });
    }
}

fn log_icon(kind: CallLogKind) -> &'static str {
    match kind {
        CallLogKind::Missed => "phone-missed",
        CallLogKind::Received => "phone-in",
        CallLogKind::Placed => "phone-out",
    }
}

/// Touch: one entry of the call log in `r`: the kind's icon, the name
/// over the number, the time (with the date, unless it is `today`), and
/// the details button (entry `i`'s).
#[allow(clippy::too_many_arguments)]
fn touch_log_row(
    c: &mut Canvas,
    th: &Theme,
    r: IRect,
    e: &opendeskphone_state::CallLogEntry,
    i: usize,
    fg: crate::gfx::Rgb,
    h24: bool,
    off: i32,
    today: &crate::time::Civil,
) {
    let (ic, rest) = r.split_left(th.back_w);
    let (text, details) = rest.split_right(th.back_w);
    if let Some(ico) = th.icon(log_icon(e.kind)) {
        c.icon_in(ico, ic, if e.kind == CallLogKind::Missed { th.error } else { th.secondary });
    }
    if let Some(ico) = th.icon("info") {
        c.icon_in(ico, details, th.secondary);
    }
    c.hit(details, Hit::Custom(LOG_DETAILS + i as u32));
    let civil = crate::time::Civil::from_unix(e.start_unix, off);
    let same_day = (civil.year, civil.month, civil.day) == (today.year, today.month, today.day);
    let when = if same_day { time_of_day(&civil, h24) } else { civil.short(h24) };
    let name = e.party.display();
    // The name over the number; the number alone (or "Unknown") where
    // there is no name.
    let (first, second) = if name.is_empty() || name == e.party.number {
        (if e.party.number.is_empty() { tr::UNKNOWN_CALLER } else { e.party.number.as_str() }, "")
    } else {
        (name, e.party.number.as_str())
    };
    let small = th.font(Font::Small);
    let text = IRect { w: text.w - th.pad, ..text };
    let when_w = small.width(&when).min(text.w / 2);
    let (left, right) = text.split_right(when_w);
    let left = IRect { w: left.w - th.pad, ..left };
    if second.is_empty() {
        c.text_in(th.font(Font::Normal), left, first, fg, HAlign::Left);
    } else {
        let (top, bottom) = left.inset_xy(0, th.pad / 2).split_top((r.h - th.pad) / 2);
        c.text_in(th.font(Font::Normal), top, first, fg, HAlign::Left);
        c.text_in(small, bottom, second, th.secondary, HAlign::Left);
    }
    c.text_in(small, right, &when, th.secondary, HAlign::Right);
}

/// Touch: the details of a call log entry ([`Info`]), with a green
/// button across the bottom that calls the number ("Call 1003").
struct EntryDetails {
    info: Box<dyn Screen>,
    entry: opendeskphone_state::CallLogEntry,
}

/// Touch: the region of [`EntryDetails`]' button (the Dial softkey's
/// action).
const DETAILS_CALL: u32 = 1;

impl Screen for EntryDetails {
    fn name(&self) -> String {
        self.info.name()
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        if id == DETAILS_CALL { dial_entry(cx, &self.entry) } else { Nav::Stay }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        self.info.key(cx, key)
    }
    fn drag(&mut self, cx: &Cx, from: Option<Hit>, dx: i32, dy: i32) -> Nav {
        self.info.drag(cx, from, dx, dy)
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let number = &self.entry.party.number;
        if number.is_empty() {
            return self.info.draw(cx, c, area);
        }
        let (rest, buttons) = bottom_buttons(cx.th, area, 1);
        self.info.draw(cx, c, rest);
        widgets::button_styled(c, cx.th, buttons[0], &format!("{} {number}", tr::CALL), widgets::ButtonStyle::Call, Hit::Soft(DETAILS_CALL));
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        self.info.deadline(cx)
    }
}

// -------------------------------------------------------------- launcher

struct Launcher {
    st: ListState,
    /// Apps' own icons already decoded, by app, with the picture each was
    /// decoded from (`None`: unusable).
    files: std::collections::HashMap<String, (opendeskphone_state::Picture, Option<crate::image::Image>)>,
}

pub(crate) fn launcher() -> Box<dyn Screen> {
    Box::new(Launcher { st: ListState::default(), files: std::collections::HashMap::new() })
}

/// The largest icon decoded for the launcher.
const ICON_FILE_MAX: usize = 256 * 1024;

/// What the launcher draws for an app.
enum AppIcon<'a> {
    Builtin(crate::icons::Icon),
    File(&'a crate::image::Image),
}

impl AppIcon<'_> {
    fn size(&self) -> (i32, i32) {
        match self {
            AppIcon::Builtin(i) => (i.width(), i.height()),
            AppIcon::File(i) => (i.width as i32, i.height as i32),
        }
    }
    fn draw(&self, c: &mut Canvas, x: i32, y: i32, tint: crate::gfx::Rgb) {
        match self {
            AppIcon::Builtin(i) => c.icon(*i, x, y, tint),
            AppIcon::File(i) => c.rgba(x, y, i.width as i32, i.height as i32, &i.pixels),
        }
    }
}

fn visible_apps<'a>(cx: &'a Cx) -> Vec<&'a opendeskphone_state::AppInfo> {
    cx.st.apps.installed.iter().filter(|a| !a.hidden).collect()
}

/// One of the phone's icons: the manifest's, the app icon of the app's
/// name, or the generic one.
fn builtin_app_icon(th: &Theme, app: &opendeskphone_state::AppInfo) -> Option<crate::icons::Icon> {
    th.icon(&app.icon)
        .or_else(|| th.icon(&format!("app-{}", app.name)))
        .or_else(|| th.icon("app-app"))
}

impl Launcher {
    /// The icon for an app: its own (reduced to the size of the phone's
    /// own app icons), else one of the phone's.
    fn app_icon(&mut self, th: &Theme, app: &opendeskphone_state::AppInfo) -> Option<AppIcon<'_>> {
        if !app.icon_png.is_empty() {
            let (w, h) = th.icon("app-app").map(|i| (i.width() as u16, i.height() as u16)).unwrap_or((48, 48));
            // Decoded once, and again when the app is replaced by one with
            // another icon.
            if self.files.get(&app.name).is_none_or(|(of, _)| *of != app.icon_png) {
                let image = (app.icon_png.0.len() <= ICON_FILE_MAX).then(|| crate::image::decode_png(&app.icon_png.0));
                let image = match image {
                    Some(Ok(i)) => Some(i.fit(w, h)),
                    Some(Err(e)) => {
                        log::warn!("ui: the icon of the app {} is unusable: {e}", app.name);
                        None
                    }
                    None => None,
                };
                self.files.insert(app.name.clone(), (app.icon_png.clone(), image));
            }
            if self.files.get(&app.name).is_some_and(|(_, i)| i.is_some()) {
                return self.files.get(&app.name).and_then(|(_, i)| i.as_ref()).map(AppIcon::File);
            }
        }
        builtin_app_icon(th, app).map(AppIcon::Builtin)
    }
}

impl Launcher {
    fn columns(th: &Theme) -> usize {
        if th.gray { 1 } else if th.touch() { 3 } else { 4 }
    }
    fn launch(&self, cx: &Cx) -> Nav {
        if let Some(a) = visible_apps(cx).get(self.st.selected) {
            cx.post(Command::LaunchApp { name: a.name.clone() });
        }
        Nav::Stay
    }

    /// Touch: the height of a cell of the grid: the icon, with its name
    /// under it.
    fn touch_cell_h(th: &Theme) -> i32 {
        let icon = th.icon("app-app").map(|i| i.height()).unwrap_or(th.touch_min);
        th.pad * 2 + icon + th.pad + th.line_h + th.pad
    }

    /// Touch: the apps in a grid of large icons, each cell a region
    /// (`Hit::Custom` of the app's index), scrolled by a finger.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, body: IRect) {
        let th = cx.th;
        let apps = visible_apps(cx);
        let cols = Self::columns(th);
        let cell_w = body.w / cols as i32;
        let cell_h = Self::touch_cell_h(th);
        let icon_h = cell_h - th.pad * 4 - th.line_h;
        let visible_rows = (body.h / cell_h).max(1) as usize;
        let total_rows = apps.len().div_ceil(cols);
        self.st.top = self.st.top.min(total_rows.saturating_sub(visible_rows));
        let f = th.font(Font::Normal);
        let old = c.push_clip(body);
        for (i, app) in apps.iter().enumerate().skip(self.st.top * cols).take(visible_rows * cols) {
            let (col, row) = ((i % cols) as i32, (i / cols - self.st.top) as i32);
            let cell = IRect::new(body.x + col * cell_w, body.y + row * cell_h, cell_w, cell_h);
            if let Some(icon) = self.app_icon(th, app) {
                let (iw, ih) = icon.size();
                icon.draw(c, cell.x + (cell.w - iw) / 2, cell.y + th.pad * 2 + (icon_h - ih).max(0) / 2, th.fg);
            }
            let label = IRect::new(cell.x + th.pad, cell.y + th.pad * 3 + icon_h, cell.w - th.pad * 2, th.line_h);
            c.text_in(f, label, &app.display_name, th.fg, HAlign::Center);
            c.hit(cell, Hit::Custom(i as u32));
        }
        c.restore_clip(old);
        widgets::scroll_bar(c, th, body, total_rows as i32, visible_rows as i32, self.st.top as i32);
    }
}

impl Screen for Launcher {
    fn name(&self) -> String {
        "apps".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: an app is touched, and the title bar has the way back.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::SELECT), soft(0, ""), soft(0, ""), soft(4, tr::BACK)]
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Custom(i) => {
                self.st.selected = i as usize;
                self.launch(cx)
            }
            _ => Nav::Ignored,
        }
    }
    fn drag(&mut self, cx: &Cx, _from: Option<Hit>, _dx: i32, dy: i32) -> Nav {
        let rows = visible_apps(cx).len().div_ceil(Self::columns(cx.th));
        self.st.drag(dy, Self::touch_cell_h(cx.th), rows);
        Nav::Stay
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        if id == 1 { self.launch(cx) } else { Nav::Pop }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let len = visible_apps(cx).len();
        let cols = Self::columns(cx.th) as i32;
        match key {
            Key::Up => {
                self.st.step(-cols, len, false);
            }
            Key::Down => {
                self.st.step(cols, len, false);
            }
            Key::Left if cols > 1 => {
                self.st.step(-1, len, true);
            }
            Key::Right if cols > 1 => {
                self.st.step(1, len, true);
            }
            Key::Select | Key::Right => return self.launch(cx),
            Key::Cancel | Key::Left => return Nav::Pop,
            Key::Digit(d) => {
                let idx = if d == 0 { 9 } else { d as usize - 1 };
                if idx < len {
                    self.st.selected = idx;
                    return self.launch(cx);
                }
            }
            _ => return Nav::Ignored,
        }
        Nav::Stay
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, if th.touch() { tr::APPS } else { tr::APPLICATIONS });
        let apps = visible_apps(cx);
        if apps.is_empty() {
            if th.touch() {
                empty_state(c, th, body, tr::NO_APPS_TOUCH, tr::NO_APPS_HINT);
            } else {
                c.text_in(th.font(Font::Normal), body, tr::NO_APPS, th.secondary, HAlign::Center);
            }
            return;
        }
        if th.touch() {
            return self.draw_touch(cx, c, body);
        }
        if th.gray {
            // Rows with a small icon, as the gray launcher has no room for
            // a grid.
            let row_h = (th.row_h).max(24 + 2).min(body.h / 3);
            let f = th.font(Font::Normal);
            let mut st = self.st;
            widgets::list(c, th, widgets::body_rows(th, body), apps.len(), &mut st, row_h, true, |c, r, i, sel, fg| {
                let (ic, rest) = r.split_left(row_h + 6);
                if let Some(icon) = self.app_icon(th, apps[i]) {
                    let (iw, ih) = icon.size();
                    // The gray app icons are opaque four-level bitmaps.
                    if sel {
                        c.fill(IRect::new(ic.x + 2, ic.y + 1, iw + 2, r.h - 2), th.bg);
                    }
                    icon.draw(c, ic.x + 3, ic.y + (r.h - ih) / 2, fg);
                }
                c.text_in(f, IRect { x: rest.x + 4, w: rest.w - 4, ..rest }, &apps[i].display_name, fg, HAlign::Left);
            });
            self.st = st;
            return;
        }
        let cols = Self::columns(th);
        let cell_w = body.w / cols as i32;
        let cell_h = 48 + th.line_h + 14;
        let visible_rows = (body.h / cell_h).max(1) as usize;
        self.st.selected = self.st.selected.min(apps.len() - 1);
        let row = self.st.selected / cols;
        if row < self.st.top {
            self.st.top = row;
        }
        if row >= self.st.top + visible_rows {
            self.st.top = row + 1 - visible_rows;
        }
        let f = th.font(Font::Small);
        let old = c.push_clip(body);
        for (i, app) in apps.iter().enumerate().skip(self.st.top * cols).take(visible_rows * cols) {
            let (cx_, cy) = ((i % cols) as i32, (i / cols - self.st.top) as i32);
            let cell = IRect::new(body.x + cx_ * cell_w, body.y + cy * cell_h + 2, cell_w, cell_h - 2);
            let sel = i == self.st.selected;
            if sel {
                c.fill_round(cell.inset_xy(4, 1), 8, th.sel_bg);
            }
            if let Some(icon) = self.app_icon(th, app) {
                let (iw, ih) = icon.size();
                // A smaller picture sits in the middle of the icon's box.
                icon.draw(c, cell.x + (cell.w - iw) / 2, cell.y + 6 + (48 - ih).max(0) / 2, th.fg);
            }
            c.text_in(f, IRect::new(cell.x + 6, cell.y + 56, cell.w - 12, th.line_h - 4), &app.display_name, if sel { th.sel_fg } else { th.fg }, HAlign::Center);
        }
        c.restore_clip(old);
        let total_rows = apps.len().div_ceil(cols);
        widgets::scroll_bar(c, th, body, total_rows as i32, visible_rows as i32, self.st.top as i32);
    }
}

// --------------------------------------------------------------- forward

/// The forward rules, in the order shown.
const FORWARD_RULES: [(ForwardKind, &str); 3] =
    [(ForwardKind::Always, tr::FWD_ALWAYS), (ForwardKind::Busy, tr::FWD_BUSY), (ForwardKind::NoAnswer, tr::FWD_NO_ANSWER)];

/// The number a forward rule forwards to now (empty: off).
fn forward_target(f: &opendeskphone_state::Forward, kind: ForwardKind) -> &str {
    match kind {
        ForwardKind::Always => &f.always,
        ForwardKind::Busy => &f.busy,
        ForwardKind::NoAnswer => &f.no_answer,
    }
}

/// Call forwarding.  On the phones with keys, a menu of the three rules,
/// each with its number or Off (Select types the number, and an empty
/// number turns the rule off), and Do Not Disturb.
///
/// Touch: each rule is a switch, and under a rule that has a number is a
/// row that changes it.  The screen keeps the number of a rule switched
/// off, and switching the rule on again forwards to it; switching on a
/// rule with no number asks for one first.  (Do Not Disturb is in the
/// menu, and on the home screen.)
pub(crate) fn forward_menu() -> Box<dyn Screen> {
    Box::new(ForwardScreen { keys: keyed_forward_menu(), numbers: Default::default(), st: ListState::default() })
}

struct ForwardScreen {
    /// The menu of the phones with keys.
    keys: Box<dyn Screen>,
    /// Touch: the last number of each rule, in the order of
    /// [`FORWARD_RULES`], kept while the rule is off; shared with the
    /// entry of a number.
    numbers: std::sync::Arc<std::sync::Mutex<[String; 3]>>,
    st: ListState,
}

/// Touch: a row of [`ForwardScreen`]: rule `i`'s switch, or the row under
/// it that changes its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForwardRow {
    Rule(usize),
    Number(usize),
}

impl ForwardScreen {
    /// Touch: the numbers kept, with the ones forwarded to now.
    fn numbers(&self, cx: &Cx) -> [String; 3] {
        let mut kept = self.numbers.lock().unwrap_or_else(|p| p.into_inner());
        for (i, (kind, _)) in FORWARD_RULES.iter().enumerate() {
            let now = forward_target(&cx.st.features.forward, *kind);
            if !now.is_empty() {
                kept[i] = now.to_string();
            }
        }
        kept.clone()
    }

    fn rows(&self, cx: &Cx) -> Vec<ForwardRow> {
        let numbers = self.numbers(cx);
        let mut v = Vec::new();
        for (i, number) in numbers.iter().enumerate() {
            v.push(ForwardRow::Rule(i));
            if !number.is_empty() {
                v.push(ForwardRow::Number(i));
            }
        }
        v
    }

    /// Touch: the entry of rule `i`'s number.  The number typed is kept,
    /// and forwarded to if the rule is on, or `switching_on`; an empty
    /// one turns the rule off.
    fn entry(&self, i: usize, switching_on: bool) -> Box<dyn Screen> {
        let (kind, label) = FORWARD_RULES[i];
        let numbers = self.numbers.clone();
        let current = numbers.lock().unwrap_or_else(|p| p.into_inner())[i].clone();
        EditScreen::boxed("forward_edit", format!("{}: {}", tr::FWD_NUMBER, label), &current, InputMode::Dial, move |cx, text| {
            let text = text.trim().to_string();
            numbers.lock().unwrap_or_else(|p| p.into_inner())[i] = text.clone();
            if switching_on || !forward_target(&cx.st.features.forward, kind).is_empty() {
                cx.post(Command::SetForward { kind, target: text, no_answer_secs: None });
            }
            Nav::Pop
        })
    }

    /// Touch: rule `i` switched the other way.
    fn toggle(&self, cx: &Cx, i: usize) -> Nav {
        let kind = FORWARD_RULES[i].0;
        let number = self.numbers(cx)[i].clone();
        if !forward_target(&cx.st.features.forward, kind).is_empty() {
            cx.post(Command::SetForward { kind, target: String::new(), no_answer_secs: None });
        } else if !number.is_empty() {
            cx.post(Command::SetForward { kind, target: number, no_answer_secs: None });
        } else {
            return Nav::Push(self.entry(i, true));
        }
        Nav::Stay
    }
}

impl Screen for ForwardScreen {
    fn name(&self) -> String {
        "forward".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        if cx.th.touch() { Vec::new() } else { self.keys.softkeys(cx) }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        self.keys.softkey(cx, id)
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        if !cx.th.touch() {
            return self.keys.key(cx, key);
        }
        if key == Key::Cancel { Nav::Pop } else { Nav::Ignored }
    }
    fn list_state(&mut self, cx: &Cx) -> Option<(&mut ListState, usize)> {
        if !cx.th.touch() {
            return None;
        }
        let len = self.rows(cx).len();
        Some((&mut self.st, len))
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, x: i32, y: i32) -> Nav {
        if !cx.th.touch() {
            return self.keys.tap(cx, hit, x, y);
        }
        let Hit::Row { row, .. } = hit else { return Nav::Ignored };
        match self.rows(cx).get(row as usize) {
            Some(ForwardRow::Rule(i)) => self.toggle(cx, *i),
            Some(ForwardRow::Number(i)) => Nav::Push(self.entry(*i, false)),
            None => Nav::Stay,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        if !th.touch() {
            return self.keys.draw(cx, c, area);
        }
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, tr::CALL_FORWARD);
        let rows = self.rows(cx);
        let numbers = self.numbers(cx);
        let f = th.font(Font::Normal);
        let fwd = &cx.st.features.forward;
        widgets::list(c, th, widgets::body_rows(th, body), rows.len(), &mut self.st, th.row_h, false, |c, r, i, _, fg| {
            let r = IRect { x: r.x + th.title_pad, w: r.w - th.title_pad - th.pad, ..r };
            match rows[i] {
                ForwardRow::Rule(k) => {
                    let (kind, label) = FORWARD_RULES[k];
                    // (Where a menu's switch is.)
                    let at = widgets::switch(c, th, IRect { w: r.w - th.pad, ..r }, !forward_target(fwd, kind).is_empty());
                    c.text_in(f, IRect { w: at.x - th.pad - r.x, ..r }, label, fg, HAlign::Left);
                }
                // Under its rule, set in.
                ForwardRow::Number(k) => {
                    let r = IRect { x: r.x + th.title_pad, w: r.w - th.title_pad, ..r };
                    widgets::key_value(c, th, r, tr::FWD_CHANGE_NUMBER, &numbers[k], fg, th.secondary);
                }
            }
        });
    }
}

fn keyed_forward_menu() -> Box<dyn Screen> {
    fn rule(label: &'static str, kind: ForwardKind, current: &str) -> Item {
        let cur = current.to_string();
        Item::new(label, move |_| {
            Nav::Push(EditScreen::boxed("forward_edit", format!("{}: {}", tr::FWD_NUMBER, label), &cur, InputMode::Dial, move |cx, text| {
                cx.post(Command::SetForward { kind, target: text.trim().to_string(), no_answer_secs: None });
                Nav::Pop
            }))
        })
        .value(if current.is_empty() { tr::OFF.to_string() } else { current.to_string() })
    }
    Menu::boxed("forward", tr::CALL_FORWARD, |cx| {
        let f = &cx.st.features.forward;
        vec![
            rule(tr::FWD_ALWAYS, ForwardKind::Always, &f.always),
            rule(tr::FWD_BUSY, ForwardKind::Busy, &f.busy),
            rule(tr::FWD_NO_ANSWER, ForwardKind::NoAnswer, &f.no_answer),
            Item::new(tr::DO_NOT_DISTURB, |cx| {
                cx.post(Command::SetDnd { on: !cx.st.features.dnd });
                Nav::Stay
            })
            .value(if cx.st.features.dnd { tr::ON } else { tr::OFF }),
        ]
    })
}

// ---------------------------------------------------------------- paging

/// Multicast paging: the page in progress (select it to stop it), the
/// groups this phone can page (select one to page it), and any SIP paging
/// calls.
///
/// Touch: with none of those, a sentence says so and where groups come
/// from.
pub(crate) fn paging() -> Box<dyn Screen> {
    Box::new(PagingScreen { menu: paging_menu() })
}

struct PagingScreen {
    menu: Box<dyn Screen>,
}

impl Screen for PagingScreen {
    fn name(&self) -> String {
        self.menu.name()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        self.menu.softkeys(cx)
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        self.menu.softkey(cx, id)
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        self.menu.key(cx, key)
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, x: i32, y: i32) -> Nav {
        self.menu.tap(cx, hit, x, y)
    }
    fn drag(&mut self, cx: &Cx, from: Option<Hit>, dx: i32, dy: i32) -> Nav {
        self.menu.drag(cx, from, dx, dy)
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        self.menu.draw(cx, c, area);
        let p = &cx.st.paging;
        let idle = p.activity == opendeskphone_state::PageActivity::Idle;
        if cx.th.touch() && idle && p.broadcasts.is_empty() && p.error.is_empty() && !cx.st.calls.live().any(|c| c.paging) {
            let (_, body) = area.split_top(cx.th.title_h);
            empty_state(c, cx.th, body, tr::NO_PAGE_GROUPS, tr::NO_PAGE_GROUPS_HINT);
        }
    }
}

fn paging_menu() -> Box<dyn Screen> {
    use opendeskphone_state::PageActivity;
    Menu::boxed("paging", tr::PAGING_TITLE, |cx| {
        let mut items = Vec::new();
        let paging = &cx.st.paging;
        let current = match &paging.activity {
            PageActivity::Idle => None,
            PageActivity::Receiving { name, .. } => Some((tr::PAGE_RECEIVING, name)),
            PageActivity::Blocked { name, .. } => Some((tr::PAGE_BLOCKED, name)),
            PageActivity::Sending { name, .. } => Some((tr::PAGE_SENDING, name)),
        };
        let busy = current.is_some();
        if let Some((what, name)) = current {
            items.push(
                Item::new(format!("{what} {name}"), |cx| {
                    cx.post(Command::StopPage);
                    Nav::Stay
                })
                .value(tr::PAGE_STOP)
                .icon("page"),
            );
        }
        if !busy {
            for group in &paging.broadcasts {
                let id = group.id.clone();
                items.push(
                    Item::new(group.name.clone(), move |cx| {
                        cx.post(Command::StartPage { id: id.clone() });
                        Nav::Stay
                    })
                    .icon("page"),
                );
            }
        }
        for c in cx.st.calls.live().filter(|c| c.paging) {
            items.push(Item::new(c.remote.display().to_string(), |_| Nav::Stay).value(calls::call_status(c, cx.now_ms).0));
        }
        if !paging.error.is_empty() {
            items.push(Item::new(paging.error.clone(), |_| Nav::Stay));
        }
        // (Touch: the screen says so in a sentence.)
        if items.is_empty() && !cx.th.touch() {
            items.push(Item::new(tr::NO_PAGE_GROUPS, |_| Nav::Stay));
        }
        items
    })
}

// -------------------------------------------------------------- settings

pub(crate) fn settings() -> Box<dyn Screen> {
    Menu::boxed("settings", tr::SETTINGS, |_| {
        vec![
            Item::new(tr::NETWORK, |_| Nav::Push(network_menu())).icon("network"),
            Item::new(tr::ACCOUNTS, |_| Nav::Push(accounts())).icon("contacts"),
            Item::new(tr::DISPLAY, |_| Nav::Push(display_menu())).icon("brightness"),
            Item::new(tr::SOUNDS, |_| Nav::Push(sounds_menu())).icon("ring"),
            Item::new(tr::RESET, |_| Nav::Push(reset_menu())).icon("update"),
        ]
    })
}

fn edit_setting(label: &'static str, id: &'static str, current: String, mode: InputMode) -> Item {
    let cur = current.clone();
    Item::new(label, move |_| {
        Nav::Push(EditScreen::boxed("setting_edit", label, &cur, mode, move |cx, text| {
            set(cx, id, text.trim());
            Nav::Pop
        }))
    })
    .value(current)
}

/// The network settings.  Addressing and VLAN discovery show the settings
/// as they are configured, which a touch changes; the network uses them
/// once it is applied (the last but one item), and until then the
/// addresses are the ones in use.
pub(crate) fn network_menu() -> Box<dyn Screen> {
    Menu::boxed("network", tr::NETWORK, |cx| {
        let n = &cx.st.network;
        let dhcp = cx.cfg.network_dhcp;
        let ip = n.ipv4.address.map(|a| a.to_string()).unwrap_or_default();
        let mask = if n.ipv4.address.is_some() { prefix_to_mask(n.ipv4.prefix_len) } else { String::new() };
        let gw = n.ipv4.gateway.map(|a| a.to_string()).unwrap_or_default();
        let dns = |i: usize| n.dns.get(i).map(|a| a.to_string()).unwrap_or_default();
        // Stock's vocabulary; anything but these two is no VLAN.
        let vlan = cx.cfg.vlan_discovery.to_ascii_uppercase();
        let vlan_mode = match vlan.as_str() {
            "MANUAL" => tr::VLAN_MANUAL,
            "LLDP" => tr::VLAN_LLDP,
            _ => tr::VLAN_NONE,
        };
        let mut v = vec![Item::new(tr::ADDRESSING, move |cx| {
            set(cx, "network_enable_dhcp", if dhcp { "0" } else { "1" });
            Nav::Stay
        })
        .value(if dhcp { tr::DHCP } else { tr::STATIC })];
        v.push(edit_setting(tr::IP_ADDRESS, "network_static_ip_address", ip, InputMode::Numeric));
        v.push(edit_setting(tr::SUBNET_MASK, "network_subnet_mask", mask, InputMode::Numeric));
        v.push(edit_setting(tr::GATEWAY, "network_default_gateway", gw, InputMode::Numeric));
        v.push(edit_setting(tr::DNS1, "network_primary_dns_server", dns(0), InputMode::Numeric));
        v.push(edit_setting(tr::DNS2, "network_secondary_dns_server", dns(1), InputMode::Numeric));
        v.push(
            Item::new(tr::VLAN_MODE, move |cx| {
                // Cycle NONE -> LLDP -> MANUAL (the stock vocabulary).
                let next = match vlan.as_str() {
                    "LLDP" => "MANUAL",
                    "MANUAL" => "NONE",
                    _ => "LLDP",
                };
                set(cx, "network_vlan_discovery_mode", next);
                Nav::Stay
            })
            .value(vlan_mode),
        );
        v.push(edit_setting(
            tr::VLAN_ID,
            "network_vlan_id",
            n.vlan.voice_id.map(|v| v.to_string()).unwrap_or_default(),
            InputMode::Digits,
        ));
        v.push(Item::new(tr::APPLY_NETWORK, |cx| {
            cx.post(Command::ReconfigureNetwork);
            Nav::Pop
        }));
        v.push(Item::new(if cx.th.touch() { tr::NETWORK_STATUS } else { tr::STATUS }, |_| Nav::Push(network_status())));
        v
    })
}

fn prefix_to_mask(prefix: u8) -> String {
    let bits: u32 = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix.min(32) as u32) };
    std::net::Ipv4Addr::from(bits).to_string()
}

fn link_text(l: &opendeskphone_state::PortLink) -> String {
    if l.up {
        format!("{} Mb/s {}", l.speed, if l.full_duplex { "full" } else { "half" })
    } else {
        tr::LINK_DOWN.to_string()
    }
}

pub(crate) fn network_status() -> Box<dyn Screen> {
    Info::boxed("network_status", tr::NETWORK, |cx| {
        let n = &cx.st.network;
        let mut v = vec![
            (tr::LINK.to_string(), link_text(&n.lan)),
            (tr::PC_PORT.to_string(), if n.pc_port_enabled { link_text(&n.pc) } else { tr::DISABLED.to_string() }),
            (
                tr::ADDRESSING.to_string(),
                match n.ipv4.source {
                    AddrSource::Static => tr::STATIC,
                    AddrSource::Dhcp => tr::DHCP,
                    _ => tr::NOT_SET,
                }
                .to_string(),
            ),
            (
                tr::IP_ADDRESS.to_string(),
                n.ipv4.address.map(|a| format!("{a}/{}", n.ipv4.prefix_len)).unwrap_or_else(|| tr::NOT_SET.into()),
            ),
            (tr::GATEWAY.to_string(), n.ipv4.gateway.map(|a| a.to_string()).unwrap_or_else(|| tr::NOT_SET.into())),
        ];
        for d in &n.dns {
            v.push((tr::DNS.to_string(), d.to_string()));
        }
        for a in &n.ipv6 {
            v.push(("IPv6".to_string(), format!("{}/{}", a.address, a.prefix_len)));
        }
        v.push((
            tr::VLAN.to_string(),
            n.vlan.voice_id.map(|v| v.to_string()).unwrap_or_else(|| tr::VLAN_NONE.to_string()),
        ));
        if let Some(l) = &n.lldp {
            v.push(("LLDP".to_string(), format!("{} {}", l.system_name, l.port_id)));
        }
        v.push((tr::MAC.to_string(), cx.st.system.mac_string()));
        if !n.detail.is_empty() {
            v.push((String::new(), n.detail.clone()));
        }
        v
    })
}

pub(crate) fn accounts() -> Box<dyn Screen> {
    Info::boxed("accounts", tr::ACCOUNTS, |cx| {
        let mut v = Vec::new();
        for a in &cx.st.accounts.accounts {
            v.push((if a.label.is_empty() { a.user.clone() } else { a.label.clone() }, String::new()));
            v.push((tr::USER.to_string(), a.user.clone()));
            v.push((tr::SERVER.to_string(), format!("{}:{}", a.server, a.port)));
            v.push((
                tr::STATUS.to_string(),
                match &a.registration {
                    Registration::Registered { .. } => tr::REGISTERED.to_string(),
                    Registration::Registering => tr::REGISTERING.to_string(),
                    Registration::Disabled => tr::DISABLED.to_string(),
                    Registration::Unregistered => tr::NOT_REGISTERED.to_string(),
                    Registration::Failed { code, reason, .. } => format!("{code} {reason}"),
                },
            ));
        }
        if v.is_empty() {
            v.push((String::new(), tr::NO_ACCOUNTS.to_string()));
        }
        v
    })
}

fn display_menu() -> Box<dyn Screen> {
    Menu::boxed("display", tr::DISPLAY, |cx| {
        // The setting and the raw level are related by the stock table.
        let model = cx.model;
        let setting = move |raw: u8| opendeskphone_hal::backlight_setting(model, opendeskphone_hal::Screen::Main, raw);
        let mut v = vec![Item::new(tr::BRIGHTNESS, move |_| {
            Nav::Push(
                Level::boxed(
                    tr::BRIGHTNESS,
                    opendeskphone_hal::BRIGHTNESS_SETTING_MAX,
                    move |cx| setting(cx.st.ui.backlight_level),
                    move |cx, v| {
                        cx.post(Command::SetBacklight { level: opendeskphone_hal::backlight_level(model, opendeskphone_hal::Screen::Main, v) })
                    },
                )
                .with_commit(|cx, v| set(cx, "brightness", v.to_string())),
            )
        })
        .value(format!("{}", setting(cx.st.ui.backlight_level)))];
        if cx.th.gray {
            let cmax = cx.cfg.contrast_max.max(1);
            v.push(
                Item::new(tr::CONTRAST, move |_| {
                    Nav::Push(
                        Level::boxed(tr::CONTRAST, cmax, |cx| cx.st.ui.contrast, |cx, v| cx.post(Command::SetContrast { level: v }))
                            .with_commit(|cx, v| set(cx, "contrast", v.to_string())),
                    )
                })
                .value(cx.st.ui.contrast.to_string()),
            );
        }
        let dim = cx.cfg.dim_backlight;
        v.push(
            Item::new(tr::DIM_BACKLIGHT, move |cx| {
                set(cx, "dim_backlight", if dim { "0" } else { "1" });
                Nav::Stay
            })
            .value(if dim { tr::ON } else { tr::OFF })
            .switch(dim),
        );
        // Touch: a choice of a few times; the other skins type the seconds.
        let timeout = cx.cfg.backlight_timeout_secs;
        if cx.th.touch() {
            v.push(Item::new(tr::BACKLIGHT_TIMEOUT, |_| Nav::Push(backlight_timeouts())).value(timeout_text(timeout)));
        } else {
            v.push(edit_setting(tr::BACKLIGHT_TIMEOUT, "backlight_timeout", timeout.to_string(), InputMode::Digits));
        }
        // Gray screens have no themes.
        if !cx.th.gray {
            v.push(
                Item::new(tr::THEME, |cx| Nav::Push(themes(cx.cfg)))
                    .value(crate::wallpaper::find_or_default(&cx.cfg.theme).name),
            );
        }
        v
    })
}

/// Touch: the backlight timeouts to choose from, in seconds (0: the
/// backlight does not dim).
const BACKLIGHT_TIMEOUTS: [u32; 5] = [15, 30, 60, 300, 0];

/// Touch: a backlight timeout in words: `15 s`, `1 min`, `Never`.
fn timeout_text(secs: u32) -> String {
    match secs {
        0 => tr::NEVER.into(),
        s if s % 60 == 0 => format!("{} {}", s / 60, tr::MINUTES_SHORT),
        s => format!("{s} {}", tr::SECONDS_SHORT),
    }
}

/// Touch: the choice of the backlight timeout.
fn backlight_timeouts() -> Box<dyn Screen> {
    Box::new(Choices {
        name: "backlight_timeout",
        title: tr::BACKLIGHT_TIMEOUT,
        choices: BACKLIGHT_TIMEOUTS.iter().map(|s| timeout_text(*s)).collect(),
        current: Box::new(|cx| BACKLIGHT_TIMEOUTS.iter().position(|s| *s == cx.cfg.backlight_timeout_secs)),
        choose: Box::new(|cx, i| set(cx, "backlight_timeout", BACKLIGHT_TIMEOUTS[i].to_string())),
        chosen: None,
        st: ListState::default(),
    })
}

/// Touch: a setting with a few values: a list of them, the one in effect
/// with a check mark.  A tap makes one the setting at once; the title
/// bar's back button leaves.
struct Choices {
    name: &'static str,
    title: &'static str,
    choices: Vec<String>,
    /// The choice in effect, if the setting is one of them.
    current: Box<dyn Fn(&Cx) -> Option<usize> + Send>,
    choose: Box<dyn Fn(&Cx, usize) + Send>,
    /// The choice tapped last, checked before the setting catches up.
    chosen: Option<usize>,
    st: ListState,
}

impl Screen for Choices {
    fn name(&self) -> String {
        self.name.into()
    }
    fn list_state(&mut self, _cx: &Cx) -> Option<(&mut ListState, usize)> {
        Some((&mut self.st, self.choices.len()))
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Row { row, .. } if (row as usize) < self.choices.len() => {
                self.chosen = Some(row as usize);
                (self.choose)(cx, row as usize);
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn key(&mut self, _cx: &Cx, key: Key) -> Nav {
        if key == Key::Cancel { Nav::Pop } else { Nav::Ignored }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, self.title);
        let checked = self.chosen.or_else(|| (self.current)(cx));
        let f = th.font(Font::Normal);
        let choices = &self.choices;
        widgets::list(c, th, widgets::body_rows(th, body), choices.len(), &mut self.st, th.row_h, false, |c, r, i, _, fg| {
            let (mark, rest) = r.split_left(th.back_w);
            if Some(i) == checked
                && let Some(icon) = th.icon("ok")
            {
                c.icon_in(icon, mark, th.accent);
            }
            c.text_in(f, rest, &choices[i], fg, HAlign::Left);
        });
    }
}

/// The theme selector: the idle screen itself (its clock, status icons,
/// and line-key tabs, drawn by the idle screen's code) over the wallpaper
/// of the theme the selection is on, in that theme's palette, with one
/// small bubble at the bottom that names the theme between a left and a
/// right arrow.  A wallpaper that moves on the idle screen moves here.
/// Left and Right (and Up and Down) step through the themes, around the
/// ends; Select saves, Back leaves the setting as it was.
///
/// Touch: in place of the bubble, a sheet over the lower part of the
/// screen with a title bar and the themes in rows, the one in use with a
/// check mark.  A tap on a theme makes it the theme, at once, and the
/// screen above the sheet shows it.  (A theme the provisioning server
/// fixed is shown, and not saved.)
///
/// The themes are rows and not a grid of pictures of their wallpapers:
/// there are forty-odd, each wallpaper is drawn from its description at
/// the size it is shown, and the idle screen above shows each one whole,
/// in its colors, which a small picture would not.
struct Themes {
    selected: usize,
    /// Touch: the rows of the sheet.
    st: ListState,
    /// Touch: the sheet is to scroll to the selection when it next draws
    /// (it was opened, or a key stepped).
    follow: bool,
}

/// The theme selector, with the theme in use selected.
pub(crate) fn themes(cfg: &crate::UiConfig) -> Box<dyn Screen> {
    let current = crate::wallpaper::find_or_default(&cfg.theme).id;
    let selected = crate::wallpaper::all().iter().position(|w| w.id == current).unwrap_or(0);
    Box::new(Themes { selected, st: ListState::default(), follow: true })
}

/// Touch: how many themes the selector's sheet shows at once.
const THEME_ROWS: i32 = 4;

/// Where the selector's bubble is: centered under the clock and the date,
/// left of the line-key tabs, just above the softkeys.
pub(crate) fn theme_bubble(th: &Theme) -> IRect {
    let (w, h) = (220, 28);
    IRect::new((th.idle_tab_x - w) / 2, th.soft_y - 6 - h, w, h)
}

impl Themes {
    fn selected(&self) -> &'static crate::wallpaper::Wallpaper {
        let all = crate::wallpaper::all();
        &all[self.selected.min(all.len() - 1)]
    }

    fn step(&mut self, by: usize) {
        let len = crate::wallpaper::all().len();
        self.selected = (self.selected + by) % len;
        self.follow = true;
    }

    /// Touch: the idle screen in the upper part of `area`, and the sheet
    /// of themes under it.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let all = crate::wallpaper::all();
        let (preview, sheet) = area.split_bottom(th.title_h + THEME_ROWS * th.row_h);
        let old = c.push_clip(preview);
        idle::Idle.draw(&Cx { idle_view: None, ..*cx }, c, preview);
        c.restore_clip(old);
        // The idle screen is to be looked at only: its lines are not the
        // line keys here.
        c.hit(preview, Hit::Nothing);
        c.fill(sheet, th.bg);
        let (title, rows) = sheet.split_top(th.title_h);
        widgets::title_bar(c, th, title, tr::THEME);
        self.st.selected = self.selected;
        if self.follow {
            // The selection second from the top, with the theme before it
            // above it.
            self.follow = false;
            self.st.top = self.selected.saturating_sub(1).min(all.len().saturating_sub(THEME_ROWS as usize));
        }
        // The theme touched last is the one in use (or, fixed by the
        // server, the one shown); there is no highlight to move.
        let checked = if cx.cfg.theme_locked { crate::wallpaper::find_or_default(&cx.cfg.theme).id } else { self.selected().id };
        let f = th.font(Font::Normal);
        widgets::list(c, th, rows, all.len(), &mut self.st, th.row_h, false, |c, r, i, _, fg| {
            let (mark, rest) = r.split_left(th.back_w);
            if all[i].id == checked
                && let Some(icon) = th.icon("ok")
            {
                c.icon_in(icon, mark, th.accent);
            }
            c.text_in(f, rest, all[i].name, fg, HAlign::Left);
        });
    }
}

impl Screen for Themes {
    fn name(&self) -> String {
        "theme".into()
    }
    fn wallpaper(&self) -> bool {
        true
    }
    fn motion(&self) -> bool {
        true
    }
    fn look(&self, _cx: &Cx) -> Option<&'static crate::wallpaper::Wallpaper> {
        Some(self.selected())
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: a tap saves, and the sheet's title bar has the way back.
        if cx.th.touch() {
            return Vec::new();
        }
        // A theme the provisioning server fixed can be looked at only.
        let select = if cx.cfg.theme_locked { soft(0, "") } else { soft(1, tr::SELECT) };
        vec![select, soft(0, ""), soft(0, ""), soft(4, tr::BACK)]
    }
    fn list_state(&mut self, _cx: &Cx) -> Option<(&mut ListState, usize)> {
        Some((&mut self.st, crate::wallpaper::all().len()))
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            // Shown in the theme, and saved.
            Hit::Row { row, .. } if (row as usize) < crate::wallpaper::all().len() => {
                self.selected = row as usize;
                if !cx.cfg.theme_locked {
                    cx.post(Command::SetTheme { name: self.selected().id.into() });
                }
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 if !cx.cfg.theme_locked => {
                cx.post(Command::SetTheme { name: self.selected().id.into() });
                Nav::Pop
            }
            4 => Nav::Pop,
            _ => Nav::Stay,
        }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let len = crate::wallpaper::all().len();
        match key {
            Key::Left | Key::Up => self.step(len - 1),
            Key::Right | Key::Down => self.step(1),
            Key::Select => return self.softkey(cx, 1),
            Key::Cancel => return Nav::Pop,
            _ => return Nav::Ignored,
        }
        Nav::Stay
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        if th.touch() {
            return self.draw_touch(cx, c, area);
        }
        // The idle screen, by its own code (without an app's idle view,
        // where the bubble is).
        idle::Idle.draw(&Cx { idle_view: None, ..*cx }, c, area);
        // The bubble: a bar with round ends in the softkeys' colors, the
        // name in the middle, an arrow at each end.
        let r = theme_bubble(th);
        c.fill_round_aa(r, r.h / 2, th.sk_edge);
        c.fill_round_aa(r.inset(1), r.h / 2 - 1, th.sk_bg);
        let (arrow_w, arrow_h, inset): (i32, i32, i32) = (7, 13, 14);
        let mid = r.y + r.h / 2;
        for j in 0..arrow_h {
            // Each row of an arrow is as long as it is far from the tip.
            let run = arrow_w - (j - arrow_h / 2).abs() * arrow_w / (arrow_h / 2 + 1);
            let y = mid - arrow_h / 2 + j;
            c.span(y, r.x + inset + arrow_w - run, r.x + inset + arrow_w, th.sk_fg);
            c.span(y, r.right() - inset - arrow_w, r.right() - inset - arrow_w + run, th.sk_fg);
        }
        let text = r.inset_xy(inset + arrow_w + th.pad, 0);
        c.text_in(th.font(Font::Bold), text, self.selected().name, th.sk_fg, HAlign::Center);
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        // The clock goes on, as on the idle screen.
        idle::Idle.deadline(cx)
    }
}

fn sounds_menu() -> Box<dyn Screen> {
    Menu::boxed("sounds", tr::SOUNDS, |cx| {
        vec![
            Item::new(tr::RINGTONE, |_| Nav::Push(ringtones())).value(cx.cfg.ringtone.clone()),
            Item::new(tr::RINGER_VOLUME, |_| {
                Nav::Push(Level::boxed(
                    tr::RINGER_VOLUME,
                    opendeskphone_state::VOLUME_MAX,
                    |cx| cx.st.audio.ringer_volume,
                    |cx, v| cx.post(Command::SetVolume { target: VolumeTarget::Ringer, level: v }),
                ))
            })
            .value(cx.st.audio.ringer_volume.to_string()),
        ]
    })
}

/// The ringtones, the one in use with a check mark.  Play sounds the
/// selected one; Select makes it the ringtone.
///
/// Touch: a tap on a ringtone sounds it and makes it the ringtone, and
/// the check mark moves to it.
struct Ringtones {
    st: ListState,
    /// Touch: the ringtone tapped last, checked before the setting
    /// catches up.
    picked: Option<usize>,
}

fn ringtones() -> Box<dyn Screen> {
    Box::new(Ringtones { st: ListState::default(), picked: None })
}

impl Screen for Ringtones {
    fn list_state(&mut self, cx: &Cx) -> Option<(&mut ListState, usize)> {
        Some((&mut self.st, cx.cfg.ringtones.len()))
    }
    fn name(&self) -> String {
        "ringtones".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: a tap plays and chooses, and the title bar has the way
        // back.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::SELECT), soft(2, tr::PLAY), soft(0, ""), soft(4, tr::BACK)]
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Row { row, .. } if (row as usize) < cx.cfg.ringtones.len() => {
                let name = cx.cfg.ringtones[row as usize].clone();
                self.picked = Some(row as usize);
                self.st.selected = row as usize;
                cx.post(Command::PlaySound { name: name.clone() });
                set(cx, "default_ringtone", name);
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        let name = cx.cfg.ringtones.get(self.st.selected).cloned();
        match (id, name) {
            (1, Some(name)) => {
                cx.post(Command::StopSound);
                set(cx, "default_ringtone", name);
                Nav::Pop
            }
            (2, Some(name)) => {
                cx.post(Command::PlaySound { name });
                Nav::Stay
            }
            (4, _) => Nav::Pop,
            _ => Nav::Stay,
        }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let len = cx.cfg.ringtones.len();
        match key {
            Key::Up => {
                self.st.step(-1, len, true);
                Nav::Stay
            }
            Key::Down => {
                self.st.step(1, len, true);
                Nav::Stay
            }
            Key::Select => self.softkey(cx, 1),
            Key::Right => self.softkey(cx, 2),
            Key::Cancel | Key::Left => Nav::Pop,
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, tr::RINGTONE);
        let f = th.font(Font::Normal);
        let tones = &cx.cfg.ringtones;
        // Touch: no highlight (there is no cursor), and the check mark on
        // the ringtone tapped last.
        let touch = th.touch();
        let mark_w = if touch { th.back_w } else { th.icon + 2 * th.pad };
        let checked = |i: usize| match self.picked {
            Some(p) if touch => p == i,
            _ => tones[i] == cx.cfg.ringtone,
        };
        widgets::list(c, th, widgets::body_rows(th, body), tones.len(), &mut self.st, th.row_h, !touch, |c, r, i, _, fg| {
            let (mark, rest) = r.split_left(mark_w);
            if checked(i)
                && let Some(icon) = th.icon("ok") {
                    c.icon_in(icon, mark, if touch { th.accent } else { fg });
                }
            c.text_in(f, rest, &tones[i], fg, HAlign::Left);
        });
    }
    fn leave(&mut self, cx: &Cx) {
        cx.post(Command::StopSound);
    }
}

/// The question asked before the phone restarts.
pub(crate) fn restart_ask() -> Ask {
    Ask {
        question: tr::RESTART_Q.into(),
        title: tr::RESTART_PHONE,
        sentence: tr::RESTART_SENTENCE.into(),
        action: tr::RESTART,
        destructive: false,
    }
}

/// The reset menu.  Touch: factory reset last, set apart, in red.
fn reset_menu() -> Box<dyn Screen> {
    Menu::boxed("reset", tr::RESET, |cx| {
        let mut v = vec![Item::new(tr::RESTART_PHONE, |cx| {
            Nav::Push(ask(cx, restart_ask(), |cx| {
                cx.post(Command::Reboot { reason: "menu".into() });
                Nav::Home
            }))
        })];
        v.push(Item::new(tr::REPROVISION, |cx| {
            cx.post(Command::Reprovision);
            Nav::Home
        }));
        v.push(Item::new(tr::FORGET_SERVER, |cx| {
            let a = Ask {
                question: tr::FORGET_SERVER_Q.into(),
                title: tr::FORGET_SERVER,
                sentence: tr::FORGET_SERVER_SENTENCE.into(),
                action: tr::FORGET_SERVER,
                destructive: true,
            };
            Nav::Push(ask(cx, a, |cx| {
                cx.post(Command::ForgetProvisioning);
                Nav::Home
            }))
        }));
        let factory_reset = Item::new(tr::FACTORY_RESET, |cx| {
            let a = Ask {
                question: tr::FACTORY_RESET_Q.into(),
                title: tr::FACTORY_RESET,
                sentence: tr::FACTORY_RESET_SENTENCE.into(),
                action: tr::ERASE_AND_RESTART,
                destructive: true,
            };
            Nav::Push(ask(cx, a, |cx| {
                cx.post(Command::FactoryReset);
                Nav::Home
            }))
        })
        .destructive();
        let mut factory_reset = Some(factory_reset);
        if !cx.th.touch() {
            v.extend(factory_reset.take());
        }
        if matches!(cx.st.update.trial, TrialBoot::Pending { .. }) {
            v.push(Item::new(tr::ROLLBACK, |cx| {
                let a = Ask {
                    question: format!("{}?", tr::ROLLBACK),
                    title: tr::ROLLBACK_TITLE,
                    sentence: tr::ROLLBACK_SENTENCE.into(),
                    action: tr::ROLLBACK,
                    destructive: false,
                };
                Nav::Push(ask(cx, a, |cx| {
                    cx.post(Command::Rollback);
                    Nav::Home
                }))
            }));
        }
        if let Some(factory_reset) = factory_reset {
            // Touch: a row of nothing between the rest and it.
            v.push(Item::new("", |_| Nav::Stay));
            v.push(factory_reset);
        }
        v
    })
}

// ---------------------------------------------------------------- status

fn status_menu(touch: bool) -> Box<dyn Screen> {
    Menu::boxed("status", if touch { tr::PHONE_STATUS } else { tr::STATUS }, |_| {
        vec![
            Item::new(tr::ABOUT, |_| Nav::Push(about())).icon("info"),
            Item::new(tr::NETWORK, |_| Nav::Push(network_status())).icon("network"),
            Item::new(tr::ACCOUNTS, |_| Nav::Push(accounts())).icon("contacts"),
            Item::new(tr::HEALTH, |_| Nav::Push(diag::health())).icon("ok"),
        ]
    })
}

pub(crate) fn about() -> Box<dyn Screen> {
    Info::boxed("about", tr::ABOUT, |cx| {
        let s = &cx.st.system;
        let p = &cx.st.provisioning;
        let mut v = vec![
            (tr::PRODUCT.to_string(), String::new()),
            (tr::MODEL.to_string(), s.model.name().to_string()),
            (tr::VERSION.to_string(), s.version.to_string()),
            (
                tr::IP_ADDRESS.to_string(),
                cx.st.network.ipv4.address.map(|a| a.to_string()).unwrap_or_else(|| tr::NOT_SET.into()),
            ),
            (tr::MAC.to_string(), s.mac_string()),
        ];
        if !s.hostname.is_empty() {
            v.push((tr::HOSTNAME.to_string(), s.hostname.clone()));
        }
        if let Some(b) = cx.st.update.running_bank {
            let trial = match &cx.st.update.trial {
                TrialBoot::Pending { .. } => format!(" ({})", tr::TRIAL_PENDING),
                TrialBoot::RolledBack { .. } => format!(" ({})", tr::ROLLED_BACK),
                TrialBoot::None => String::new(),
            };
            v.push((tr::BANK.to_string(), format!("{}{}", b.index(), trial)));
        }
        if let Some(srv) = &p.server {
            v.push((tr::PROVISIONING.to_string(), srv.address.clone()));
        }
        v.push((
            tr::DSP.to_string(),
            match &cx.st.audio.dsp {
                DspStatus::NotStarted => "not started".to_string(),
                DspStatus::Starting => "starting".to_string(),
                DspStatus::Running => "running".to_string(),
                DspStatus::Failed(e) => format!("failed: {e}"),
            },
        ));
        v.push((tr::UPTIME.to_string(), crate::time::duration(cx.now_ms / 1000)));
        if s.mem_total_kib > 0 {
            v.push((tr::MEMORY.to_string(), format!("{} MiB", s.mem_total_kib / 1024)));
        }
        if !s.last_reboot_reason.is_empty() {
            v.push((tr::LAST_REBOOT.to_string(), s.last_reboot_reason.clone()));
        }
        v
    })
}

// --------------------------------------------------------- touch layout

/// Touch: the places of `n` buttons as wide as the screen, one above the
/// other at the bottom of `area` (the last at the bottom), and what is
/// left of `area` above them.
pub(crate) fn bottom_buttons(th: &Theme, area: IRect, n: usize) -> (IRect, Vec<IRect>) {
    // (At the sides, where the home and call screens' buttons are.)
    let (side, below, gap) = (dialpad::MARGIN, th.px(6), th.pad);
    let n = n as i32;
    let band = n * th.row_h + (n - 1).max(0) * gap + below + gap;
    let (rest, band) = area.split_bottom(band);
    let rects = (0..n).map(|i| IRect::new(band.x + side, band.y + gap + i * (th.row_h + gap), band.w - 2 * side, th.row_h)).collect();
    (rest, rects)
}

/// Touch: `r` (one of [`bottom_buttons`]) as two buttons side by side.
pub(crate) fn halves(th: &Theme, r: IRect) -> (IRect, IRect) {
    let gap = th.pad;
    let (left, right) = r.split_left((r.w - gap) / 2);
    (left, IRect { x: right.x + gap, w: right.w - gap, ..right })
}

/// Touch: a list with nothing in it, said in `r`: what is not there, and
/// under it, dimmer, a sentence on what to do about it.  In the upper
/// third, where the eye is.
pub(crate) fn empty_state(c: &mut Canvas, th: &Theme, r: IRect, title: &str, hint: &str) {
    let f = th.font(Font::Normal);
    let inner = r.inset_xy(th.pad * 3, 0);
    let hint_lines = if hint.is_empty() { 0 } else { f.wrap(hint, inner.w).len() as i32 };
    let block = th.line_h + th.pad + hint_lines * th.line_h;
    let y = r.y + (r.h / 3 - block / 2).max(th.pad * 2);
    c.text_in(th.font(Font::Bold), IRect::new(inner.x, y, inner.w, th.line_h), title, th.fg, HAlign::Center);
    let hint_r = IRect::new(inner.x, y + th.line_h + th.pad, inner.w, hint_lines * th.line_h);
    c.text_wrapped(f, hint_r, hint, th.secondary, HAlign::Center, th.line_h);
}

/// A question asked before an action: what [`Confirm`] shows.  Each
/// caller's words are in one of these, and [`ask`] is the one place that
/// gives them to `Confirm`.
pub(crate) struct Ask {
    /// The question as the phones with keys ask it.
    pub question: String,
    /// Touch: the dialog's title.
    pub title: &'static str,
    /// Touch: what the action does, in a sentence.
    pub sentence: String,
    /// Touch: the verb on the button that does it.
    pub action: &'static str,
    /// Touch: the action erases or forgets something (a red button).
    pub destructive: bool,
}

/// The screen that asks `a`, and does `yes` when the answer is yes.
pub(crate) fn ask(cx: &Cx, a: Ask, yes: impl Fn(&Cx) -> Nav + Send + 'static) -> Box<dyn Screen> {
    // Touch: a card that says what happens, with the verb on its button.
    if !cx.th.touch() {
        return Confirm::boxed(a.question, yes);
    }
    let confirm = Confirm::boxed(a.sentence, yes).title(a.title).action(a.action);
    if a.destructive { confirm.destructive() } else { confirm }
}
