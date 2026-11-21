//! The dialer and the call screen.
//!
//! On the touch skin (D80) both are made of buttons ([`dialpad`]), and
//! have no softkeys.  The dialer has a dial pad whose keys are the digit
//! keys, the number with a button that deletes, the accounts to choose
//! from, and the buttons that call.  The call screen has a card for each
//! call (a touch selects the call), the lines, and a button for each
//! thing a key does during a call on a phone that has keys: where there
//! is such a key the button is that key ([`Hit::Key`]), so that what it
//! does is in one place.  (The volume is the exception: the screen shows
//! the level between its two buttons, where the keys raise a popup.)

use super::dialpad::Look;
use super::*;
use crate::gfx::Rgb;
use opendeskphone_state::{
    AccountId, AudioPath, Call, CallId, CallState, Direction, EndCause, LampColor, Tone, VolumeTarget, VOLUME_MAX,
};

/// Why the dialer is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Call,
    /// Transfer this call to the number entered.
    Transfer(CallId),
    /// Call a third party to join with this call.
    Conference(CallId),
}

pub(crate) struct Dialer {
    purpose: Purpose,
    pub text: String,
    account: Option<AccountId>,
    /// Opened by going off-hook (handset, speaker, or headset key): dial
    /// tone plays and digits time out into a call.
    pub live: Option<AudioPath>,
    last_key_ms: u64,
    tone: bool,
}

impl Dialer {
    pub fn new(purpose: Purpose, account: Option<AccountId>) -> Dialer {
        Dialer { purpose, text: String::new(), account, live: None, last_key_ms: 0, tone: false }
    }

    /// A dialer opened by lifting the handset or pressing speaker/headset.
    pub fn off_hook(cx: &Cx, path: AudioPath, account: Option<AccountId>) -> Dialer {
        cx.post(Command::SetAudioPath { path });
        cx.post(Command::PlayTone { tone: Tone::Dial, duration_ms: 0 });
        Dialer { purpose: Purpose::Call, text: String::new(), account, live: Some(path), last_key_ms: cx.now_ms, tone: true }
    }

    fn stop_tone(&mut self, cx: &Cx) {
        if self.tone {
            self.tone = false;
            cx.post(Command::StopTone);
        }
    }

    pub fn type_char(&mut self, cx: &Cx, ch: char) {
        self.stop_tone(cx);
        if self.text.len() < 64 {
            self.text.push(ch);
        }
        self.last_key_ms = cx.now_ms;
        if self.live.is_some() {
            cx.post(Command::PlayTone { tone: Tone::Dtmf(ch), duration_ms: 100 });
        }
    }

    fn close_live(&mut self, cx: &Cx) {
        self.stop_tone(cx);
        if self.live.take().is_some() && cx.st.calls.is_idle() {
            cx.post(Command::SetAudioPath { path: AudioPath::None });
        }
    }

    /// The dial plan of the account the call would go out on.
    fn plan<'a>(&self, cx: &'a Cx) -> Option<&'a crate::dialplan::DialPlan> {
        let acct = self
            .account
            .or(cx.st.accounts.default_account)
            .and_then(|id| cx.st.accounts.get(id))
            .or(cx.st.accounts.accounts.first());
        match acct.and_then(|a| cx.cfg.dial_plans.iter().find(|(id, _)| *id == a.config_id)) {
            Some((_, plan)) => plan.as_ref(),
            None => cx.cfg.default_dial_plan.as_ref(),
        }
    }

    /// Whether the digits are dialed with no key press: an off-hook
    /// dialer for a new call with something entered.
    fn dials_by_itself(&self) -> bool {
        self.live.is_some() && !self.text.is_empty() && self.purpose == Purpose::Call
    }

    /// When the digits entered are dialed if no further key comes: when
    /// the dial plan says so (stock), and at the latest after the
    /// inter-digit timeout (not stock: there, digits that could still
    /// become a match wait for a key, and without a plan nothing is
    /// dialed).
    fn dial_at(&self, cx: &Cx) -> Option<u64> {
        if !self.dials_by_itself() {
            return None;
        }
        let timeout = self.last_key_ms + cx.cfg.dial_timeout_ms as u64;
        let elapsed = (cx.now_ms.saturating_sub(self.last_key_ms) / 1000) as u32;
        let by_plan = self
            .plan(cx)
            .and_then(|plan| plan.dial_after(&self.text, elapsed))
            .map(|secs| self.last_key_ms + u64::from(secs) * 1000);
        Some(by_plan.map_or(timeout, |at| at.min(timeout)))
    }

    fn dial(&mut self, cx: &Cx, path: Option<AudioPath>) -> Nav {
        let target = self.text.trim().to_string();
        if target.is_empty() {
            if self.purpose == Purpose::Call {
                self.stop_tone(cx);
                self.live = None;
                cx.post(Command::Redial);
                return Nav::Pop;
            }
            return Nav::Stay;
        }
        self.stop_tone(cx);
        let path = path.or(self.live);
        self.live = None;
        match self.purpose {
            Purpose::Transfer(call) => cx.post(Command::TransferBlind { call, target }),
            _ => cx.post(Command::Dial { target, account: self.account, path }),
        }
        Nav::Pop
    }
}

