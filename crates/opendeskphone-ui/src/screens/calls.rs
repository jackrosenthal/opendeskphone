//! The dialer and the call screen.

use super::*;
use crate::gfx::Rgb;
use opendeskphone_state::{AccountId, AudioPath, Call, CallId, CallState, Direction, EndCause, Tone};

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
    fn softkeys(&self, _cx: &Cx) -> Vec<Soft> {
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
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, _area: IRect) {
        let th = cx.th;
        let label = match self.purpose {
            Purpose::Transfer(_) => tr::TRANSFER_TO,
            Purpose::Conference(_) => tr::CONFERENCE_WITH,
            _ => tr::NEW_CALL_TITLE,
        };
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

impl Dialer {
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
    let th = cx.th;
    let r = th.header_rect();
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
}

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
                cx.post(Command::Dtmf { call: cid, digit: key.dial_char().unwrap_or('0') });
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
    fn wallpaper(&self) -> bool {
        true
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, _area: IRect) {
        let th = cx.th;
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
        cx.st.calls.live().any(|c| c.connected_mono_ms.is_some()).then(|| cx.now_ms + 1000 - cx.now_ms % 1000)
    }
}
