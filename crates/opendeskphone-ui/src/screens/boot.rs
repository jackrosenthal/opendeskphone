//! Base screens that replace the idle screen: boot progress, recovery,
//! provisioning, and firmware update.
//!
//! On the touch skin (D80) each is laid out for the upright screen: the
//! start-up artwork is drawn for it (`assets/art/splash.touch.svg`), a
//! message is a block of an icon, a title, the text, and a bar in the
//! middle of the screen ([`notice`]), and a title has no back button
//! ([`title_band`]: a base screen has nothing to go back to).  What the
//! keys do is done by touch: the softkeys' actions are buttons across the
//! bottom, in words a first-time user knows, with the rarer ones under
//! More; a server or a user is touched in its list, under a line that
//! says what to do; and a touch elsewhere stops the count to the
//! automatic choice of a server, as a key does.  The list of users has a
//! field that narrows it as it is typed in.  The server's address and the
//! PIN are typed in an [`EditScreen`], which is the touch skin's own.

use super::*;
use crate::gfx::Rgb;
use opendeskphone_state::{Health, HealthStatus, NetPhase, Phase, ProvisionPhase, Skin, Subsystem, UpdatePhase};

/// The title band of a base screen: [`widgets::title_bar`], without the
/// back button it has on the touch skin.  (Also the ringing alarm's: there
/// it is the alarm's buttons that take it down.)
pub(crate) fn title_band(c: &mut Canvas, th: &Theme, r: IRect, title: &str) {
    if !th.touch() {
        return widgets::title_bar(c, th, r, title);
    }
    c.fill(r, th.title_bg);
    let text = IRect { x: r.x + th.title_pad, w: r.w - 2 * th.title_pad, ..r };
    c.text_in(th.font(Font::Title), text, title, th.title_fg, HAlign::Left);
}

/// A message filling `area` ([`splash`]): on the touch skin one block in
/// the middle of the screen, of the icon called `icon` (tinted `tint`
/// where it takes a tint), the title, the text, smaller under it the
/// `detail` (the other skins show none), and a bar filled `percent`, each
/// where it is given.
#[allow(clippy::too_many_arguments)]
fn notice(c: &mut Canvas, th: &Theme, area: IRect, icon: &str, tint: Rgb, title: &str, text: &str, detail: &str, percent: Option<u32>) {
    if !th.touch() {
        return splash(c, th, area, title, text, percent);
    }
    let inner = area.inset_xy(th.pad * 3, 0);
    let f = th.font(Font::Normal);
    let small = th.font(Font::Small);
    let small_h = small.height() + th.px(4);
    let gap = th.px(12);
    let bar_h = th.px(8);
    let icon = th.icon(icon);
    let lines = if text.is_empty() { 0 } else { f.wrap(text, inner.w).len() as i32 };
    let detail_lines = if detail.is_empty() { 0 } else { small.wrap(detail, inner.w).len() as i32 };
    let block = icon.map_or(0, |i| i.height() + 2 * gap)
        + if title.is_empty() { 0 } else { th.line_h + gap }
        + lines * th.line_h
        + if detail_lines > 0 { gap + detail_lines * small_h } else { 0 }
        + if percent.is_some() { 2 * gap + bar_h } else { 0 };
    let mut y = area.y + ((area.h - block) / 2).max(0);
    if let Some(i) = icon {
        c.icon(i, inner.x + (inner.w - i.width()) / 2, y, tint);
        y += i.height() + 2 * gap;
    }
    if !title.is_empty() {
        c.text_in(th.font(Font::Title), IRect::new(inner.x, y, inner.w, th.line_h), title, th.fg, HAlign::Center);
        y += th.line_h + gap;
    }
    let (body_fg, detail_fg) = if detail.is_empty() { (th.secondary, th.secondary) } else { (th.fg, th.secondary) };
    y += c.text_wrapped(f, IRect::new(inner.x, y, inner.w, lines * th.line_h), text, body_fg, HAlign::Center, th.line_h);
    if detail_lines > 0 {
        y += gap;
        y += c.text_wrapped(small, IRect::new(inner.x, y, inner.w, detail_lines * small_h), detail, detail_fg, HAlign::Center, small_h);
    }
    if let Some(p) = percent {
        let w = area.w * 2 / 3;
        widgets::loading_bar(c, th, IRect::new(area.x + (area.w - w) / 2, y + 2 * gap, w, bar_h), p.min(100) * 10);
    }
}

/// The start-up stages the loading bar follows, in the order the main
/// role starts them: the subsystem whose health reports the stage, its
/// share of the bar in thousandths, and the text shown while it starts.
/// A stage is complete once its subsystem has settled (running, or failed:
/// start-up goes on without it).  The shares follow the times measured on
/// a D60, where the audio engine and the DSP take most of the start; the
/// rest of the bar fills when start-up ends and the phone leaves this
/// screen.
const STAGES: [(Subsystem, u32, &str); 5] = [
    (Subsystem::Config, 100, tr::STARTING_CONFIG),
    (Subsystem::Media, 550, tr::STARTING_AUDIO),
    (Subsystem::Sip, 100, tr::STARTING_TELEPHONY),
    (Subsystem::Apps, 100, tr::STARTING_APPS),
    (Subsystem::Web, 100, tr::STARTING_WEB),
];

