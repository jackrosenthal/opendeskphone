//! The dialer and the call screen.
//!
//! On the touch skin (D80) both are made of buttons ([`dialpad`]), and
//! have no softkeys.  The dialer has a dial pad whose keys are the digit
//! keys, the number with a button that deletes, the accounts to choose
//! from, and the buttons that call.  The call screen has a card for each
//! call, in the calls' order (a touch goes to that call), and a button
//! for each thing a key does during a call on a phone that has keys:
//! where there is such a key the button is that key ([`Hit::Key`]), so
//! that what it does is in one place.  (The volume is the exception: the
//! screen shows the level between its two buttons, where the keys raise
//! a popup.)  The buttons act on the call that is heard; a call that
//! rings has its own, on its card, or, when it is the only call, a
//! screen of its own.

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
        // A phone with no account calls nobody, and says why.
        if self.purpose == Purpose::Call && no_account_notice(cx) {
            self.close_live(cx);
            return Nav::Pop;
        }
        if target.is_empty() {
            // Touch: the button is one only while there is a number to
            // call again.
            if self.purpose == Purpose::Call && !(cx.th.touch() && last_placed(cx).is_none()) {
                self.stop_tone(cx);
                self.live = None;
                redial(cx);
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
            // A recent number: entered, for the user to say how it is
            // transferred to.
            Hit::Custom(n) if (HIT_RECENT..HIT_ACCOUNT).contains(&n) => {
                if let Some((_, number)) = self.recent(cx).into_iter().nth((n - HIT_RECENT) as usize) {
                    self.text = number;
                    self.last_key_ms = cx.now_ms;
                }
                Nav::Stay
            }
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
        let label = self.title(cx);
        if th.touch() {
            return self.draw_touch(cx, c, area, &label);
        }
        call_header(cx, c, &label, &[]);
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
/// button, from `HIT_RECENT` the recent numbers in their order, and, from
/// `HIT_ACCOUNT`, the accounts in their order.
const HIT_DELETE: u32 = 1;
const HIT_RECENT: u32 = 50;
const HIT_ACCOUNT: u32 = 100;

/// The newest call the user placed, from the call log.
pub(crate) fn last_placed<'a>(cx: &'a Cx) -> Option<&'a opendeskphone_state::CallLogEntry> {
    let placed = |e: &&opendeskphone_state::CallLogEntry| e.kind == opendeskphone_state::CallLogKind::Placed && !e.party.number.is_empty();
    cx.st.call_log.entries.iter().find(placed)
}

/// Says that the phone has no account to call with, if it has none.
/// Returns whether it said so (and nothing can be dialed).
pub(crate) fn no_account_notice(cx: &Cx) -> bool {
    if !cx.st.accounts.accounts.is_empty() {
        return false;
    }
    cx.post(Command::Notify {
        level: opendeskphone_state::NoticeLevel::Warning,
        title: tr::NO_PHONE_ACCOUNT.into(),
        text: tr::NO_ACCOUNT_TEXT.into(),
        timeout_ms: 6000,
    });
    true
}

/// Calls the last number again.  Touch: the newest call placed of the
/// call log, which is what the screens show there is to redial (the call
/// core's own memory of it does not outlast a restart).
pub(crate) fn redial(cx: &Cx) {
    if !cx.th.touch() {
        return cx.post(Command::Redial);
    }
    if no_account_notice(cx) {
        return;
    }
    if let Some(e) = last_placed(cx) {
        let account = cx.st.accounts.get(e.account).map(|a| a.id);
        cx.post(Command::Dial { target: e.party.number.clone(), account, path: None });
    }
}

impl Dialer {
    /// The title: what the number entered is for.  A transfer says whom
    /// it transfers.
    fn title(&self, cx: &Cx) -> String {
        match self.purpose {
            Purpose::Transfer(id) if cx.th.touch() => match cx.st.calls.get(id) {
                Some(call) => format!("{} {} {}", tr::TRANSFER, party_name_or_number(call), tr::TRANSFER_WHOM_TO),
                None => tr::TRANSFER_TO.to_string(),
            },
            Purpose::Transfer(_) => tr::TRANSFER_TO.to_string(),
            Purpose::Conference(_) => tr::CONFERENCE_WITH.to_string(),
            Purpose::Call => tr::NEW_CALL_TITLE.to_string(),
        }
    }

    /// Touch: the numbers a transfer is likely to go to: the newest of
    /// the call log, each once, without the party being transferred.  The
    /// name to show (the number, where there is none) and the number.
    fn recent(&self, cx: &Cx) -> Vec<(String, String)> {
        let Purpose::Transfer(id) = self.purpose else { return Vec::new() };
        let own = cx.st.calls.get(id).map(|c| c.remote.number.as_str()).unwrap_or("");
        let mut v: Vec<(String, String)> = Vec::new();
        for e in &cx.st.call_log.entries {
            let number = &e.party.number;
            if number.is_empty() || number == own || v.iter().any(|(_, n)| n == number) {
                continue;
            }
            let name = e.party.display();
            v.push((if name.is_empty() { number.clone() } else { name.to_string() }, number.clone()));
            if v.len() == 3 {
                break;
            }
        }
        v
    }