impl Screen for Dialer {
    fn name(&self) -> String {
        "dialer".into()
    }
    fn tag(&self) -> Tag {
        Tag::Dialer
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the screen has its own buttons for these.
        if cx.th.touch() {
            return Vec::new();
        }
        match self.purpose {
            Purpose::Transfer(_) => vec![soft(1, tr::TRANSFER), soft(2, tr::CONSULT), soft(3, tr::DELETE), soft(4, tr::CANCEL)],
            _ => vec![
                soft(1, if self.text.is_empty() { tr::REDIAL } else { tr::DIAL }),
                soft(5, tr::CONTACTS),
                soft(3, tr::DELETE),
                soft(4, tr::CANCEL),
            ],
        }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 => self.dial(cx, None),
            2 => {
                // Consultation call: place a normal call; the call screen
                // then offers to complete the transfer.
                let target = self.text.trim().to_string();
                if target.is_empty() {
                    return Nav::Stay;
                }
                cx.post(Command::Dial { target, account: self.account, path: None });
                Nav::Pop
            }
            3 => {
                self.text.pop();
                self.last_key_ms = cx.now_ms;
                Nav::Stay
            }
            4 => {
                self.close_live(cx);
                Nav::Pop
            }
            5 => {
                cx.post(Command::LaunchApp { name: "contacts".into() });
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match key {
            Key::Digit(_) | Key::Star | Key::Pound => {
                self.type_char(cx, key.dial_char().unwrap_or('0'));
                // Stock: the call starts by itself once the dial plan
                // says the number is complete.
                if self.dial_at(cx).is_some_and(|at| cx.now_ms >= at) {
                    return self.dial(cx, None);
                }
                Nav::Stay
            }
            Key::Select => self.dial(cx, None),
            Key::Left => {
                self.text.pop();
                self.last_key_ms = cx.now_ms;
                Nav::Stay
            }
            Key::Cancel => {
                self.close_live(cx);
                Nav::Pop
            }
            Key::Speaker if self.live != Some(AudioPath::Speaker) => self.dial_or_switch(cx, AudioPath::Speaker),
            Key::Headset if self.live != Some(AudioPath::Headset) => self.dial_or_switch(cx, AudioPath::Headset),
            Key::Speaker | Key::Headset => {
                // The key that opened the dialer closes it again.
                self.close_live(cx);
                Nav::Pop
            }
            Key::Line(n) => {
                let acct = cx.st.keys.line_keys.get(n as usize - 1).and_then(|k| match k.binding {
                    opendeskphone_state::LineKeyBinding::Account(a) => Some(a),
                    _ => None,
                });
                if acct.is_some() {
                    self.account = acct;
                    if !self.text.is_empty() {
                        return self.dial(cx, None);
                    }
                    return Nav::Stay;
                }
                Nav::Ignored
            }
            Key::Redial if self.text.is_empty() => self.dial(cx, None),
            _ => Nav::Ignored,
        }
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Custom(HIT_DELETE) => self.softkey(cx, 3),
            Hit::Custom(n) if n >= HIT_ACCOUNT => {
                if let Some(a) = cx.st.accounts.accounts.get((n - HIT_ACCOUNT) as usize) {
                    self.account = Some(a.id);
                }
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn long_press(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            // A finger that stays on the delete button deletes it all.
            Hit::Custom(HIT_DELETE) => {
                self.text.clear();
                self.last_key_ms = cx.now_ms;
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn hook(&mut self, cx: &Cx, off_hook: bool) -> Nav {
        if off_hook {
            if self.text.is_empty() {
                // Lifted with nothing typed: become a live dialer.
                if self.live.is_none() {
                    cx.post(Command::PlayTone { tone: Tone::Dial, duration_ms: 0 });
                    self.tone = true;
                }
                cx.post(Command::SetAudioPath { path: AudioPath::Handset });
                self.live = Some(AudioPath::Handset);
                self.last_key_ms = cx.now_ms;
                Nav::Stay
            } else {
                self.dial(cx, Some(AudioPath::Handset))
            }
        } else if self.live == Some(AudioPath::Handset) {
            self.close_live(cx);
            Nav::Pop
        } else {
            Nav::Stay
        }
    }
    fn wallpaper(&self) -> bool {
        true
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let label = match self.purpose {
            Purpose::Transfer(_) => tr::TRANSFER_TO,
            Purpose::Conference(_) => tr::CONFERENCE_WITH,
            _ => tr::NEW_CALL_TITLE,
        };
        if th.touch() {
            return self.draw_touch(cx, c, area, label);
        }
        call_header(cx, c, label, &[]);
        idle::draw_tabs(cx, c, th.tab_x);
        // Stock: the digits appear on the first line of a call card.
        let acct = self
            .account
            .or(cx.st.accounts.default_account)
            .and_then(|id| cx.st.accounts.get(id))
            .or(cx.st.accounts.accounts.first());
        let via = match (cx.st.accounts.accounts.len() > 1, acct) {
            (true, Some(a)) => if a.label.is_empty() { a.user.clone() } else { a.label.clone() },
            _ => String::new(),
        };
        let card = Card {
            fill: th.card,
            line1: &self.text,
            line2: &via,
            line2_bold: false,
            icon: "phone-out",
            right: "",
            cursor: true,
            marked: false,
        };
        call_card(cx, c, 0, &card);
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        self.dial_at(cx)
    }
    fn tick(&mut self, cx: &Cx) -> Nav {
        if self.dial_at(cx).is_some_and(|at| cx.now_ms >= at) {
            return self.dial(cx, None);
        }
        Nav::Stay
    }
    fn leave(&mut self, cx: &Cx) {
        self.stop_tone(cx);
    }
    fn dial_buffer(&self) -> Option<&str> {
        Some(&self.text)
    }
}

/// Touch: the regions of the dialer's own ([`Hit::Custom`]): the delete
/// button, and, from `HIT_ACCOUNT`, the accounts in their order.
const HIT_DELETE: u32 = 1;
const HIT_ACCOUNT: u32 = 100;

impl Dialer {
    /// The touch skin's dialer: under the title bar (whose back button
    /// leaves), the accounts when there is more than one, the number, the
    /// dial pad, and the buttons that call.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect, label: &str) {
        let th = cx.th;
        let m = dialpad::MARGIN;
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, label);
        if !matches!(self.purpose, Purpose::Transfer(_)) {
            let w = th.px(116);
            let r = IRect::new(title.right() - m - w, title.y + th.px(2), w, title.h - th.px(4));
            dialpad::control(c, th, r, "contacts", tr::CONTACTS, Look::Plain, Hit::Soft(5));
        }
        let inner = IRect::new(body.x + m, body.y + th.pad, body.w - 2 * m, body.h - th.pad - m);

        // The account the call goes out on, when there is one to choose.
        let accounts = &cx.st.accounts.accounts;
        let chosen = self
            .account
            .or(cx.st.accounts.default_account)
            .filter(|id| cx.st.accounts.get(*id).is_some())
            .or(accounts.first().map(|a| a.id));
        let mut y = inner.y;
        if accounts.len() > 1 {
            let row = IRect::new(inner.x, y, inner.w, th.touch_min);
            for (i, (a, r)) in accounts.iter().zip(dialpad::columns(row, accounts.len(), th.px(6))).enumerate() {
                let label = if a.label.is_empty() { &a.user } else { &a.label };
                let look = if Some(a.id) == chosen { Look::On } else { Look::Plain };
                dialpad::control(c, th, r, "", label, look, Hit::Custom(HIT_ACCOUNT + i as u32));
            }
            y = row.bottom() + th.pad;
        }

        // The number, as on a call card, with the delete button at its
        // right.
        let field = IRect::new(inner.x, y, inner.w, th.px(64));
        c.fill_round_aa(field, th.px(6), th.card);
        let (text, del) = field.split_right(th.px(52));
        let text = IRect { x: text.x + th.pad, w: text.w - th.pad, ..text };
        // The large digits while they fit, then the smaller ones, and then
        // the end of the number.
        let f = if th.font(Font::Large).width(&self.text) < text.w - th.px(4) { th.font(Font::Large) } else { th.number_font() };
        let mut shown: &str = &self.text;
        while f.width(shown) > text.w - th.px(4) && !shown.is_empty() {
            shown = &shown[shown.chars().next().map(|c| c.len_utf8()).unwrap_or(1)..];
        }
        let base = text.y + (text.h - f.height()) / 2 + f.ascent as i32;
        let w = c.text(f, text.x, base, shown, th.fg);
        c.fill(IRect::new(text.x + w + th.px(1), text.y + text.h / 4, th.px(1), text.h / 2), th.fg);
        if let Some(icon) = th.icon("backspace") {
            let dim = crate::gfx::mix(th.card, th.fg, 3);
            c.icon_in(icon, del, if self.text.is_empty() { dim } else { th.fg });
        }
        if !self.text.is_empty() {
            c.hit(IRect { x: del.x - th.px(4), w: del.w + th.px(4), ..del }, Hit::Custom(HIT_DELETE));
        }

        // The buttons that call, at the bottom, and the pad between.
        let actions = IRect::new(inner.x, inner.bottom() - th.px(56), inner.w, th.px(56));
        let gap = th.pad;
        let empty = self.text.is_empty();
        let go = |on: bool| if on { Look::Go } else { Look::Off };
        match self.purpose {
            Purpose::Transfer(_) => {
                let cols = dialpad::columns(actions, 2, gap);
                dialpad::control(c, th, cols[0], "transfer", tr::TRANSFER, go(!empty), Hit::Soft(1));
                let look = if empty { Look::Off } else { Look::Plain };
                dialpad::control(c, th, cols[1], "phone-out", tr::CONSULT, look, Hit::Soft(2));
            }
            Purpose::Conference(_) => dialpad::control(c, th, actions, "phone", tr::DIAL, go(!empty), Hit::Soft(1)),
            Purpose::Call => {
                // Speaker and headset are the keys: with a number they
                // call it there, and without one they go off-hook there.
                let side = th.px(80);
                let (speaker, rest) = actions.split_left(side);
                let (call, headset) = rest.split_right(side);
                let call = call.inset_xy(gap, 0);
                let look = |path| if self.live == Some(path) { Look::On } else { Look::Plain };
                dialpad::control(c, th, speaker, "speaker", tr::VOL_SPEAKER, look(AudioPath::Speaker), Hit::Key(Key::Speaker));
                dialpad::control(c, th, headset, "headset", tr::VOL_HEADSET, look(AudioPath::Headset), Hit::Key(Key::Headset));
                dialpad::control(c, th, call, "phone", if empty { tr::REDIAL } else { tr::DIAL }, Look::Go, Hit::Soft(1));
            }
        }
        let space = IRect::new(inner.x, field.bottom() + gap, inner.w, actions.y - gap - (field.bottom() + gap));
        let key_h = ((space.h - 3 * th.pad) / 4).min(th.px(84));
        let h = dialpad::pad_h(th, key_h);
        dialpad::pad(c, th, IRect::new(space.x, space.y + (space.h - h) / 2, space.w, h));
    }

    fn dial_or_switch(&mut self, cx: &Cx, path: AudioPath) -> Nav {
        if self.text.is_empty() {
            if self.live.is_none() {
                cx.post(Command::PlayTone { tone: Tone::Dial, duration_ms: 0 });
                self.tone = true;
            }
            cx.post(Command::SetAudioPath { path });
            self.live = Some(path);
            self.last_key_ms = cx.now_ms;
            Nav::Stay
        } else {
            self.dial(cx, Some(path))
        }
    }
}

// ------------------------------------------------------------ call screen

/// State text and icon of a call.
/// The parts of a call's status: state label (stock wording), icon, and
/// the running timer once connected.
pub(crate) fn call_parts(call: &Call, now_ms: u64) -> (String, &'static str, Option<String>) {
    let timer = call.duration_secs(now_ms).filter(|_| !call.state.is_ended()).map(crate::time::duration);
    let (label, icon): (String, &'static str) = match &call.state {
        CallState::Dialing | CallState::Proceeding => (tr::CALLING.into(), "phone-out"),
        CallState::Ringback { .. } => (tr::RINGING.into(), "phone-out"),
        CallState::Ringing => (if call.paging { tr::PAGING.into() } else { tr::INCOMING_CALL.into() }, "phone-in"),
        CallState::Active if call.in_conference => (tr::IN_CONFERENCE.into(), "conference"),
        CallState::Active if call.paging => (tr::PAGING.into(), "page"),
        CallState::Active => {
            (tr::CONNECTED.into(), if call.direction == Direction::Outgoing { "phone-out" } else { "phone-in" })
        }
        CallState::Held { local: false, remote: true } => (tr::HELD_BY_REMOTE.into(), "hold"),
        CallState::Held { .. } => (tr::ON_HOLD.into(), "hold"),
        CallState::Transferring => (tr::TRANSFERRING.into(), "transfer"),
        CallState::Ended(cause) => (
            match cause {
                EndCause::Busy => tr::BUSY.into(),
                EndCause::NoAnswer => tr::NO_ANSWER.into(),
                EndCause::Rejected => tr::REJECTED.into(),
                EndCause::Failed { code, reason } => format!("{}: {} {}", tr::CALL_FAILED, code, reason),
                EndCause::Error(e) => format!("{}: {}", tr::CALL_FAILED, e),
                _ => tr::CALL_ENDED.into(),
            },
            "phone",
        ),
    };
    (label, icon, timer)
}

/// State text with the timer appended, and the icon (list rows).
pub(crate) fn call_status(call: &Call, now_ms: u64) -> (String, &'static str) {
    let (label, icon, timer) = call_parts(call, now_ms);
    match timer {
        Some(t) => (format!("{label}  {t}"), icon),
        None => (label, icon),
    }
}

/// The state band at the top left of the call screens: the stock header
/// (x 0..295, y 0..39 on color, with a black line under it; the black bar
/// on gray).  `flags` are icons drawn at its right end.
pub(crate) fn call_header(cx: &Cx, c: &mut Canvas, text: &str, flags: &[&str]) {
    call_header_in(cx, c, cx.th.header_rect(), text, flags);
}

/// [`call_header`], in `r` (the touch skin's is as wide as the screen).
fn call_header_in(cx: &Cx, c: &mut Canvas, r: IRect, text: &str, flags: &[&str]) {
    let th = cx.th;
    if th.gray {
        idle::gray_header(cx, c, r, Some(text));
    } else {
        c.fill(r, th.title_bg);
        c.hline(r.x, r.bottom(), r.w, 0x000001);
        c.text_in(
            th.font(Font::Bold),
            IRect { x: r.x + th.title_pad, w: r.w - 2 * th.title_pad, ..r },
            text,
            th.title_fg,
            HAlign::Left,
        );
    }
    let mut x = r.right() - th.pad;
    for name in flags {
        if let Some(i) = th.icon(name) {
            x -= i.width();
            c.icon(i, x, r.y + (r.h - i.height()) / 2, th.title_fg);
            x -= th.pad;
        }
    }
}

/// What a call card shows.
pub(crate) struct Card<'a> {
    pub fill: Rgb,
    pub line1: &'a str,
    pub line2: &'a str,
    pub line2_bold: bool,
    pub icon: &'a str,
    /// Right-aligned on the second line (the timer).
    pub right: &'a str,
    /// Draw a text cursor after `line1` (the dialer).
    pub cursor: bool,
    /// Mark this card as the one the softkeys act on.
    pub marked: bool,
}

/// Height of one card including its separator line.
pub(crate) fn card_pitch(th: &Theme) -> i32 {
    if th.gray { 2 * th.line_h } else { th.px(63) }
}

/// How many cards fit between the header and the softkeys.
pub(crate) fn max_cards(th: &Theme) -> usize {
    (((th.soft_y - 1) - (th.header_h + 1)) / card_pitch(th)).max(1) as usize
}

/// The rectangle of card `idx` (0 is directly under the header band).
/// Color: 296x62 at y 41, as on stock.
pub(crate) fn card_rect(th: &Theme, idx: usize) -> IRect {
    let r = th.header_rect();
    if th.gray {
        IRect::new(0, th.header_h + 1 + idx as i32 * card_pitch(th), r.w, card_pitch(th))
    } else {
        IRect::new(0, th.header_h + 1 + idx as i32 * card_pitch(th), r.w, card_pitch(th) - 1)
    }
}

/// Draws a call card.
pub(crate) fn call_card(cx: &Cx, c: &mut Canvas, idx: usize, card: &Card) {
    let th = cx.th;
    let r = card_rect(th, idx);
    let icon = th.icon(card.icon);
    if th.gray {
        // Stock gray layout: icon at the left, number then name, the timer
        // at the right of the first line.
        let f = th.font(Font::Normal);
        let (ic, text) = r.split_left(if th.skin == opendeskphone_state::Skin::LargeGray { 26 } else { 22 });
        if card.marked {
            c.fill(IRect::new(r.x, r.y, 2, r.h - 2), th.fg);
        }
        if let Some(i) = icon {
            c.icon(i, ic.x + (ic.w - i.width()) / 2 + 1, r.y + (th.line_h - i.height()) / 2, th.fg);
        }
        let tw = f.width(card.right);
        let l1 = IRect::new(text.x + 6, r.y, text.w - 4 - th.pad - tw - 4, th.line_h);
        c.text_in(f, l1, card.line1, th.fg, HAlign::Left);
        if card.cursor {
            let x = l1.x + f.width(card.line1).min(l1.w) + 1;
            c.vline(x, l1.y + 2, l1.h - 4, th.fg);
        }
        c.text_in(f, IRect::new(text.right() - th.pad - tw, r.y, tw, th.line_h), card.right, th.fg, HAlign::Left);
        c.text_in(f, IRect::new(text.x + 6, r.y + th.line_h, text.w - 6 - th.pad, th.line_h), card.line2, th.fg, HAlign::Left);
        return;
    }
    // The colored part ends in a slanted edge with a light stripe; the
    // right part is dark.  Stock: the dark part starts at x 238 on the
    // first row and x 203 on the last.
    // Stripe colors by measurement for the two stock fills (the skin's
    // own blue and green); derived for the others and in other palettes.
    let (light, mid) = match card.fill {
        0x01579B => (0x67B4F1, 0x3592CA),
        0x2A6B2D => (0x78E27D, 0x388E3C),
        fill => (crate::gfx::mix(fill, 0xFFFFFF, 4), crate::gfx::mix(fill, 0xFFFFFF, 2)),
    };
    for j in 0..r.h {
        let y = r.y + j;
        let xd = 238 - j * 35 / 61;
        c.span(y, r.x, xd - 8, card.fill);
        c.span(y, xd - 8, xd - 5, light);
        c.span(y, xd - 5, xd, mid);
        c.span(y, xd, r.right(), th.card_dark);
    }
    c.hline(r.x, r.bottom(), r.w, 0x000001);
    if card.marked {
        c.fill(IRect::new(r.x, r.y, 3, r.h), th.fg);
    }
    let num = th.number_font();
    // Stock baselines: number at y 64, name and timer at y 90 (card at 41).
    let text_w = 238 - 61 * 35 / 61 - 8 - 10 - 6;
    let old = c.push_clip(IRect::new(r.x + 10, r.y, text_w, r.h));
    let mut shown: &str = card.line1;
    while num.width(shown) > text_w - 3 && !shown.is_empty() {
        shown = &shown[shown.chars().next().map(|c| c.len_utf8()).unwrap_or(1)..];
    }
    let w = c.text(num, r.x + 10, r.y + 23, shown, th.fg);
    if card.cursor {
        c.vline(r.x + 10 + w + 1, r.y + 9, 16, th.fg);
    }
    c.restore_clip(old);
    let f2 = th.font(if card.line2_bold { Font::Bold } else { Font::Normal });
    c.text_in(f2, IRect::new(r.x + 10, r.y + 49 - f2.ascent as i32, text_w, f2.height()), card.line2, th.fg, HAlign::Left);
    if let Some(i) = icon {
        // Stock: a 17x18 glyph at x 264, 8 rows into the card.
        c.icon(i, 272 - i.width() / 2, r.y + 17 - i.height() / 2, th.fg);
    }
    let f = th.font(Font::Normal);
    let tw = f.width(card.right);
    c.text(f, 286 - tw, r.y + 49, card.right, th.fg);
}

fn party_name(call: &Call) -> String {
    let d = call.remote.display();
    if d.is_empty() { tr::UNKNOWN_CALLER.to_string() } else { d.to_string() }
}

#[derive(Default)]
pub(crate) struct CallScreen {
    /// The call the softkeys act on (follows the focused call until the
    /// user moves it).
    sel: Option<CallId>,
    /// Touch: the dial pad is shown in place of the buttons, for DTMF.
    keypad: bool,
    /// Touch: the digits sent since the dial pad was shown.
    dtmf: String,
}

/// Touch: the regions of the call screen's own ([`Hit::Custom`]): the
/// button that shows and hides the dial pad, the two volume buttons, and,
/// from `HIT_CARD`, the card of the call with that id.
const HIT_KEYPAD: u32 = 1;
const HIT_VOLUME_DOWN: u32 = 2;
const HIT_VOLUME_UP: u32 = 3;
const HIT_CARD: u32 = 1000;

impl CallScreen {
    fn selected<'a>(&self, cx: &'a Cx) -> Option<&'a Call> {
        let calls = &cx.st.calls;
        self.sel
            .and_then(|id| calls.get(id))
            .filter(|c| !c.state.is_ended())
            .or_else(|| calls.ringing().next())
            .or_else(|| calls.focused().filter(|c| !c.state.is_ended()))
            .or_else(|| calls.live().next())
    }