/// How far start-up has come, in thousandths, and the text for the stage
/// under way (the first one that is starting).
pub(crate) fn boot_progress(health: &Health) -> (u32, &'static str) {
    let mut done = 0;
    let mut label = None;
    for (subsystem, share, text) in STAGES {
        match health.status(subsystem) {
            HealthStatus::Stopped => {}
            HealthStatus::Starting => {
                label.get_or_insert(text);
            }
            HealthStatus::Ok | HealthStatus::Degraded(_) | HealthStatus::Failed(_) => done += share,
        }
    }
    (done, label.unwrap_or(tr::STARTING))
}

/// Boot and shutdown progress: the product's mark and name (the only
/// screen with them), the loading bar, and what is going on.  No key does
/// anything while the phone starts, so there are no softkey shapes.
#[derive(Default)]
pub(crate) struct Boot {
    /// The furthest the bar has been drawn: it never moves backward.
    shown: u32,
}

impl Screen for Boot {
    fn name(&self) -> String {
        "boot".into()
    }
    fn wallpaper(&self) -> bool {
        true
    }
    /// Always the first wallpaper and the skin's own palette, whatever
    /// the theme is: the artwork is drawn for them.
    fn look(&self, _cx: &Cx) -> Option<&'static crate::wallpaper::Wallpaper> {
        Some(crate::wallpaper::first())
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // (Touch: the screen draws its two buttons itself.)
        if cx.st.system.phase == Phase::NetworkWait && !cx.th.touch() {
            vec![soft(1, tr::NETWORK), soft(0, ""), soft(0, ""), soft(4, tr::STATUS)]
        } else {
            Vec::new()
        }
    }
    fn softkey_bar(&self, cx: &Cx) -> bool {
        !self.softkeys(cx).is_empty()
    }
    fn softkey(&mut self, _cx: &Cx, id: u32) -> Nav {
        match id {
            1 => Nav::Push(menus::network_menu()),
            4 => Nav::Push(menus::about()),
            _ => Nav::Stay,
        }
    }
    fn key(&mut self, _cx: &Cx, _key: Key) -> Nav {
        Nav::Ignored
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        // Touch, waiting for the network: the network's settings and the
        // phone's details, side by side across the bottom.
        let area = if th.touch() && cx.st.system.phase == Phase::NetworkWait {
            let (rest, buttons) = menus::bottom_buttons(th, area, 1);
            let (left, right) = menus::halves(th, buttons[0]);
            widgets::button_styled(c, th, left, tr::NETWORK_SETTINGS, widgets::ButtonStyle::Secondary, Hit::Soft(1));
            widgets::button_styled(c, th, right, tr::ABOUT_PHONE, widgets::ButtonStyle::Secondary, Hit::Soft(4));
            rest
        } else {
            area
        };
        let net = &cx.st.network;
        let (progress, stage) = boot_progress(&cx.st.health);
        self.shown = self.shown.max(progress);
        let text = match cx.st.system.phase {
            Phase::ShuttingDown => tr::SHUTTING_DOWN.to_string(),
            Phase::NetworkWait => match net.phase {
                NetPhase::Failed if !net.detail.is_empty() => format!("{}: {}", tr::NETWORK_FAILED, net.detail),
                NetPhase::Failed => tr::NETWORK_FAILED.to_string(),
                _ if !net.lan.up => tr::NO_LINK.to_string(),
                NetPhase::Configuring => tr::GETTING_ADDRESS.to_string(),
                _ => tr::WAITING_NETWORK.to_string(),
            },
            _ => stage.to_string(),
        };
        // One block, centered: the mark and name, the bar, and the text.
        // (Gap above the bar, the bar's height, gap above the text.)
        let (gap_bar, bar_h, gap_text) = match th.skin {
            Skin::SmallGray => (12, 7, 4),
            Skin::LargeGray => (16, 9, 5),
            Skin::Color => (30, 8, 12),
            Skin::Touch => (60, 16, 24),
        };
        // The color artwork carries the name; on gray the name is text, in
        // black, so that its edges get all four levels.
        let icon = th.icon("splash");
        let name_font = th.font(Font::Large);
        let (icon_w, icon_h) = icon.map_or((0, 0), |i| (i.width(), i.height()));
        let name_gap = icon_w / 4;
        let logo_w = if th.gray { icon_w + name_gap + name_font.width(tr::PRODUCT) } else { icon_w };
        let block_h = icon_h + gap_bar + bar_h + gap_text + th.line_h;
        let mut y = area.y + ((area.h - block_h) / 2).max(0);
        let x = area.x + (area.w - logo_w) / 2;
        if let Some(icon) = icon {
            c.icon(icon, x, y, th.fg);
        }
        if th.gray {
            let r = IRect::new(x + icon_w + name_gap, y, logo_w - icon_w - name_gap, icon_h);
            c.text_in(name_font, r, tr::PRODUCT, th.fg, HAlign::Left);
        }
        y += icon_h + gap_bar;
        // The bar is for starting up, not for shutting down.
        if cx.st.system.phase != Phase::ShuttingDown {
            let w = area.w / 2;
            widgets::loading_bar(c, th, IRect::new(area.x + (area.w - w) / 2, y, w, bar_h), self.shown);
        }
        y += bar_h + gap_text;
        c.text_wrapped(
            th.font(Font::Normal),
            IRect::new(area.x + th.pad, y, area.w - 2 * th.pad, (area.bottom() - y).max(th.line_h)),
            &text,
            if th.gray { th.fg } else { th.secondary },
            HAlign::Center,
            th.line_h,
        );
    }
}