    /// The touch skin's dialer: under the title bar (whose back button
    /// leaves), the accounts when there is more than one to choose from,
    /// the number with the way to the contacts beside it, for a transfer
    /// the recent numbers, the dial pad, and the buttons that call.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect, label: &str) {
        let th = cx.th;
        let m = dialpad::MARGIN;
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, label);
        let inner = IRect::new(body.x + m, body.y + th.pad, body.w - 2 * m, body.h - th.pad - m);
        let transfer = matches!(self.purpose, Purpose::Transfer(_));

        // The account the call goes out on, when there is one to choose
        // (a transfer goes out on the account of the call it transfers).
        let accounts = &cx.st.accounts.accounts;
        let chosen = self
            .account
            .or(cx.st.accounts.default_account)
            .filter(|id| cx.st.accounts.get(*id).is_some())
            .or(accounts.first().map(|a| a.id));
        let mut y = inner.y;
        if accounts.len() > 1 && !transfer {
            let row = IRect::new(inner.x, y, inner.w, th.touch_min);
            for (i, (a, r)) in accounts.iter().zip(dialpad::columns(row, accounts.len(), th.px(6))).enumerate() {
                let label = if a.label.is_empty() { &a.user } else { &a.label };
                let look = if Some(a.id) == chosen { Look::On } else { Look::Plain };
                dialpad::control(c, th, r, "", label, look, Hit::Custom(HIT_ACCOUNT + i as u32));
            }
            y = row.bottom() + th.pad;
        }

        // The number, as on a call card, with the delete button at its
        // right; beside it, the contacts.
        let row = IRect::new(inner.x, y, inner.w, th.px(64));
        let (field, contacts) = row.split_right(th.px(76));
        let field = IRect { w: field.w - th.pad, ..field };
        dialpad::control(c, th, contacts, "contacts", tr::CONTACTS, Look::Plain, Hit::Soft(5));
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
        let mut top = row.bottom() + th.pad;

        // A transfer: the recent numbers; a touch enters one.
        let recent = self.recent(cx);
        if !recent.is_empty() {
            let row = IRect::new(inner.x, top, inner.w, th.touch_min);
            for (i, ((name, _), r)) in recent.iter().zip(dialpad::columns(row, recent.len(), th.px(6))).enumerate() {
                dialpad::control(c, th, r, "", name, Look::Plain, Hit::Custom(HIT_RECENT + i as u32));
            }
            top = row.bottom() + th.pad;
        }

        // The buttons that call, at the bottom, and the pad between.
        let actions = IRect::new(inner.x, inner.bottom() - th.px(56), inner.w, th.px(56));
        let gap = th.pad;
        let empty = self.text.is_empty();
        let go = |on: bool| if on { Look::Go } else { Look::Off };
        match self.purpose {
            // Now, unannounced; or after the user has spoken to whoever
            // answers there.
            Purpose::Transfer(_) => {
                let cols = dialpad::columns(actions, 2, gap);
                let look = if empty { Look::Off } else { Look::Plain };
                dialpad::control(c, th, cols[0], "phone-out", tr::ASK_FIRST, look, Hit::Soft(2));
                dialpad::control(c, th, cols[1], "transfer", tr::TRANSFER_NOW, go(!empty), Hit::Soft(1));
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
                // With no number, the last one again, if there is one.
                let (label, look) = match (empty, last_placed(cx).is_some()) {
                    (true, true) => (tr::REDIAL, Look::Go),
                    (true, false) => (tr::DIAL, Look::Off),
                    (false, _) => (tr::DIAL, Look::Go),
                };
                dialpad::control(c, th, call, "phone", label, look, Hit::Soft(1));
            }
        }
        let space = IRect::new(inner.x, top, inner.w, actions.y - gap - top);
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
    /// user moves it).  Touch: there is no such choice; the buttons act
    /// on the call that is heard ([`CallScreen::heard`]).
    sel: Option<CallId>,
    /// Touch: the dial pad is shown in place of the buttons, for DTMF.
    keypad: bool,
    /// Touch: the digits sent since the dial pad was shown.
    dtmf: String,
    /// Touch: the ringing calls whose ringing the user silenced.
    silenced: Vec<CallId>,
    /// Touch: the call whose transfer was begun here (its dialer was
    /// opened): with a second call up, the screen offers to complete it.
    transfer: Option<CallId>,
    /// Touch: the microphone was muted when the screen was last drawn;
    /// when the Mute key was last pressed; and since when the screen says
    /// that the phone, not the key, took the mute off.
    muted: bool,
    mute_key_ms: Option<u64>,
    unmuted_ms: Option<u64>,
}