    /// Another established call, for join and attended transfer.
    fn other<'a>(&self, cx: &'a Cx, this: CallId) -> Option<&'a Call> {
        cx.st.calls.live().find(|c| c.id != this && c.state.is_established())
    }

    fn step(&mut self, cx: &Cx, d: i32) {
        let live: Vec<CallId> = cx.st.calls.live().map(|c| c.id).collect();
        if live.is_empty() {
            return;
        }
        let cur = self.selected(cx).and_then(|c| live.iter().position(|i| *i == c.id)).unwrap_or(0) as i32;
        self.sel = Some(live[(cur + d).rem_euclid(live.len() as i32) as usize]);
    }

    fn answer_path(cx: &Cx) -> Option<AudioPath> {
        if cx.off_hook { Some(AudioPath::Handset) } else { None }
    }
}

impl Screen for CallScreen {
    fn name(&self) -> String {
        "call".into()
    }
    fn tag(&self) -> Tag {
        Tag::Call
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the screen has its own buttons for these.
        if cx.th.touch() {
            return Vec::new();
        }
        let Some(call) = self.selected(cx) else { return Vec::new() };
        let other = self.other(cx, call.id).is_some();
        match &call.state {
            // The sets and positions follow stock: End Call is always the
            // fourth key; Hold and Transfer also have hard keys; Cancel
            // declines a ringing call.
            CallState::Ringing => vec![soft(1, tr::ANSWER), soft(2, tr::IGNORE), soft(5, tr::TRANSFER), soft(0, "")],
            CallState::Dialing | CallState::Proceeding | CallState::Ringback { .. } | CallState::Transferring => {
                vec![soft(0, ""), soft(0, ""), soft(0, ""), soft(10, tr::END_CALL)]
            }
            CallState::Active if call.in_conference => {
                vec![soft(6, tr::HOLD), soft(13, tr::SPLIT), soft(0, ""), soft(10, tr::END_CALL)]
            }
            CallState::Active | CallState::Held { .. } if other => {
                vec![soft(12, tr::JOIN), soft(14, tr::SWAP), soft(8, tr::TRANSFER), soft(10, tr::END_CALL)]
            }
            CallState::Held { .. } => {
                vec![soft(7, tr::RESUME), soft(15, tr::NEW_CALL), soft(0, ""), soft(10, tr::END_CALL)]
            }
            CallState::Active => vec![soft(9, tr::CONFERENCE), soft(0, ""), soft(0, ""), soft(10, tr::END_CALL)],
            CallState::Ended(_) => Vec::new(),
        }
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        let Some(call) = self.selected(cx) else { return Nav::Stay };
        let cid = call.id;
        let account = Some(call.account);
        match id {
            1 => cx.post(Command::Answer { call: cid, path: Self::answer_path(cx) }),
            2 => cx.post(Command::Silence { call: cid }),
            3 => cx.post(Command::Reject { call: cid, busy: true }),
            4 => cx.post(Command::SendToVoicemail { call: cid }),
            5 | 8 => {
                // With a second call up, Transfer connects the two.
                if id == 8
                    && let Some(o) = self.other(cx, cid) {
                        cx.post(Command::TransferAttended { call: o.id, consult: cid });
                        return Nav::Stay;
                    }
                return Nav::Push(Box::new(Dialer::new(Purpose::Transfer(cid), account)));
            }
            6 => cx.post(Command::Hold { call: cid }),
            7 => cx.post(Command::Resume { call: cid }),
            9 | 15 => return Nav::Push(Box::new(Dialer::new(Purpose::Conference(cid), account))),
            10 => cx.post(Command::Hangup { call: cid }),
            11 => cx.post(Command::Park { call: cid }),
            12 => {
                if let Some(o) = self.other(cx, cid) {
                    cx.post(Command::Conference { a: cid, b: o.id });
                }
            }
            13 => cx.post(Command::SplitConference { keep: cid }),
            14 => {
                if let Some(o) = self.other(cx, cid) {
                    cx.post(Command::Focus { call: o.id });
                    self.sel = Some(o.id);
                }
            }
            _ => {}
        }
        Nav::Stay
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let Some(call) = self.selected(cx) else { return Nav::Ignored };
        let cid = call.id;
        match key {
            Key::Up => self.step(cx, -1),
            Key::Down => self.step(cx, 1),
            // Stock: on the call screen Select accepts and Cancel rejects.
            Key::Select => match call.state {
                CallState::Ringing => cx.post(Command::Answer { call: cid, path: Self::answer_path(cx) }),
                CallState::Held { local: true, .. } => cx.post(Command::Resume { call: cid }),
                _ => {}
            },
            Key::Cancel => match call.state {
                CallState::Ringing => cx.post(Command::Reject { call: cid, busy: true }),
                CallState::Dialing | CallState::Proceeding | CallState::Ringback { .. } => {
                    cx.post(Command::Hangup { call: cid })
                }
                _ => {}
            },
            Key::Digit(_) | Key::Star | Key::Pound if call.state.has_media() => {
                let digit = key.dial_char().unwrap_or('0');
                cx.post(Command::Dtmf { call: cid, digit });
                if self.keypad && self.dtmf.len() < 64 {
                    self.dtmf.push(digit);
                }
            }
            Key::Digit(_) | Key::Star | Key::Pound if call.state.is_held() => {
                let mut d = Dialer::new(Purpose::Conference(cid), Some(call.account));
                d.type_char(cx, key.dial_char().unwrap_or('0'));
                return Nav::Push(Box::new(d));
            }
            Key::Hold => match call.state {
                CallState::Active => cx.post(Command::Hold { call: cid }),
                CallState::Held { .. } => cx.post(Command::Resume { call: cid }),
                _ => {}
            },
            Key::Transfer if call.state.is_established() || call.state == CallState::Ringing => {
                return self.softkey(cx, 8);
            }
            Key::Conference if call.state.is_established() => {
                return self.softkey(cx, if self.other(cx, cid).is_some() { 12 } else { 9 });
            }
            _ => return Nav::Ignored,
        }
        Nav::Stay
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Custom(HIT_KEYPAD) => {
                self.keypad = !self.keypad;
                self.dtmf.clear();
                Nav::Stay
            }
            // What the volume keys do, without their popup: the level is
            // on the screen.
            Hit::Custom(n @ (HIT_VOLUME_DOWN | HIT_VOLUME_UP)) => {
                let delta = if n == HIT_VOLUME_UP { 1 } else { -1 };
                cx.post(Command::AdjustVolume { target: VolumeTarget::Current, delta });
                Nav::Stay
            }
            // A call's card: the buttons act on that call from now on.
            Hit::Custom(n) if n >= HIT_CARD => {
                let id = CallId(n - HIT_CARD);
                if cx.st.calls.live().any(|c| c.id == id) {
                    if self.sel != Some(id) {
                        self.dtmf.clear();
                    }
                    self.sel = Some(id);
                }
                Nav::Stay
            }
            _ => Nav::Ignored,
        }
    }
    fn wallpaper(&self) -> bool {
        true
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        if th.touch() {
            return self.draw_touch(cx, c, area);
        }
        let Some(sel) = self.selected(cx) else { return };
        let (label, _, _) = call_parts(sel, cx.now_ms);
        let mut flags: Vec<&str> = Vec::new();
        if cx.st.audio.muted || sel.muted {
            flags.push("mute");
        }
        if sel.srtp {
            flags.push("lock");
        }
        call_header(cx, c, &label, &flags);
        idle::draw_tabs(cx, c, th.tab_x);
        // One card per call, the selected one first (stock shows the call
        // card directly under the state band).
        let mut calls: Vec<&Call> = vec![sel];
        calls.extend(cx.st.calls.live().filter(|c| c.id != sel.id));
        let fit = max_cards(th);
        let many = calls.len() > 1;
        for (i, call) in calls.iter().take(fit).enumerate() {
            let (_, icon, timer) = call_parts(call, cx.now_ms);
            let name = party_name(call);
            // Stock: the number on the first line, the name in bold on
            // the second; a call with no name shows the number alone.
            let number = if call.remote.number.is_empty() { name.clone() } else { call.remote.number.clone() };
            let second = if name == number { String::new() } else { name };
            let fill = match call.state {
                CallState::Ringing => th.card_ringing,
                CallState::Held { .. } => th.card_held,
                _ => th.card,
            };
            let card = Card {
                fill,
                line1: &number,
                line2: &second,
                line2_bold: true,
                icon,
                right: timer.as_deref().unwrap_or(""),
                cursor: false,
                marked: many && i == 0,
            };
            call_card(cx, c, i, &card);
        }
        if calls.len() > fit {
            let r = th.header_rect();
            let f = th.font(Font::Small);
            let s = format!("+{}", calls.len() - fit);
            let w = f.width(&s);
            c.text_in(f, IRect::new(r.right() - w - th.pad, r.y, w, r.h), &s, th.title_fg, HAlign::Left);
        }
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        let timer = cx.st.calls.live().any(|c| c.connected_mono_ms.is_some()).then(|| cx.now_ms + 1000 - cx.now_ms % 1000);
        // Touch: and when a line's lamp next blinks.
        let lamps = if cx.th.touch() { idle::lamps_deadline(&idle::lines(cx), cx.now_ms) } else { None };
        match (timer, lamps) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Touch: the number and the name a call's card shows (stock: the number
/// on the first line, the name on the second; a call with no name shows
/// the number alone).
fn card_lines(call: &Call) -> (String, String) {
    let name = party_name(call);
    let number = if call.remote.number.is_empty() { name.clone() } else { call.remote.number.clone() };
    let second = if name == number { String::new() } else { name };
    (number, second)
}

/// Touch: the card of `call` in `r`: its icon, number, and name, and at
/// the right its state and timer.  `marked` outlines it as the call the
/// buttons act on.
fn touch_card(cx: &Cx, c: &mut Canvas, r: IRect, call: &Call, marked: bool) {
    let th = cx.th;
    let fill = match call.state {
        CallState::Ringing => th.card_ringing,
        CallState::Held { .. } => th.card_held,
        _ => th.card,
    };
    let rad = th.px(6);
    if marked {
        c.fill_round_aa(r, rad, th.fg);
        c.fill_round_aa(r.inset(th.px(2)), rad - th.px(2), fill);
    } else {
        c.fill_round_aa(r, rad, fill);
    }
    let (label, icon, timer) = call_parts(call, cx.now_ms);
    let (ic, rest) = r.inset_xy(th.pad, th.px(6)).split_left(th.px(32));
    if let Some(i) = th.icon(icon) {
        c.icon_in(i, ic, th.fg);
    }
    // At the right: the state over the timer.
    let small = th.font(Font::Small);
    let side_w = small.width(&label).max(th.number_font().width(timer.as_deref().unwrap_or(""))).min(rest.w / 2);
    let (text, side) = rest.split_right(side_w);
    let text = IRect { x: text.x + th.px(4), w: text.w - th.pad - th.px(4), ..text };
    match &timer {
        Some(t) => {
            let (top, bottom) = side.split_top(side.h / 2);
            c.text_in(small, top, &label, th.fg, HAlign::Right);
            c.text_in(th.number_font(), bottom, t, th.fg, HAlign::Right);
        }
        None => c.text_in(small, side, &label, th.fg, HAlign::Right),
    }
    let (number, name) = card_lines(call);
    let big = if th.font(Font::Large).width(&number) <= text.w { th.font(Font::Large) } else { th.number_font() };
    if name.is_empty() {
        c.text_in(big, text, &number, th.fg, HAlign::Left);
    } else {
        let bold = th.font(Font::Bold);
        let block = big.height() + bold.height();
        let top = text.y + (text.h - block) / 2;
        c.text_in(big, IRect::new(text.x, top, text.w, big.height()), &number, th.fg, HAlign::Left);
        c.text_in(bold, IRect::new(text.x, top + big.height(), text.w, bold.height()), &name, th.fg, HAlign::Left);
    }
}

impl CallScreen {
    /// The touch skin's call screen.  From the top: the state of the
    /// selected call, the lines, and the calls' cards; from the bottom:
    /// the buttons that answer or end, and above them the buttons for
    /// what the keys of a phone that has keys do during a call, or, when
    /// it is asked for, the dial pad.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let st = cx.st;
        let m = dialpad::MARGIN;
        let Some(sel) = self.selected(cx) else { return };
        let (label, _, _) = call_parts(sel, cx.now_ms);
        let mut flags: Vec<&str> = Vec::new();
        if st.audio.muted || sel.muted {
            flags.push("mute");
        }
        if sel.srtp {
            flags.push("lock");
        }
        let (header, body) = area.split_top(th.header_h);
        call_header_in(cx, c, header, &label, &flags);
        let inner = IRect::new(body.x + m, body.y + th.px(4), body.w - 2 * m, body.h - th.px(4) - m);
        let gap = th.pad;
        let state = &sel.state;
        let held = state.is_held();
        let ringing = *state == CallState::Ringing;
        let established = state.is_established();
        let can_type = state.has_media() || held;
        let keypad = self.keypad && can_type;

        // The lines: what a line key does during a call (to the call on
        // it, a new call on a free line, a watched contact) is a touch.
        let mut y = inner.y;
        let lines = idle::lines(cx);
        if !lines.is_empty() {
            idle::lines_strip(cx, c, IRect::new(inner.x, y, inner.w, th.touch_min), &lines);
            y += th.touch_min + th.px(6);
        }

        // One card per call, the selected one first; with the dial pad
        // there is room for that one only.
        let mut calls: Vec<&Call> = vec![sel];
        calls.extend(st.calls.live().filter(|c| c.id != sel.id));
        let fit = if keypad { 1 } else { 3 };
        let many = calls.len() > 1;
        for call in calls.iter().take(fit) {
            let r = IRect::new(inner.x, y, inner.w, th.px(80));
            touch_card(cx, c, r, call, many && call.id == sel.id);
            c.hit(r, Hit::Custom(HIT_CARD.saturating_add(call.id.0)));
            y = r.bottom() + th.px(6);
        }
        if calls.len() > fit {
            let f = th.font(Font::Small);
            let s = format!("+{}", calls.len() - fit);
            let w = f.width(&s);
            let x = header.right() - m - w - flags.len() as i32 * (th.icon + th.pad);
            c.text_in(f, IRect::new(x, header.y, w, header.h), &s, th.title_fg, HAlign::Left);
        }

        // The bottom row: a ringing call is answered, silenced, or
        // refused; any other is ended.
        let bottom = IRect::new(inner.x, inner.bottom() - th.px(60), inner.w, th.px(60));
        if keypad {
            // What was sent, the pad, and the way back to the buttons.
            let sent = IRect::new(inner.x, y, inner.w, th.px(32));
            let f = th.number_font();
            let mut shown: &str = &self.dtmf;
            while f.width(shown) > sent.w && !shown.is_empty() {
                shown = &shown[1..];
            }
            c.text_in(f, sent, shown, th.wallpaper_fg, HAlign::Center);
            let space = IRect::new(inner.x, sent.bottom(), inner.w, bottom.y - gap - sent.bottom());
            let key_h = ((space.h - 3 * th.pad) / 4).min(th.px(84));
            let h = dialpad::pad_h(th, key_h);
            dialpad::pad(c, th, IRect::new(space.x, space.y + (space.h - h) / 2, space.w, h));
            let cols = dialpad::columns(bottom, 2, gap);
            dialpad::control(c, th, cols[0], dialpad::KEYPAD, tr::HIDE_KEYPAD, Look::Plain, Hit::Custom(HIT_KEYPAD));
            dialpad::control(c, th, cols[1], "phone", tr::END_CALL, Look::Stop, Hit::Soft(10));
            return;
        }
        if ringing {
            let cols = dialpad::columns(bottom, 3, gap);
            dialpad::control(c, th, cols[0], "phone", tr::ANSWER, Look::Go, Hit::Soft(1));
            dialpad::control(c, th, cols[1], "", tr::IGNORE, Look::Plain, Hit::Soft(2));
            dialpad::control(c, th, cols[2], "", tr::REJECT, Look::Stop, Hit::Soft(3));
        } else {
            dialpad::control(c, th, bottom, "phone", tr::END_CALL, Look::Stop, Hit::Soft(10));
        }

        // The buttons, four to a row.  The audio buttons show what the
        // lamps of the keys show.
        let (speaker, headset, _) = opendeskphone_state::audio_lamps(&st.audio, &st.calls);
        let look = |on: bool, enabled: bool| match (enabled, on) {
            (false, _) => Look::Off,
            (true, true) => Look::On,
            (true, false) => Look::Plain,
        };
        let other = self.other(cx, sel.id).is_some();
        let (cell_h, rows) = (th.px(58), 2);
        let grid = IRect::new(inner.x, bottom.y - gap - rows * cell_h - (rows - 1) * gap, inner.w, rows * cell_h + (rows - 1) * gap);
        let cells: Vec<IRect> = (0..rows)
            .flat_map(|row| dialpad::columns(IRect::new(grid.x, grid.y + row * (cell_h + gap), grid.w, cell_h), 4, gap))
            .collect();
        dialpad::control(c, th, cells[0], "mute", tr::MUTE, look(st.audio.muted || sel.muted, true), Hit::Key(Key::Mute));
        dialpad::control(c, th, cells[1], dialpad::KEYPAD, tr::KEYPAD, look(false, can_type), Hit::Custom(HIT_KEYPAD));
        let on = speaker.color != LampColor::Off;
        dialpad::control(c, th, cells[2], "speaker", tr::VOL_SPEAKER, look(on, true), Hit::Key(Key::Speaker));
        let on = headset.color != LampColor::Off;
        dialpad::control(c, th, cells[3], "headset", tr::VOL_HEADSET, look(on, true), Hit::Key(Key::Headset));
        let (label, can_hold) = (if held { tr::RESUME } else { tr::HOLD }, held || *state == CallState::Active);
        dialpad::control(c, th, cells[4], "hold", label, look(held, can_hold), Hit::Key(Key::Hold));
        if other {
            dialpad::control(c, th, cells[5], "", tr::SWAP, Look::Plain, Hit::Soft(14));
        } else {
            dialpad::control(c, th, cells[5], "phone-out", tr::NEW_CALL, look(false, established), Hit::Soft(15));
        }
        dialpad::control(c, th, cells[6], "transfer", tr::TRANSFER, look(false, established || ringing), Hit::Key(Key::Transfer));
        if sel.in_conference && *state == CallState::Active {
            dialpad::control(c, th, cells[7], "conference", tr::SPLIT, Look::Plain, Hit::Soft(13));
        } else {
            let label = if other { tr::JOIN } else { tr::CONFERENCE };
            dialpad::control(c, th, cells[7], "conference", label, look(false, established), Hit::Key(Key::Conference));
        }

        // The volume of what is sounding (the ringer, for a ringing call):
        // its level between the buttons that lower and raise it.
        let row = IRect::new(inner.x, grid.y - gap - th.touch_min, inner.w, th.touch_min);
        let (minus, rest) = row.split_left(th.px(52));
        let (rest, plus) = rest.split_right(th.px(52));
        dialpad::control(c, th, minus, "", "\u{2013}", Look::Plain, Hit::Custom(HIT_VOLUME_DOWN));
        dialpad::control(c, th, plus, "", "+", Look::Plain, Hit::Custom(HIT_VOLUME_UP));
        let icon = match st.audio.current_target() {
            VolumeTarget::Handset => "handset",
            VolumeTarget::Headset => "headset",
            VolumeTarget::Speaker => "speaker",
            VolumeTarget::Ringer | VolumeTarget::Current => "ring",
        };
        let (ic, meter) = rest.inset_xy(gap, 0).split_left(th.icon + gap);
        if let Some(i) = th.icon(icon) {
            c.icon_in(i, IRect { w: th.icon, ..ic }, th.wallpaper_fg);
        }
        let h = th.px(18);
        let level = st.audio.volume(VolumeTarget::Current) as u32;
        widgets::meter(c, th, IRect::new(meter.x, meter.y + (meter.h - h) / 2, meter.w, h), level, VOLUME_MAX as u32);
    }
}