/// nvdata could not be mounted.
pub(crate) struct Recovery;

impl Screen for Recovery {
    fn name(&self) -> String {
        "recovery".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // (Touch: the screen draws its buttons itself.)
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::STATUS), soft(2, tr::NETWORK), soft(0, ""), soft(4, tr::REBOOT)]
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 => Nav::Push(menus::about()),
            2 => Nav::Push(menus::network_status()),
            4 => Nav::Push(menus::ask(cx, menus::restart_ask(), |cx| {
                cx.post(Command::Reboot { reason: "recovery screen".into() });
                Nav::Pop
            })),
            _ => Nav::Stay,
        }
    }
    fn key(&mut self, _cx: &Cx, _key: Key) -> Nav {
        Nav::Ignored
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        // Touch: the phone's details and its network's side by side, and
        // the restart across the bottom.
        let area = if th.touch() {
            let (rest, buttons) = menus::bottom_buttons(th, area, 2);
            let (left, right) = menus::halves(th, buttons[0]);
            widgets::button_styled(c, th, left, tr::ABOUT_PHONE, widgets::ButtonStyle::Secondary, Hit::Soft(1));
            widgets::button_styled(c, th, right, tr::NETWORK_STATUS, widgets::ButtonStyle::Secondary, Hit::Soft(2));
            widgets::button_styled(c, th, buttons[1], tr::RESTART_PHONE, widgets::ButtonStyle::Primary, Hit::Soft(4));
            rest
        } else {
            area
        };
        let (title, body) = area.split_top(th.title_h);
        title_band(c, th, title, tr::RECOVERY_TITLE);
        let mut text = tr::RECOVERY_TEXT.to_string();
        if let Some(ip) = cx.st.network.ipv4.address {
            // (Touch: the address on a line of its own.)
            text = format!("{text}{}http://{ip}/", if th.touch() { "\n\n" } else { " " });
        }
        let f = th.font(if th.gray { Font::Small } else { Font::Normal });
        let lh = if th.gray { th.line_h - 3 } else { th.line_h };
        let inset = if th.touch() { th.title_pad } else { th.pad };
        c.text_wrapped(f, body.inset_xy(inset, if th.touch() { th.pad } else { 2 }), &text, th.fg, HAlign::Left, lh);
    }
}

/// DPMA provisioning, following `Provisioning::phase`.
///
/// While the server list holds exactly one server, the screen counts down
/// and then selects it by itself, as stock's `choosePbx.js` does
/// (`startSingleCountdown`): the count starts when the list is first drawn
/// with its one entry, a key press other than Select stops it for as long
/// as the list is shown, and so does a second server.
///
/// Touch: each list has a line under its title on what to do, and the
/// users' list a field that narrows it (by name or extension) as it is
/// typed in, with the keyboard below while it is.  The buttons are across
/// the bottom: "Use without a server" apart from the servers, a Cancel
/// while a server is contacted (it can take most of a minute to give up),
/// and after a failure "Try again" and "Enter a different address".  What
/// a first-time user rarely needs (the network's settings, the phone's
/// details) is under More.  A list shown again after Cancel does not
/// count down to its lone server: the user just turned it down.
#[derive(Default)]
pub(crate) struct Provision {
    servers: ListState,
    users: ListState,
    countdown: Countdown,
    /// Touch: what the users' list is narrowed to.
    filter: String,
    filter_cursor: u16,
    /// Touch: the keyboard of the filter, while it is up.
    keyboard: Option<crate::osk::Keyboard>,
    /// Touch: where the filter was drawn.
    filter_field: IRect,
    /// Touch: the user canceled the contact with a server, and the list
    /// that follows has no count.
    canceled: bool,
}

/// The lone-server countdown of the server list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Countdown {
    /// The list is not shown, or holds no server yet.
    #[default]
    Idle,
    /// Counting since this monotonic time (the first draw of the list with
    /// its one server).
    Since(u64),
    /// Stopped by a key or by a second server, or run out.
    Stopped,
}

/// Stock counts 5, 4, 3, 2, 1, 0, one number a second, and selects a
/// second after the 0: six seconds in all.
const AUTO_SELECT_MS: u64 = 6000;
/// Followed by the seconds left (stock: "Automatically selecting in...%s").
const AUTO_SELECTING: &str = "Automatically selecting in\u{2026}";