/// Touch: how long the call screen says that the microphone is on again,
/// in milliseconds.
const MIC_NOTE_MS: u64 = 5000;

/// Touch: the regions of the call screen's own ([`Hit::Custom`]): the
/// button that shows and hides the dial pad, the two volume buttons, and
/// the button that connects the two calls.  The regions of one call are
/// the call's id with what they do in the top byte: its card, and the
/// buttons that end it, answer it, silence its ringing, decline it, send
/// it to another number, and send it to voicemail.
const HIT_KEYPAD: u32 = 1;
const HIT_VOLUME_DOWN: u32 = 2;
const HIT_VOLUME_UP: u32 = 3;
const HIT_COMPLETE: u32 = 4;
const HIT_CARD: u32 = 0x0100_0000;
const HIT_END: u32 = 0x0200_0000;
const HIT_ANSWER: u32 = 0x0300_0000;
const HIT_SILENCE: u32 = 0x0400_0000;
const HIT_DECLINE: u32 = 0x0500_0000;
const HIT_SEND_TO: u32 = 0x0600_0000;
const HIT_TO_VOICEMAIL: u32 = 0x0700_0000;
const HIT_CALL: u32 = 0x00FF_FFFF;

impl CallScreen {
    /// Touch: the call that is heard, which the buttons act on: the
    /// focused call, or else one that is connected, or else any; a call
    /// that rings only when there is no other.
    fn heard<'a>(&self, cx: &'a Cx) -> Option<&'a Call> {
        let calls = &cx.st.calls;
        let answered = |c: &&Call| !c.state.is_ended() && c.state != CallState::Ringing;
        calls
            .focused()
            .filter(answered)
            .or_else(|| calls.live().find(|c| c.state == CallState::Active))
            .or_else(|| calls.live().find(answered))
            .or_else(|| calls.ringing().next())
    }

    fn selected<'a>(&self, cx: &'a Cx) -> Option<&'a Call> {
        if cx.th.touch() {
            return self.heard(cx);
        }
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
                self.transfer = Some(cid);
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
        // (The phone mutes; the screen notes that the key did it.)
        if key == Key::Mute {
            self.mute_key_ms = Some(cx.now_ms);
            return Nav::Ignored;
        }
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
            // The two calls, connected: the other one to the party of
            // the one that is heard.
            Hit::Custom(HIT_COMPLETE) => {
                if let Some(sel) = self.heard(cx)
                    && let Some(o) = self.other(cx, sel.id)
                {
                    cx.post(Command::TransferAttended { call: o.id, consult: sel.id });
                }
                Nav::Stay
            }
            // One call's regions.
            Hit::Custom(n) if n >= HIT_CARD => {
                let id = CallId(n & HIT_CALL);
                let Some(call) = cx.st.calls.live().find(|c| c.id == id) else { return Nav::Stay };
                match n & !HIT_CALL {
                    // Its card: the call to hear from now on.  One on
                    // hold is taken back, which holds the others.
                    HIT_CARD => {
                        match call.state {
                            CallState::Ringing => {}
                            CallState::Held { .. } => cx.post(Command::Resume { call: id }),
                            _ => cx.post(Command::Focus { call: id }),
                        }
                        self.dtmf.clear();
                    }
                    HIT_END => cx.post(Command::Hangup { call: id }),
                    HIT_ANSWER => cx.post(Command::Answer { call: id, path: Self::answer_path(cx) }),
                    HIT_SILENCE => {
                        cx.post(Command::Silence { call: id });
                        if !self.silenced.contains(&id) {
                            self.silenced.push(id);
                        }
                    }
                    HIT_DECLINE => cx.post(Command::Reject { call: id, busy: true }),
                    HIT_SEND_TO => return Nav::Push(Box::new(Dialer::new(Purpose::Transfer(id), Some(call.account)))),
                    HIT_TO_VOICEMAIL => cx.post(Command::SendToVoicemail { call: id }),
                    _ => {}
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
        // Touch: and when the note that the microphone is on again goes.
        let note = self.unmuted_ms.map(|at| at + MIC_NOTE_MS);
        match (timer, note) {
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

/// The name of a call's other party, or the number where there is none.
fn party_name_or_number(call: &Call) -> String {
    let (number, name) = card_lines(call);
    if name.is_empty() { number } else { name }
}

/// Touch: the fill of a call's card.
fn card_fill(th: &Theme, call: &Call) -> Rgb {
    match call.state {
        CallState::Ringing => th.card_ringing,
        CallState::Held { .. } => th.card_held,
        _ => th.card,
    }
}

/// Touch: the largest of the fonts for names in which `s` is no wider
/// than `w` (the smallest, if it fits in none).
fn name_font(th: &Theme, s: &str, w: i32) -> &'static crate::font::FontData {
    let fonts = [th.font(Font::Large), th.date_font(), th.number_font()];
    fonts.into_iter().find(|f| f.width(s) <= w).unwrap_or(th.font(Font::Normal))
}

/// Touch: a mark in `r` that the microphone is muted: the warning color,
/// with the icon and the word.
fn muted_mark(c: &mut Canvas, th: &Theme, r: IRect) {
    let fg = idle::on_warn(th);
    c.fill_round_aa(r, r.h / 2, th.warn);
    let f = th.font(Font::Bold);
    let icon = th.icon("mute");
    let iw = icon.map_or(0, |i| i.width() + th.px(4));
    let x = r.x + (r.w - iw - f.width(tr::MUTED)) / 2;
    if let Some(i) = icon {
        c.icon(i, x, r.y + (r.h - i.height()) / 2, fg);
    }
    c.text_in(f, IRect::new(x + iw, r.y, r.right() - x - iw, r.h), tr::MUTED, fg, HAlign::Left);
}

/// Touch: what the card of the call that is heard says under its state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MicNote {
    None,
    /// The microphone is muted.
    Muted,
    /// The phone took the mute off (it does when a call is put on hold or
    /// taken back, as the stock firmware does).
    OnAgain,
}

impl CallScreen {
    /// Touch: the card of the one call there is, in `r`, with the room
    /// that leaves: the name (or the number) large, the number under it,
    /// and below them the state and a large timer.
    fn big_card(&self, cx: &Cx, c: &mut Canvas, r: IRect, call: &Call, mic: MicNote) {
        let th = cx.th;
        c.fill_round_aa(r, th.px(6), card_fill(th, call));
        let (label, _, timer) = call_parts(call, cx.now_ms);
        let (number, name) = card_lines(call);
        let inner = r.inset_xy(th.title_pad, th.title_pad);
        let first = if name.is_empty() { &number } else { &name };
        let big = name_font(th, first, inner.w);
        let sub = th.date_font();
        let state = th.font(Font::Normal);
        let large = th.font(Font::Large);
        let mark_h = th.px(26);
        // The block, in the middle of the card.
        let mut block = big.height();
        if !name.is_empty() {
            block += sub.height();
        }
        block += th.pad + state.height();
        if timer.is_some() {
            block += large.height();
        }
        if mic != MicNote::None {
            block += th.pad + mark_h;
        }
        let mut y = inner.y + ((inner.h - block) / 2).max(0);
        let line = |c: &mut Canvas, y: &mut i32, f: &'static crate::font::FontData, s: &str| {
            c.text_in(f, IRect::new(inner.x, *y, inner.w, f.height()), s, th.fg, HAlign::Center);
            *y += f.height();
        };
        line(c, &mut y, big, first);
        if !name.is_empty() {
            line(c, &mut y, sub, &number);
        }
        y += th.pad;
        line(c, &mut y, state, &label);
        if let Some(t) = &timer {
            line(c, &mut y, large, t);
        }
        y += th.pad;
        match mic {
            MicNote::None => {}
            MicNote::Muted => {
                let w = th.font(Font::Bold).width(tr::MUTED) + th.icon + 4 * th.pad;
                muted_mark(c, th, IRect::new(inner.x + (inner.w - w) / 2, y, w, mark_h));
            }
            MicNote::OnAgain => c.text_in(state, IRect::new(inner.x, y, inner.w, mark_h), tr::MIC_ON_AGAIN, th.fg, HAlign::Center),
        }
    }

    /// Touch: the card of one of several calls, in `r`: its icon, the
    /// number and the name, at the right its state and timer, and, with
    /// `end`, the button that ends that call.  `heard` outlines it as the
    /// call that is heard, the one the buttons below act on.  A touch on
    /// the card goes to that call (it is taken back from hold, and the
    /// others are held).
    #[allow(clippy::too_many_arguments)]
    fn card(&self, cx: &Cx, c: &mut Canvas, r: IRect, call: &Call, heard: bool, end: bool, mic: MicNote) {
        let th = cx.th;
        let fill = card_fill(th, call);
        let rad = th.px(6);
        if heard {
            c.fill_round_aa(r, rad, th.fg);
            c.fill_round_aa(r.inset(th.px(2)), rad - th.px(2), fill);
        } else {
            c.fill_round_aa(r, rad, fill);
        }
        let (label, icon, timer) = call_parts(call, cx.now_ms);
        let silenced = self.silenced.contains(&call.id);
        let label = if silenced { tr::RINGER_SILENCED.to_string() } else { label };
        let mut body = r.inset_xy(th.pad, th.px(6));
        // (The card is touched beside its button.)
        let mut touched = r;
        if end {
            let (rest, button) = body.split_right(th.px(60));
            dialpad::control(c, th, button, "", tr::END, Look::Plain, Hit::Custom(HIT_END | call.id.0));
            body = IRect { w: rest.w - th.pad, ..rest };
            touched.w = button.x - th.pad / 2 - r.x;
        }
        if call.state != CallState::Ringing {
            c.hit(touched, Hit::Custom(HIT_CARD | call.id.0));
        }
        let (ic, rest) = body.split_left(th.px(32));
        if let Some(i) = th.icon(icon) {
            c.icon_in(i, ic, th.fg);
        }
        // At the right: the state over the timer, or, with the timer
        // beside it, over the mark that the microphone is muted.
        let label = match (mic, &timer) {
            (MicNote::Muted | MicNote::OnAgain, Some(t)) => format!("{label}  {t}"),
            _ => label,
        };
        let small = th.font(Font::Small);
        let mark_w = th.font(Font::Bold).width(tr::MUTED) + th.icon + 3 * th.pad;
        let below = match (mic, &timer) {
            (MicNote::Muted, _) => mark_w,
            (MicNote::OnAgain, _) => small.width(tr::MIC_ON_AGAIN),
            (MicNote::None, Some(t)) => th.number_font().width(t),
            (MicNote::None, None) => 0,
        };
        let side_w = small.width(&label).max(below).min(rest.w * 3 / 5);
        let (text, side) = rest.split_right(side_w);
        let text = IRect { x: text.x + th.px(4), w: text.w - th.pad - th.px(4), ..text };
        let (top, bottom) = side.split_top(side.h / 2);
        match (mic, &timer) {
            (MicNote::Muted, _) => {
                c.text_in(small, top, &label, th.fg, HAlign::Right);
                let h = th.px(24);
                muted_mark(c, th, IRect::new(bottom.right() - mark_w.min(bottom.w), bottom.y + (bottom.h - h) / 2, mark_w.min(bottom.w), h));
            }
            (MicNote::OnAgain, _) => {
                c.text_in(small, top, &label, th.fg, HAlign::Right);
                c.text_in(small, bottom, tr::MIC_ON_AGAIN, th.fg, HAlign::Right);
            }
            (MicNote::None, Some(t)) => {
                c.text_in(small, top, &label, th.fg, HAlign::Right);
                c.text_in(th.number_font(), bottom, t, th.fg, HAlign::Right);
            }
            (MicNote::None, None) => c.text_in(small, side, &label, th.fg, HAlign::Right),
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

    /// Touch: the card of a call that rings while there are others, in
    /// `r`: the card, and under it, on the card, the buttons that silence
    /// its ringing, decline it, and answer it (which holds the others).
    fn waiting_card(&self, cx: &Cx, c: &mut Canvas, r: IRect, call: &Call) {
        let th = cx.th;
        c.fill_round_aa(r, th.px(6), card_fill(th, call));
        let (top, buttons) = r.split_bottom(th.touch_min + th.pad);
        self.card(cx, c, top, call, false, false, MicNote::None);
        let row = IRect::new(buttons.x + th.pad, buttons.y, buttons.w - 2 * th.pad, th.touch_min);
        let id = call.id.0;
        let silenced = self.silenced.contains(&call.id);
        let cols = dialpad::columns(row, if silenced { 2 } else { 3 }, th.pad);
        let mut cols = cols.into_iter();
        if !silenced {
            dialpad::control(c, th, cols.next().unwrap_or_default(), "", tr::SILENCE, Look::Plain, Hit::Custom(HIT_SILENCE | id));
        }
        dialpad::control(c, th, cols.next().unwrap_or_default(), "", tr::DECLINE, Look::Stop, Hit::Custom(HIT_DECLINE | id));
        dialpad::control(c, th, cols.next().unwrap_or_default(), "phone", tr::ANSWER, Look::Go, Hit::Custom(HIT_ANSWER | id));
    }

    /// Touch: the height of a call's card among several, and of one that
    /// rings while there are others.
    fn card_h(th: &Theme, call: &Call) -> i32 {
        if call.state == CallState::Ringing { th.px(72) + th.touch_min + th.pad } else { th.px(72) }
    }

    /// Touch: the cards of `calls` from `y` down, as many as end above
    /// `bottom`.  The calls keep their order; where there is no room for
    /// all, the ones left out are those that neither ring nor are heard,
    /// the last first, and the header says how many (`+2`).  Returns
    /// where the cards end, and how many calls were left out.
    #[allow(clippy::too_many_arguments)]
    fn cards(&self, cx: &Cx, c: &mut Canvas, x: i32, w: i32, y: i32, bottom: i32, calls: &[&Call], heard: &Call, mic: MicNote) -> (i32, usize) {
        let th = cx.th;
        let gap = th.px(6);
        let mut shown: Vec<&Call> = calls.to_vec();
        let height = |shown: &[&Call]| shown.iter().map(|call| Self::card_h(th, call) + gap).sum::<i32>() - gap;
        while y + height(&shown) > bottom {
            let Some(i) = shown.iter().rposition(|call| call.state != CallState::Ringing && call.id != heard.id) else { break };
            shown.remove(i);
        }
        // Each call has its own button that ends it, so that the large
        // one below is always for the call that is heard.
        let end = calls.len() > 1;
        let mut y = y;
        for call in &shown {
            let r = IRect::new(x, y, w, Self::card_h(th, call));
            if call.state == CallState::Ringing {
                self.waiting_card(cx, c, r, call);
            } else {
                let is_heard = call.id == heard.id;
                self.card(cx, c, r, call, is_heard && end, end, if is_heard { mic } else { MicNote::None });
            }
            y = r.bottom() + gap;
        }
        (y, calls.len() - shown.len())
    }

    /// Touch: the path a call is answered on by the button that answers,
    /// and what the button says.
    fn answer_label(cx: &Cx) -> &'static str {
        if cx.off_hook {
            tr::ANSWER_ON_HANDSET
        } else if cx.st.audio.headset_mode {
            tr::ANSWER_ON_HEADSET
        } else {
            tr::ANSWER_ON_SPEAKER
        }
    }

    /// Touch: the screen of a call that rings while there is no other.
    /// The caller, large, in the upper half; at the bottom, where the hand
    /// is, the button that answers, which says where the call will be
    /// heard (the handset answers it there by being lifted), with the
    /// buttons that silence the ringing and decline the call under it.
    /// Between them, small: the ways to send the call elsewhere, and to
    /// answer it on the other of the speaker and the headset.
    fn draw_ringing(&mut self, cx: &Cx, c: &mut Canvas, area: IRect, call: &Call, calls: &[&Call]) {
        let th = cx.th;
        let st = cx.st;
        let m = dialpad::MARGIN;
        let (header, body) = area.split_top(th.header_h);
        let title = if call.paging { tr::PAGING } else { tr::INCOMING_CALL_TITLE };
        call_header_in(cx, c, header, title, if call.srtp { &["lock"] } else { &[] });
        let inner = IRect::new(body.x + m, body.y, body.w - 2 * m, body.h - m);
        let gap = th.pad;
        let id = call.id.0;
        let silenced = self.silenced.contains(&call.id);

        // Silence and Decline, apart; Decline alone once silenced.
        let row = IRect::new(inner.x, inner.bottom() - th.row_h, inner.w, th.row_h);
        if silenced {
            dialpad::control(c, th, row, "", tr::DECLINE, Look::Stop, Hit::Custom(HIT_DECLINE | id));
        } else {
            let cols = dialpad::columns(row, 2, th.px(12));
            dialpad::control(c, th, cols[0], "", tr::SILENCE, Look::Plain, Hit::Custom(HIT_SILENCE | id));
            dialpad::control(c, th, cols[1], "", tr::DECLINE, Look::Stop, Hit::Custom(HIT_DECLINE | id));
        }
        let answer = IRect::new(inner.x, row.y - gap - th.px(80), inner.w, th.px(80));
        dialpad::control(c, th, answer, "phone", Self::answer_label(cx), Look::Go, Hit::Custom(HIT_ANSWER | id));

        // The small buttons: to another number, to voicemail (where the
        // account has a number for it), and the other way to answer (the
        // speaker's and the headset's keys answer a ringing call there).
        let other: Option<(&str, &str, Key)> = if cx.off_hook || st.audio.headset_mode {
            Some(("speaker", tr::ON_SPEAKER, Key::Speaker))
        } else if st.audio.headset_kind != opendeskphone_state::HeadsetKind::Wired {
            Some(("headset", tr::ON_HEADSET, Key::Headset))
        } else {
            None
        };
        let voicemail = st.accounts.get(call.account).is_some_and(|a| !a.voicemail_number.is_empty());
        let small = IRect::new(inner.x, answer.y - gap - th.touch_min, inner.w, th.touch_min);
        let n = 1 + usize::from(voicemail) + usize::from(other.is_some());
        let mut cols = dialpad::columns(small, n.max(2), gap).into_iter();
        dialpad::control(c, th, cols.next().unwrap_or_default(), "transfer", tr::SEND_TO, Look::Plain, Hit::Custom(HIT_SEND_TO | id));
        if voicemail {
            dialpad::control(c, th, cols.next().unwrap_or_default(), "voicemail", tr::TO_VOICEMAIL, Look::Plain, Hit::Custom(HIT_TO_VOICEMAIL | id));
        }
        if let Some((icon, label, key)) = other {
            let r = cols.last().unwrap_or_default();
            dialpad::control(c, th, r, icon, label, Look::Go, Hit::Key(key));
        }

        // Other calls that ring: their cards, above the buttons.
        let others: Vec<&Call> = calls.iter().copied().filter(|other| other.id != call.id).collect();
        let cards_h: i32 = others.iter().map(|other| Self::card_h(th, other) + th.px(6)).sum();
        let top = small.y - gap - cards_h;
        self.cards(cx, c, inner.x, inner.w, top, small.y - gap, &others, call, MicNote::None);

        // The caller: the name, large, over the number; the number alone,
        // large, where there is no name.
        let space = IRect::new(inner.x, body.y, inner.w, top - body.y);
        let fg = th.wallpaper_fg;
        let (number, name) = card_lines(call);
        let first = if name.is_empty() { &number } else { &name };
        let big = name_font(th, first, space.w);
        let sub = th.date_font();
        let note = th.font(Font::Normal);
        let mut block = big.height();
        if !name.is_empty() {
            block += th.px(4) + sub.height();
        }
        if silenced {
            block += gap + note.height();
        }
        let mut y = space.y + ((space.h - block) / 2).max(0);
        c.text_in(big, IRect::new(space.x, y, space.w, big.height()), first, fg, HAlign::Center);
        y += big.height();
        if !name.is_empty() {
            y += th.px(4);
            c.text_in(sub, IRect::new(space.x, y, space.w, sub.height()), &number, fg, HAlign::Center);
            y += sub.height();
        }
        if silenced {
            c.text_in(note, IRect::new(space.x, y + gap, space.w, note.height()), tr::RINGER_SILENCED, fg, HAlign::Center);
        }
    }

    /// The touch skin's call screen.  A call that rings with no other has
    /// a screen of its own ([`CallScreen::draw_ringing`]).  Otherwise,
    /// from the top: the state of the call that is heard, and the calls'
    /// cards, in the calls' order; from the bottom: the button that ends
    /// the call that is heard, and above it the buttons for what the keys
    /// of a phone that has keys do during a call, or, when it is asked
    /// for, the dial pad.  A call that rings meanwhile has its buttons on
    /// its card.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let st = cx.st;
        let m = dialpad::MARGIN;
        let Some(sel) = self.heard(cx) else { return };
        let calls: Vec<&Call> = st.calls.live().collect();
        self.silenced.retain(|id| calls.iter().any(|call| call.id == *id && call.state == CallState::Ringing));
        if self.transfer.is_some_and(|id| !calls.iter().any(|call| call.id == id)) {
            self.transfer = None;
        }
        let state = &sel.state;
        let held = state.is_held();

        // The microphone: muted, or unmuted by the phone and not by the
        // Mute button.  The phone unmutes when a call is put on hold or
        // taken back; the card says so while the call is on hold and for
        // a moment after.
        let muted = st.audio.muted || sel.muted;
        let by_button = self.mute_key_ms.is_some_and(|at| cx.now_ms.saturating_sub(at) < 1500);
        if self.muted && !muted && !by_button {
            self.unmuted_ms = Some(cx.now_ms);
        }
        self.muted = muted;
        match self.unmuted_ms {
            Some(_) if muted || *state == CallState::Ringing => self.unmuted_ms = None,
            Some(_) if held => self.unmuted_ms = Some(cx.now_ms),
            Some(at) if cx.now_ms >= at + MIC_NOTE_MS => self.unmuted_ms = None,
            _ => {}
        }
        let mic = match (muted, self.unmuted_ms) {
            (true, _) => MicNote::Muted,
            (false, Some(_)) => MicNote::OnAgain,
            (false, None) => MicNote::None,
        };

        if *state == CallState::Ringing {
            return self.draw_ringing(cx, c, area, sel, &calls);
        }
        let (label, _, _) = call_parts(sel, cx.now_ms);
        let mut flags: Vec<&str> = Vec::new();
        if muted {
            flags.push("mute");
        }
        if sel.srtp {
            flags.push("lock");
        }
        let (header, body) = area.split_top(th.header_h);
        call_header_in(cx, c, header, &label, &flags);
        let inner = IRect::new(body.x + m, body.y + th.px(4), body.w - 2 * m, body.h - th.px(4) - m);
        let gap = th.pad;
        let established = state.is_established();
        let can_type = state.has_media() || held;
        // (A call that rings takes the dial pad's place: it has to be
        // seen, and its buttons are on its card.)
        let keypad = self.keypad && can_type && !calls.iter().any(|call| call.state == CallState::Ringing);
        let left_out = |c: &mut Canvas, n: usize| {
            if n > 0 {
                let f = th.font(Font::Small);
                let s = format!("+{n}");
                let w = f.width(&s);
                let x = header.right() - m - w - flags.len() as i32 * (th.icon + th.pad);
                c.text_in(f, IRect::new(x, header.y, w, header.h), &s, th.title_fg, HAlign::Left);
            }
        };

        let bottom = IRect::new(inner.x, inner.bottom() - th.px(60), inner.w, th.px(60));
        if keypad {
            // The call the digits go to, what was sent, the pad, and the
            // way back to the buttons.
            let r = IRect::new(inner.x, inner.y, inner.w, Self::card_h(th, sel));
            self.card(cx, c, r, sel, false, false, mic);
            left_out(c, calls.len() - 1);
            let sent = IRect::new(inner.x, r.bottom() + th.px(6), inner.w, th.px(32));
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
        dialpad::control(c, th, bottom, "phone", tr::END_CALL, Look::Stop, Hit::Soft(10));
        let mut top = bottom.y - gap;

        // With another call up, the two can be connected: the end of a
        // transfer begun here ("Ask first"), which the button says, in
        // green; or, of two calls that came about otherwise, who goes to
        // whom.
        let other = self.other(cx, sel.id).filter(|_| established);
        if let Some(o) = other {
            let r = IRect::new(inner.x, top - th.row_h, inner.w, th.row_h);
            if self.transfer == Some(o.id) {
                dialpad::control(c, th, r, "transfer", tr::COMPLETE_TRANSFER, Look::Go, Hit::Custom(HIT_COMPLETE));
            } else {
                let label = format!("{} {} {} {}", tr::TRANSFER, party_name_or_number(o), tr::TRANSFER_WHOM_TO, party_name_or_number(sel));
                dialpad::control(c, th, r, "transfer", &label, Look::Plain, Hit::Custom(HIT_COMPLETE));
            }
            top = r.y - gap;
        }

        // The buttons: four to a row, or, with one more than two rows of
        // four hold, three.  The audio buttons show what the lamps of the
        // keys show; a muted microphone is in the warning color, and says
        // so.
        let (speaker, headset, _) = opendeskphone_state::audio_lamps(&st.audio, &st.calls);
        let look = |on: bool, enabled: bool| match (enabled, on) {
            (false, _) => Look::Off,
            (true, true) => Look::On,
            (true, false) => Look::Plain,
        };
        let active = *state == CallState::Active;
        let mut buttons: Vec<(&str, &str, Look, Hit)> = vec![
            if muted { ("mute", tr::MUTED, Look::Warn, Hit::Key(Key::Mute)) } else { ("mute", tr::MUTE, Look::Plain, Hit::Key(Key::Mute)) },
            (dialpad::KEYPAD, tr::KEYPAD, look(false, can_type), Hit::Custom(HIT_KEYPAD)),
            ("speaker", tr::VOL_SPEAKER, look(speaker.color != LampColor::Off, true), Hit::Key(Key::Speaker)),
            ("headset", tr::VOL_HEADSET, look(headset.color != LampColor::Off, true), Hit::Key(Key::Headset)),
            ("hold", if held { tr::RESUME } else { tr::HOLD }, look(held, held || active), Hit::Key(Key::Hold)),
            ("phone-out", tr::NEW_CALL, look(false, established), Hit::Soft(15)),
            // (With another call up, the button above is the transfer.)
            ("transfer", tr::TRANSFER, look(false, established && other.is_none()), Hit::Key(Key::Transfer)),
        ];
        if sel.in_conference && active {
            buttons.push(("conference", tr::SPLIT, Look::Plain, Hit::Soft(13)));
        } else {
            let label = if other.is_some() { tr::JOIN } else { tr::CONFERENCE };
            buttons.push(("conference", label, look(false, established), Hit::Key(Key::Conference)));
        }
        if cx.cfg.park {
            buttons.push(("park", tr::PARK, look(false, active && !sel.in_conference), Hit::Soft(11)));
        }
        let per_row = if buttons.len() > 8 { 3 } else { 4 };
        let cell_h = th.px(58);
        let rows = buttons.len().div_ceil(per_row) as i32;
        let grid = IRect::new(inner.x, top - rows * cell_h - (rows - 1) * gap, inner.w, rows * cell_h + (rows - 1) * gap);
        for (i, (icon, label, look, hit)) in buttons.into_iter().enumerate() {
            let row = IRect::new(grid.x, grid.y + (i / per_row) as i32 * (cell_h + gap), grid.w, cell_h);
            dialpad::control(c, th, dialpad::columns(row, per_row, gap)[i % per_row], icon, label, look, hit);
        }
        top = grid.y - gap;

        // The volume of what is heard: its level between the buttons that
        // lower and raise it.  (Not while the call is on hold: nothing is
        // heard, and the buttons would set the ringer's.)
        if !held {
            let row = IRect::new(inner.x, top - th.touch_min, inner.w, th.touch_min);
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
            top = row.y - gap;
        }

        // The calls: one has the room there is; several are cards in the
        // calls' order, which a call keeps whichever is heard.
        if let [only] = calls[..] {
            let h = (top - inner.y).min(th.px(200));
            self.big_card(cx, c, IRect::new(inner.x, inner.y, inner.w, h), only, mic);
        } else {
            let (_, out) = self.cards(cx, c, inner.x, inner.w, inner.y, top, &calls, sel, mic);
            left_out(c, out);
        }
    }
}