/// Touch: the list's screen away from its rows ([`Hit::Custom`]).  A touch
/// there stops the count, as a key other than Select does (and puts the
/// filter's keyboard away).
const HIT_ELSEWHERE: u32 = 1;
/// Touch: the field that narrows the list of users.
const HIT_FILTER: u32 = 2;

/// The actions of the softkeys and of the touch skin's buttons.
const SK_SELECT: u32 = 1;
const SK_RETRY: u32 = 2;
const SK_MANUAL: u32 = 3;
const SK_ABOUT: u32 = 4;
const SK_SKIP: u32 = 5;
const SK_NETWORK: u32 = 6;
/// Touch: what a first-time user rarely needs ([`more`]).
const SK_MORE: u32 = 7;
/// Touch: stop contacting the server.
const SK_CANCEL: u32 = 8;

/// The longest filter of the users' list.
const FILTER_MAX: usize = 40;

/// Touch: the setup's rarer choices: going on without a server (when
/// `skip`: after a failure, where it is not a button of its own), the
/// network's settings, and the phone's details.
fn more(skip: bool) -> Box<dyn Screen> {
    Menu::boxed("provision-more", tr::MORE_CHOICES, move |_| {
        let mut v = Vec::new();
        if skip {
            v.push(Item::new(tr::PROV_NO_SERVER, |cx| {
                cx.post(Command::SkipProvisioning);
                Nav::Pop
            }));
        }
        v.push(Item::new(tr::NETWORK_SETTINGS, |_| Nav::Push(menus::network_menu())));
        v.push(Item::new(tr::ABOUT_PHONE, |_| Nav::Push(menus::about())));
        v
    })
}

/// Touch: one line under a list's title on what to do with the list, in
/// `r`, and what is left of `r` under it.
fn instruction(c: &mut Canvas, th: &Theme, r: IRect, text: &str) -> IRect {
    let (line, rest) = r.split_top(th.line_h + th.pad * 2);
    c.text_in(th.font(Font::Normal), line.inset_xy(th.title_pad, 0), text, th.secondary, HAlign::Left);
    rest
}

/// Touch: a row of two lines, `first` over a dimmer `second` (or `first`
/// alone, when `second` is empty), in `r`.
fn two_lines(c: &mut Canvas, th: &Theme, r: IRect, first: &str, second: &str, fg: Rgb) {
    let r = r.inset_xy(th.title_pad, 0);
    if second.is_empty() {
        return c.text_in(th.font(Font::Normal), r, first, fg, HAlign::Left);
    }
    let (top, bottom) = r.inset_xy(0, th.pad / 2).split_top((r.h - th.pad) / 2);
    c.text_in(th.font(Font::Normal), top, first, fg, HAlign::Left);
    c.text_in(th.font(Font::Small), bottom, second, th.secondary, HAlign::Left);
}

fn manual_server() -> Box<dyn Screen> {
    EditScreen::boxed("provision-server", tr::PROV_ENTER_SERVER, "", InputMode::Text, |cx, text| {
        let t = text.trim();
        if !t.is_empty() {
            cx.post(Command::StartProvisioning { server: Some(t.to_string()) });
        }
        Nav::Pop
    })
    .action(tr::CONNECT)
}

impl Provision {
    /// The servers found, "Enter address...", and "Use without a server"
    /// (touch: a button of its own).
    fn server_rows(cx: &Cx) -> usize {
        cx.st.provisioning.discovered.len() + if cx.th.touch() { 1 } else { 2 }
    }
    /// The users shown, as indexes into the users: those the filter
    /// matches (by extension or name, in any case).
    fn shown_users(&self, cx: &Cx) -> Vec<usize> {
        let f = self.filter.trim().to_lowercase();
        let users = &cx.st.provisioning.users;
        (0..users.len()).filter(|i| f.is_empty() || users[*i].id.to_lowercase().contains(&f) || users[*i].name.to_lowercase().contains(&f)).collect()
    }
    /// Follows the list: called at each draw, so the count runs only for a
    /// list that has been painted.
    fn follow(&mut self, cx: &Cx) {
        let p = &cx.st.provisioning;
        self.countdown = match (self.countdown, &p.phase, p.discovered.len()) {
            (_, phase, _) if *phase != ProvisionPhase::AwaitingServer => Countdown::Idle,
            (Countdown::Idle, _, 1) if self.canceled => Countdown::Stopped,
            (Countdown::Idle, _, 1) => {
                // Stock puts the selection on the entry it counts down to.
                self.servers = ListState::default();
                Countdown::Since(cx.now_ms)
            }
            (Countdown::Since(_), _, n) if n != 1 => Countdown::Stopped,
            (other, _, _) => other,
        };
    }
    /// The seconds shown while the count runs.
    fn seconds_left(&self, cx: &Cx) -> Option<u64> {
        match self.countdown {
            Countdown::Since(t) if cx.st.provisioning.phase == ProvisionPhase::AwaitingServer => {
                Some((AUTO_SELECT_MS / 1000 - 1).saturating_sub(cx.now_ms.saturating_sub(t) / 1000))
            }
            _ => None,
        }
    }
    fn pick_server(&mut self, cx: &Cx) -> Nav {
        let d = &cx.st.provisioning.discovered;
        let i = self.servers.selected;
        if let Some(s) = d.get(i) {
            self.canceled = false;
            cx.post(Command::StartProvisioning { server: Some(s.address.clone()) });
            Nav::Stay
        } else if i == d.len() {
            Nav::Push(manual_server())
        } else {
            cx.post(Command::SkipProvisioning);
            Nav::Stay
        }
    }
    fn pick_user(&self, cx: &Cx) -> Nav {
        let shown = self.shown_users(cx);
        let Some(u) = shown.get(self.users.selected).and_then(|i| cx.st.provisioning.users.get(*i)) else { return Nav::Stay };
        let id = u.id.clone();
        if u.needs_pin {
            Nav::Push(EditScreen::boxed("provision-pin", tr::PROV_ENTER_PIN, "", InputMode::Pin, move |cx, pin| {
                cx.post(Command::SelectUser { id: id.clone(), pin: pin.to_string() });
                Nav::Pop
            }))
        } else {
            cx.post(Command::SelectUser { id, pin: String::new() });
            Nav::Stay
        }
    }
}

impl Screen for Provision {
    fn name(&self) -> String {
        "provisioning".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the screen draws its buttons itself.
        if cx.th.touch() {
            return Vec::new();
        }
        match &cx.st.provisioning.phase {
            // Network leads to the phone's network settings from every
            // setup screen that takes keys (stock: the Settings softkey of
            // `choosePbx.js` and `noAvahiServers.js`); Cancel there comes
            // back here.
            ProvisionPhase::AwaitingServer | ProvisionPhase::AwaitingUser => {
                vec![soft(SK_SELECT, tr::SELECT), soft(SK_NETWORK, tr::NETWORK), soft(0, ""), soft(SK_ABOUT, tr::STATUS)]
            }
            ProvisionPhase::Failed { .. } => {
                vec![
                    soft(SK_RETRY, tr::RETRY),
                    soft(SK_MANUAL, tr::MANUAL),
                    soft(SK_SKIP, tr::SKIP),
                    soft(SK_NETWORK, tr::NETWORK),
                    soft(SK_ABOUT, tr::STATUS),
                ]
            }
            ProvisionPhase::Idle | ProvisionPhase::Discovering => {
                vec![soft(SK_MANUAL, tr::MANUAL), soft(SK_NETWORK, tr::NETWORK), soft(SK_SKIP, tr::SKIP), soft(SK_ABOUT, tr::STATUS)]
            }
            _ => Vec::new(),
        }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            SK_SELECT => self.key(cx, Key::Select),
            SK_RETRY => {
                cx.post(Command::StartProvisioning { server: None });
                Nav::Stay
            }
            SK_MANUAL => Nav::Push(manual_server()),
            SK_ABOUT => Nav::Push(menus::about()),
            SK_SKIP => {
                cx.post(Command::SkipProvisioning);
                Nav::Stay
            }
            SK_NETWORK => Nav::Push(menus::network_menu()),
            SK_MORE => Nav::Push(more(matches!(cx.st.provisioning.phase, ProvisionPhase::Failed { .. }))),
            SK_CANCEL => {
                self.canceled = true;
                cx.post(Command::CancelProvisioning);
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        match self.countdown {
            Countdown::Since(t) if cx.st.provisioning.phase == ProvisionPhase::AwaitingServer => {
                // The next change of the number, or the end.
                let elapsed = cx.now_ms.saturating_sub(t);
                Some(t + ((elapsed / 1000 + 1) * 1000).min(AUTO_SELECT_MS))
            }
            _ => None,
        }
    }
    fn tick(&mut self, cx: &Cx) -> Nav {
        if let Countdown::Since(t) = self.countdown {
            let p = &cx.st.provisioning;
            if p.phase == ProvisionPhase::AwaitingServer && p.discovered.len() == 1 && cx.now_ms >= t + AUTO_SELECT_MS {
                self.countdown = Countdown::Stopped;
                cx.post(Command::StartProvisioning { server: Some(p.discovered[0].address.clone()) });
            }
        }
        Nav::Stay
    }
    fn leave(&mut self, _cx: &Cx) {
        // Covered by another screen (address entry, status): stock stops
        // the count for Settings, Info, and About alike.
        if self.countdown != Countdown::Idle {
            self.countdown = Countdown::Stopped;
        }
    }
    fn list_state(&mut self, cx: &Cx) -> Option<(&mut ListState, usize)> {
        match cx.st.provisioning.phase {
            ProvisionPhase::AwaitingServer => Some((&mut self.servers, Self::server_rows(cx))),
            ProvisionPhase::AwaitingUser => {
                let len = self.shown_users(cx).len();
                Some((&mut self.users, len))
            }
            _ => None,
        }
    }
    /// Touch: a row is chosen as Select chooses it (the default, through
    /// [`Screen::list_state`]); a touch away from the rows stops the count
    /// (and puts the filter's keyboard away); the filter's field brings up
    /// its keyboard, whose keys narrow the list.
    fn tap(&mut self, cx: &Cx, hit: Hit, x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Custom(HIT_ELSEWHERE) => {
                if self.countdown != Countdown::Idle {
                    self.countdown = Countdown::Stopped;
                }
                self.keyboard = None;
                Nav::Stay
            }
            Hit::Custom(HIT_FILTER) => {
                let at = widgets::text_field_cursor_at(cx.th, self.filter_field, &self.filter, self.filter_cursor as usize, InputMode::Text, false, Font::Normal, x);
                self.filter_cursor = if self.keyboard.is_some() { at as u16 } else { self.filter.chars().count() as u16 };
                self.keyboard.get_or_insert(crate::osk::Keyboard::new(InputMode::Text));
                Nav::Stay
            }
            hit if crate::osk::Keyboard::owns(hit) => {
                let Some(kb) = self.keyboard.as_mut() else { return Nav::Stay };
                match kb.edit(hit, &mut self.filter, &mut self.filter_cursor, FILTER_MAX) {
                    Some(crate::osk::Press::Done) => self.keyboard = None,
                    Some(crate::osk::Press::Char(_) | crate::osk::Press::Backspace) => self.users = ListState::default(),
                    _ => {}
                }
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    /// Touch: a finger scrolling the list stops the count, as a key does
    /// (and the list scrolls).
    fn drag(&mut self, _cx: &Cx, _from: Option<Hit>, _dx: i32, _dy: i32) -> Nav {
        if self.countdown != Countdown::Idle {
            self.countdown = Countdown::Stopped;
        }
        Nav::Ignored
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        // Any key but Select stops the count; Select picks the entry now.
        if self.countdown != Countdown::Idle {
            self.countdown = Countdown::Stopped;
        }
        let users = self.shown_users(cx).len();
        let (st, len) = match cx.st.provisioning.phase {
            ProvisionPhase::AwaitingServer => (&mut self.servers, Self::server_rows(cx)),
            ProvisionPhase::AwaitingUser => (&mut self.users, users),
            _ => return Nav::Ignored,
        };
        match key {
            Key::Up => {
                st.step(-1, len, true);
                Nav::Stay
            }
            Key::Down => {
                st.step(1, len, true);
                Nav::Stay
            }
            Key::Select => match cx.st.provisioning.phase {
                ProvisionPhase::AwaitingServer => self.pick_server(cx),
                _ => self.pick_user(cx),
            },
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        self.follow(cx);
        let th = cx.th;
        if th.touch() {
            return self.draw_touch(cx, c, area);
        }
        let p = &cx.st.provisioning;
        let f = th.font(Font::Normal);
        let counting = self.seconds_left(cx);
        // The list, under its title.
        let listing = |c: &mut Canvas, title: &str, rows: Vec<String>, st: &mut ListState| {
            c.fill(area, th.bg);
            let (t, body) = area.split_top(th.title_h);
            title_band(c, th, t, title);
            let body = widgets::body_rows(th, body);
            widgets::list(c, th, body, rows.len(), st, th.row_h, true, |c, r, i, _, fg| {
                c.text_in(f, r.inset_xy(th.pad * 2, 0), &rows[i], fg, HAlign::Left);
            });
        };
        match &p.phase {
            ProvisionPhase::AwaitingServer => {
                let mut rows: Vec<String> = p
                    .discovered
                    .iter()
                    .map(|s| if s.name.is_empty() { s.address.clone() } else { format!("{} ({})", s.name, s.address) })
                    .collect();
                rows.push(tr::PROV_ENTER_MANUALLY.into());
                rows.push(tr::PROV_NO_SERVER.into());
                listing(c, tr::PROV_SELECT_SERVER, rows, &mut self.servers);
                if let Some(n) = counting {
                    // The last line of the body, below the three rows.
                    let line = IRect { y: area.bottom() - th.line_h, h: th.line_h, ..area };
                    c.fill(line, th.bg);
                    c.text_in(f, line, &format!("{}{n}", AUTO_SELECTING), th.fg, HAlign::Center);
                }
            }
            ProvisionPhase::AwaitingUser => {
                let rows = p
                    .users
                    .iter()
                    .map(|u| if u.name.is_empty() || u.name == u.id { u.id.clone() } else { format!("{}  {}", u.id, u.name) })
                    .collect();
                listing(c, tr::PROV_SELECT_USER, rows, &mut self.users);
            }
            ProvisionPhase::Failed { reason, code, .. } => {
                let text = match code {
                    Some(code) => format!("{reason} ({code})"),
                    None => reason.clone(),
                };
                notice(c, th, area, "warning", th.warn, tr::PROV_FAILED, &text, "", None);
            }
            other => notice(c, th, area, "app-network", th.accent, "", &progress_text(p, other), "", None),
        }
    }
}

/// What the setup is doing, with the server it is contacting under it.
fn progress_text(p: &opendeskphone_state::Provisioning, phase: &ProvisionPhase) -> String {
    let text = match phase {
        ProvisionPhase::Handshaking => tr::PROV_CONTACTING,
        ProvisionPhase::FetchingConfig => tr::PROV_FETCHING,
        ProvisionPhase::Applying | ProvisionPhase::Provisioned | ProvisionPhase::Standalone => tr::PROV_APPLYING,
        _ => tr::PROV_DISCOVERING,
    };
    let server = p.server.as_ref().map(|s| s.address.as_str()).unwrap_or("");
    if server.is_empty() { text.to_string() } else { format!("{text}\n{server}") }
}

impl Provision {
    /// Touch: the setup's screens (see [`Provision`]).
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let p = &cx.st.provisioning;
        let button = |c: &mut Canvas, r: IRect, label: &str, role: widgets::ButtonStyle, id: u32| {
            widgets::button_styled(c, th, r, label, role, Hit::Soft(id));
        };
        match &p.phase {
            ProvisionPhase::AwaitingServer => self.draw_servers(cx, c, area),
            ProvisionPhase::AwaitingUser => self.draw_users(cx, c, area),
            ProvisionPhase::Failed { reason, code, detail } => {
                // What to do, and under it, small, what happened.
                let detail = match (detail.as_str(), code) {
                    ("", Some(code)) => format!("{} {code}", tr::PROV_ERROR),
                    (d, Some(code)) => format!("{d} ({} {code})", tr::PROV_ERROR),
                    (d, None) => d.to_string(),
                };
                let (rest, b) = menus::bottom_buttons(th, area, 3);
                button(c, b[0], tr::TRY_AGAIN, widgets::ButtonStyle::Primary, SK_RETRY);
                button(c, b[1], tr::PROV_OTHER_ADDRESS, widgets::ButtonStyle::Secondary, SK_MANUAL);
                button(c, b[2], tr::MORE_CHOICES, widgets::ButtonStyle::Secondary, SK_MORE);
                notice(c, th, rest, "warning", th.warn, tr::PROV_FAILED, reason, &detail, None);
            }
            ProvisionPhase::Idle | ProvisionPhase::Discovering => {
                let (rest, b) = menus::bottom_buttons(th, area, 2);
                button(c, b[0], tr::PROV_ENTER_MANUALLY, widgets::ButtonStyle::Secondary, SK_MANUAL);
                let (skip, more) = menus::halves(th, b[1]);
                button(c, skip, tr::PROV_NO_SERVER, widgets::ButtonStyle::Secondary, SK_SKIP);
                button(c, more, tr::MORE_CHOICES, widgets::ButtonStyle::Secondary, SK_MORE);
                notice(c, th, rest, "app-network", th.accent, "", &progress_text(p, &p.phase), "", None);
            }
            ProvisionPhase::Handshaking => {
                let (rest, b) = menus::bottom_buttons(th, area, 1);
                button(c, b[0], tr::CANCEL, widgets::ButtonStyle::Secondary, SK_CANCEL);
                notice(c, th, rest, "app-network", th.accent, "", &progress_text(p, &p.phase), "", None);
            }
            other => notice(c, th, area, "app-network", th.accent, "", &progress_text(p, other), "", None),
        }
    }

    /// Touch: the servers found, and "Enter address..." after them; "Use
    /// without a server" and More across the bottom.
    fn draw_servers(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let p = &cx.st.provisioning;
        c.fill(area, th.bg);
        let (t, body) = area.split_top(th.title_h);
        title_band(c, th, t, tr::PROV_SELECT_SERVER);
        c.hit(body, Hit::Custom(HIT_ELSEWHERE));
        let (body, b) = menus::bottom_buttons(th, body, 1);
        let (skip, more) = menus::halves(th, b[0]);
        widgets::button_styled(c, th, skip, tr::PROV_NO_SERVER, widgets::ButtonStyle::Secondary, Hit::Soft(SK_SKIP));
        widgets::button_styled(c, th, more, tr::MORE_CHOICES, widgets::ButtonStyle::Secondary, Hit::Soft(SK_MORE));
        let body = instruction(c, th, body, tr::PROV_SELECT_SERVER_HINT);
        // There is no cursor, but the server the count will choose is
        // shown chosen.
        let counting = self.seconds_left(cx);
        let found = &p.discovered;
        let shown = widgets::list(c, th, body, found.len() + 1, &mut self.servers, th.row_h, counting.is_some(), |c, r, i, _, fg| {
            match found.get(i) {
                Some(s) if s.name.is_empty() || s.name == s.address => two_lines(c, th, r, &s.address, "", fg),
                Some(s) => two_lines(c, th, r, &s.name, &s.address, fg),
                None => two_lines(c, th, r, tr::PROV_ENTER_MANUALLY, "", fg),
            }
        });
        if let Some(n) = counting {
            // Right below the rows, where the eye is.
            let end = body.y + shown.min(found.len() + 1) as i32 * th.row_h;
            let line = IRect { y: (end + th.pad).min(body.bottom() - th.line_h), h: th.line_h, ..body };
            c.text_in(th.font(Font::Normal), line, &format!("{}{n}", AUTO_SELECTING), th.fg, HAlign::Center);
        }
    }

    /// Touch: the users the filter matches, under the filter's field; More
    /// across the bottom, or the filter's keyboard while it is up.
    fn draw_users(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let users = &cx.st.provisioning.users;
        c.fill(area, th.bg);
        let (t, body) = area.split_top(th.title_h);
        title_band(c, th, t, tr::PROV_SELECT_USER);
        c.hit(body, Hit::Custom(HIT_ELSEWHERE));
        let body = match &self.keyboard {
            Some(kb) => {
                let (rest, keys) = body.split_bottom(crate::osk::Keyboard::height(th));
                kb.draw(c, th, keys);
                rest
            }
            None => {
                let (rest, b) = menus::bottom_buttons(th, body, 1);
                widgets::button_styled(c, th, b[0], tr::MORE_CHOICES, widgets::ButtonStyle::Secondary, Hit::Soft(SK_MORE));
                rest
            }
        };
        let body = instruction(c, th, body, tr::PROV_SELECT_USER_HINT);
        let (field, body) = body.split_top(th.touch_min + th.pad * 2);
        self.filter_field = field.inset_xy(th.pad * 2, th.pad);
        let focused = self.keyboard.is_some();
        widgets::text_field(c, th, self.filter_field, &self.filter, self.filter_cursor as usize, InputMode::Text, focused, false, tr::PROV_FILTER, Font::Normal);
        c.hit(self.filter_field, Hit::Custom(HIT_FILTER));
        let shown = self.shown_users(cx);
        if shown.is_empty() && !users.is_empty() {
            return menus::empty_state(c, th, body, tr::PROV_NO_MATCH, tr::PROV_NO_MATCH_HINT);
        }
        widgets::list(c, th, body, shown.len(), &mut self.users, th.row_h, false, |c, r, i, _, fg| {
            let u = &users[shown[i]];
            if u.name.is_empty() || u.name == u.id {
                two_lines(c, th, r, &u.id, "", fg);
            } else {
                two_lines(c, th, r, &u.name, &u.id, fg);
            }
        });
    }
}

/// Firmware update, from the image arriving to its trial being confirmed.
///
/// While the image is received and written, keys are ignored.  After the
/// restart into the new image this screen comes back as the trial cover
/// (`finishing`), in place of the idle screen, until the trial is
/// confirmed or the phone goes back to the old image; no key does
/// anything on the cover itself, but calls work over it as over any base
/// screen (see `Stack::wanted` in `ui.rs` for when it shows and for how
/// long).
pub(crate) struct UpdateScreen {
    pub finishing: bool,
}

/// (label, percent) for an update phase.
pub(crate) fn update_progress(p: &UpdatePhase) -> (&'static str, Option<u32>) {
    match p {
        UpdatePhase::Idle => ("", None),
        UpdatePhase::Receiving { percent } => (tr::UPD_RECEIVING, Some(*percent as u32)),
        UpdatePhase::Checking => (tr::UPD_CHECKING, None),
        UpdatePhase::Erasing { percent } => (tr::UPD_ERASING, Some(*percent as u32)),
        UpdatePhase::WritingKernel { percent } => (tr::UPD_KERNEL, Some(*percent as u32)),
        UpdatePhase::WritingRootfs { percent } => (tr::UPD_ROOTFS, Some(*percent as u32)),
        UpdatePhase::Verifying { percent } => (tr::UPD_VERIFYING, Some(*percent as u32)),
        UpdatePhase::Committing => (tr::UPD_COMMITTING, None),
        UpdatePhase::Complete => (tr::UPD_COMPLETE, Some(100)),
        UpdatePhase::Failed(_) => (tr::UPD_FAILED, None),
    }
}

impl Screen for UpdateScreen {
    fn name(&self) -> String {
        if self.finishing { "update-trial".into() } else { "update".into() }
    }
    fn key(&mut self, _cx: &Cx, _key: Key) -> Nav {
        if self.finishing { Nav::Ignored } else { Nav::Stay }
    }
    fn hook(&mut self, _cx: &Cx, _off: bool) -> Nav {
        if self.finishing { Nav::Ignored } else { Nav::Stay }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let u = &cx.st.update;
        let (label, pct) = if self.finishing { (tr::UPD_TRIAL, None) } else { update_progress(&u.phase) };
        let mut text = label.to_string();
        if !self.finishing && !u.incoming_version.is_empty() {
            text = format!("{text} ({})", u.incoming_version);
        }
        if !matches!(u.phase, UpdatePhase::Complete) {
            text = format!("{text}\n{}", tr::UPDATE_WARNING);
        }
        notice(c, cx.th, area, "app-update", cx.th.accent, tr::UPDATE_TITLE, &text, "", pct);
    }
}
