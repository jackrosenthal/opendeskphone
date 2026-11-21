//! The D80's idle screen, dialer, and call screen, by touch, through the
//! HAL simulator's touchscreen: the idle screen says in words what its
//! user has to know and leads to what a desk phone is used for; a number
//! is dialed by touching the dial pad; and a call is answered, held,
//! muted, transferred, sent DTMF, and ended by touching the call screen's
//! buttons, which do what the keys of a phone that has keys do.

use opendeskphone_hal::Hal;
use opendeskphone_state::*;
use opendeskphone_ui::gfx::IRect;
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::{Hit, Ui, UiConfig};

const MONO: u64 = 5_000_000;
const UNIX: u64 = 1_794_607_087;

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    calls: CommandReceiver,
    audio: CommandReceiver,
    apps: CommandReceiver,
}

impl Rig {
    /// A D80 with two accounts on its first two lines, a watched contact
    /// on the third, and the rest of its lines unused.
    fn new() -> Rig {
        Rig::with_config(UiConfig { backlight_timeout_secs: 0, ..UiConfig::for_model(Model::D80) })
    }
    fn with_config(cfg: UiConfig) -> Rig {
        let model = Model::D80;
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        store.update::<Accounts, _>(|a| {
            for i in 0..2 {
                let mut acct = Account::new(AccountId(i));
                acct.label = format!("10{i}");
                acct.registration = Registration::Registered { host: ActiveHost::Primary, expires_mono_ms: u64::MAX };
                a.accounts.push(acct);
            }
            a.default_account = Some(AccountId(0));
        });
        store.update::<Presence, _>(|p| {
            p.entries.push(BlfEntry { id: "blf0".into(), label: "Alice".into(), number: "200".into(), state: BlfState::Idle, ..Default::default() });
        });
        store.update::<Keys, _>(|k| {
            for i in 0..model.lines() {
                let binding = match i {
                    0 | 1 => LineKeyBinding::Account(AccountId(i)),
                    2 => LineKeyBinding::Blf("blf0".into()),
                    _ => LineKeyBinding::Unused,
                };
                k.line_keys.push(LineKey { index: i, binding, label: String::new(), lamp: LampState::OFF, calls: vec![] });
            }
        });
        let calls = store.handle(Domain::Calls);
        let audio = store.handle(Domain::Audio);
        let apps = store.handle(Domain::Apps);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), cfg);
        ui.set_fixed_time(Some((MONO, UNIX)));
        ui.pump();
        Rig { store, ui, sim, calls, audio, apps }
    }
    fn screen(&self) -> String {
        self.ui.screen_name()
    }
    fn dial_buffer(&self) -> String {
        self.store.get::<opendeskphone_state::Ui>().dial_buffer.clone()
    }
    fn has(&self, hit: Hit) -> bool {
        self.ui.canvas().hit_rect(hit).is_some()
    }
    /// The region registered with `hit` in the picture on the screen.
    fn region(&self, hit: Hit) -> IRect {
        self.ui.canvas().hit_rect(hit).unwrap_or_else(|| panic!("no region {hit:?} on {}", self.screen()))
    }
    /// A finger down and up in the middle of the region of `hit`.
    fn tap(&mut self, hit: Hit) {
        let r = self.region(hit);
        self.sim.touch_tap((r.x + r.w / 2) as u16, (r.y + r.h / 2) as u16);
        self.ui.pump();
    }
    fn digits(&mut self, digits: &str) {
        for ch in digits.chars() {
            self.tap(Hit::Key(match ch {
                '*' => Key::Star,
                '#' => Key::Pound,
                d => Key::Digit(d as u8 - b'0'),
            }));
        }
    }
    fn drain(rx: &CommandReceiver) -> Vec<Command> {
        let mut v = Vec::new();
        while let Some(env) = rx.try_recv() {
            v.push(env.command.clone());
            env.done();
        }
        v
    }
    fn call_cmds(&self) -> Vec<Command> {
        Self::drain(&self.calls)
    }
    fn audio_cmds(&self) -> Vec<Command> {
        Self::drain(&self.audio)
    }
    fn add_call(&mut self, state: CallState, number: &str) -> CallId {
        let id = self.store.next_call_id();
        self.store.update::<Calls, _>(|c| {
            let mut call = Call::new(id, AccountId(0), Direction::Incoming, Party { number: number.into(), ..Default::default() });
            if state.is_established() {
                call.connected_mono_ms = Some(MONO - 10_000);
            }
            call.state = state;
            c.calls.push(call);
            c.focused = Some(id);
        });
        self.ui.pump();
        id
    }
    fn set_state(&mut self, id: CallId, state: CallState) {
        self.store.update::<Calls, _>(|c| c.get_mut(id).unwrap().state = state);
        self.ui.pump();
    }
    /// Every region of the picture on the screen is large enough to
    /// touch, and none reaches beyond the screen.
    fn check_regions(&self, what: &str) {
        let th = Theme::for_model(Model::D80);
        let hits = self.ui.canvas().hits().to_vec();
        assert!(!hits.is_empty(), "{what}");
        for (rect, hit) in hits {
            assert!(rect.w >= th.touch_min && rect.h >= th.touch_min, "{what}: {hit:?} is {rect:?}");
            assert!(rect.x >= 0 && rect.y >= 0 && rect.right() <= th.width && rect.bottom() <= th.height, "{what}: {hit:?} is {rect:?}");
        }
    }
}

/// The screens' own regions.  The dialer's: the delete button, the recent
/// numbers, and the accounts.  The call screen's: the dial pad button, the
/// volume buttons, the button that connects two calls, and the regions of
/// one call (its card, and the buttons that end, answer, silence, decline,
/// send on, and send to voicemail).
const DELETE: Hit = Hit::Custom(1);
const KEYPAD: Hit = Hit::Custom(1);
const VOLUME_DOWN: Hit = Hit::Custom(2);
const VOLUME_UP: Hit = Hit::Custom(3);
const COMPLETE: Hit = Hit::Custom(4);
fn recent(i: u32) -> Hit {
    Hit::Custom(50 + i)
}
fn account(i: u32) -> Hit {
    Hit::Custom(100 + i)
}
fn card(id: CallId) -> Hit {
    Hit::Custom(0x0100_0000 | id.0)
}
fn end(id: CallId) -> Hit {
    Hit::Custom(0x0200_0000 | id.0)
}
fn answer(id: CallId) -> Hit {
    Hit::Custom(0x0300_0000 | id.0)
}
fn silence(id: CallId) -> Hit {
    Hit::Custom(0x0400_0000 | id.0)
}
fn decline(id: CallId) -> Hit {
    Hit::Custom(0x0500_0000 | id.0)
}
fn send_to(id: CallId) -> Hit {
    Hit::Custom(0x0600_0000 | id.0)
}
fn to_voicemail(id: CallId) -> Hit {
    Hit::Custom(0x0700_0000 | id.0)
}
/// The idle screen's: the button that opens the dialer, the ringer's
/// volume, the buttons that turn Do Not Disturb and forwarding off, the
/// missed calls, the tile of a phone with no account, and the free line
/// whose line key is `n`.
const NEW_CALL: Hit = Hit::Custom(1);
const RINGER_VOLUME: Hit = Hit::Custom(2);
const DND_OFF: Hit = Hit::Custom(3);
const FORWARD_OFF: Hit = Hit::Custom(4);
const MISSED: Hit = Hit::Custom(5);
const NO_ACCOUNT: Hit = Hit::Custom(6);
fn line(n: u32) -> Hit {
    Hit::Custom(100 + n)
}
/// The home buttons of the D80 when none are configured: a softkey
/// action that is 1000 and the button's place, from 1.
const HOME_CALL_LOG: Hit = Hit::Soft(1000 + 1);
const HOME_VOICEMAIL: Hit = Hit::Soft(1000 + 3);
const HOME_REDIAL: Hit = Hit::Soft(1000 + 4);
const HOME_DND: Hit = Hit::Soft(1000 + 5);
const HOME_FORWARD: Hit = Hit::Soft(1000 + 6);

fn log_entry(kind: CallLogKind, number: &str) -> CallLogEntry {
    CallLogEntry {
        id: 0,
        kind,
        party: Party { number: number.into(), ..Default::default() },
        account: AccountId(0),
        start_unix: UNIX - 600,
        duration_secs: 0,
    }
}

impl Rig {
    fn notices(&self) -> Vec<Notice> {
        self.store.get::<opendeskphone_state::Ui>().notices.clone()
    }
    /// How many pixels of the region of `hit` have `color`.
    fn count(&self, hit: Hit, color: u32) -> usize {
        let (r, c) = (self.region(hit), self.ui.canvas());
        (r.y..r.bottom()).flat_map(|y| (r.x..r.right()).map(move |x| (x, y))).filter(|(x, y)| c.pixel(*x, *y) == color).count()
    }
    /// How many rows of a list are on the screen.
    fn rows(&self) -> usize {
        self.ui.canvas().hits().iter().filter(|(_, hit)| matches!(hit, Hit::Row { .. })).count()
    }
    /// No two regions of the picture on the screen overlap.
    fn check_apart(&self, what: &str) {
        let hits = self.ui.canvas().hits().to_vec();
        for (i, (a, ha)) in hits.iter().enumerate() {
            for (b, hb) in &hits[i + 1..] {
                assert!(a.intersect(b).is_empty(), "{what}: {ha:?} {a:?} and {hb:?} {b:?}");
            }
        }
    }
}

#[test]
fn the_idle_screen_shows_the_lines_in_use() {
    let mut r = Rig::new();
    assert_eq!(r.screen(), "idle");
    r.check_regions("idle");
    r.check_apart("idle");
    // The two accounts, which are free, and the watched contact; not the
    // unused lines.
    assert!(r.has(line(1)) && r.has(line(2)) && r.has(Hit::Key(Key::Line(3))));
    assert!(!(4..=Model::D80.lines()).any(|n| r.has(Hit::Key(Key::Line(n))) || r.has(line(u32::from(n)))));
    // A watched contact's line calls the contact, as its key does.
    r.tap(Hit::Key(Key::Line(3)));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "200".into(), account: None, path: None }]);
    // A free line opens the dialer for its account, as New Call does for
    // the default one: nothing goes off-hook until the number is called.
    r.tap(line(2));
    assert_eq!(r.screen(), "dialer");
    assert!(r.audio_cmds().is_empty());
    r.digits("12");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "12".into(), account: Some(AccountId(1)), path: None }]);
}

#[test]
fn a_line_shows_what_its_lamp_shows() {
    let mut r = Rig::new();
    let th = Theme::for_model(Model::D80);
    assert_eq!(r.count(line(1), th.good), 0);
    assert_eq!(r.count(line(1), th.error), 0);
    r.store.update::<Keys, _>(|k| k.line_keys[0].lamp = LampState::solid(LampColor::Green));
    r.ui.pump();
    assert!(r.count(line(1), th.good) > 100);
    // A blinking lamp is dark for the dark part of its blinking.
    r.store.update::<Keys, _>(|k| k.line_keys[0].lamp = LampState::blinking(LampColor::Red, Blink::Slow));
    r.ui.set_fixed_time(Some((MONO + 100, UNIX)));
    r.ui.pump();
    assert!(r.count(line(1), th.error) > 100);
    r.ui.set_fixed_time(Some((MONO + 600, UNIX)));
    r.ui.pump();
    assert_eq!(r.count(line(1), th.error), 0);
}

#[test]
fn the_home_buttons_have_voicemail_and_redial_and_do_not_disturb_is_lit_while_on() {
    let mut r = Rig::new();
    let th = Theme::for_model(Model::D80);
    // Eight buttons, in two rows, each with room for its label.
    let buttons: Vec<IRect> = [1, 2, 3, 5, 6, 7, 8].map(|id| r.region(Hit::Soft(1000 + id))).to_vec();
    assert!(buttons[..3].iter().all(|b| b.y == buttons[0].y) && buttons[3..].iter().all(|b| b.y == buttons[3].y));
    assert!(buttons[3].y > buttons[0].y);
    assert!(r.region(HOME_DND).w > r.region(HOME_FORWARD).w, "Do Not Disturb has the width its label needs");
    // Nothing to redial yet: the button is not one.  Then the newest
    // call placed.
    assert!(!r.has(HOME_REDIAL));
    r.store.update::<CallLog, _>(|l| {
        l.push(log_entry(CallLogKind::Placed, "1001"));
        l.push(log_entry(CallLogKind::Placed, "1005"));
        l.push(log_entry(CallLogKind::Received, "1009"));
    });
    r.ui.pump();
    r.tap(HOME_REDIAL);
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "1005".into(), account: Some(AccountId(0)), path: None }]);
    // Do Not Disturb: the button is lit while it is on, and a banner
    // says so in words, with the button that turns it off.
    assert!(!r.has(DND_OFF));
    let unlit = r.count(HOME_DND, th.sk_fg);
    r.tap(HOME_DND);
    assert_eq!(r.call_cmds(), vec![Command::SetDnd { on: true }]);
    r.store.update::<Features, _>(|f| f.dnd = true);
    r.ui.pump();
    assert!(r.count(HOME_DND, th.sk_fg) > 4 * unlit, "lit");
    r.check_regions("idle with Do Not Disturb");
    r.check_apart("idle with Do Not Disturb");
    let banner = r.ui.canvas().pixels().iter().filter(|p| **p == th.warn).count();
    assert!(banner > 700 * 80 / 2, "a banner as wide as the screen's content: {banner}");
    r.tap(DND_OFF);
    assert_eq!(r.call_cmds(), vec![Command::SetDnd { on: false }]);
    // Forwarding every call: the banner, and its button.
    r.store.update::<Features, _>(|f| {
        f.dnd = false;
        f.forward.always = "1099".into();
    });
    r.ui.pump();
    assert!(!r.has(DND_OFF));
    r.tap(FORWARD_OFF);
    assert_eq!(r.call_cmds(), vec![Command::SetForward { kind: ForwardKind::Always, target: String::new(), no_answer_secs: None }]);
    // Forwarding when busy does not stop the phone from ringing.
    r.store.update::<Features, _>(|f| f.forward = Forward { busy: "1099".into(), ..Default::default() });
    r.ui.pump();
    assert!(!r.has(FORWARD_OFF));
}

#[test]
fn the_idle_screen_says_what_was_missed_and_nothing_in_its_corner_is_touched() {
    let mut r = Rig::new();
    let th = Theme::for_model(Model::D80);
    assert!(!r.has(Hit::Key(Key::Messages)) && !r.has(MISSED));
    r.store.update::<Mwi, _>(|m| {
        m.accounts.push(MwiAccount { account: AccountId(0), waiting: true, new: 2, old: 0, urgent_new: 0, urgent_old: 0 })
    });
    r.store.update::<CallLog, _>(|l| {
        l.push(log_entry(CallLogKind::Placed, "1001"));
        l.push(log_entry(CallLogKind::Missed, "1002"));
    });
    r.ui.pump();
    r.check_regions("idle with messages and missed calls");
    r.check_apart("idle with messages and missed calls");
    // The cards are as wide as the screen's content; where every other
    // screen has its back button, nothing is touched.
    assert!(r.region(MISSED).w > 700 && r.region(Hit::Key(Key::Messages)).w > 700);
    let corner = IRect::new(0, 0, th.back_w, th.title_h);
    for (rect, hit) in r.ui.canvas().hits() {
        assert!(rect.intersect(&corner).is_empty(), "{hit:?} is in the corner");
    }
    // Voicemail: the account's number, of the first account that has
    // one; a phone with none says so.
    r.tap(Hit::Key(Key::Messages));
    assert!(r.call_cmds().is_empty());
    assert_eq!(r.notices().len(), 1);
    r.tap(Hit::Dismiss);
    assert!(r.notices().is_empty());
    r.store.update::<Accounts, _>(|a| a.accounts[1].voicemail_number = "*97".into());
    r.tap(Hit::Key(Key::Messages));
    assert_eq!(r.call_cmds(), vec![Command::DialVoicemail { account: Some(AccountId(1)) }]);
    // The voicemail app, though installed, cannot reach the server
    // without its application account: the home button calls too.
    r.store.update::<Apps, _>(|a| a.installed.push(AppInfo { name: "voicemail".into(), ..Default::default() }));
    r.tap(HOME_VOICEMAIL);
    assert_eq!(r.call_cmds(), vec![Command::DialVoicemail { account: Some(AccountId(1)) }]);
    assert!(Rig::drain(&r.apps).is_empty());
    let cfg = UiConfig { backlight_timeout_secs: 0, voicemail_app_account: true, ..UiConfig::for_model(Model::D80) };
    r.ui.handle().set_config(cfg);
    r.ui.pump();
    r.tap(HOME_VOICEMAIL);
    assert_eq!(Rig::drain(&r.apps), vec![Command::LaunchApp { name: "voicemail".into() }]);
    // The Call Log button, while there are missed calls not yet looked
    // at, opens the log on them (one entry of the two); so does the
    // card.
    r.store.update::<CallLog, _>(|l| l.missed_unseen = 1);
    r.ui.pump();
    r.tap(HOME_CALL_LOG);
    assert_eq!(r.screen(), "call_log");
    assert_eq!(r.rows(), 1);
    r.tap(Hit::Key(Key::Cancel));
    r.store.update::<CallLog, _>(|l| l.missed_unseen = 1);
    r.ui.pump();
    r.tap(MISSED);
    assert_eq!(r.screen(), "call_log");
    assert_eq!(r.rows(), 1);
    // With none, the whole log.
    r.tap(Hit::Key(Key::Cancel));
    r.store.update::<CallLog, _>(|l| l.missed_unseen = 0);
    r.ui.pump();
    r.tap(HOME_CALL_LOG);
    assert_eq!(r.rows(), 2);
}

#[test]
fn the_ringer_volume_is_set_from_the_idle_screen() {
    let mut r = Rig::new();
    assert!(!r.has(Hit::Key(Key::VolumeUp)));
    r.tap(RINGER_VOLUME);
    r.tap(Hit::Key(Key::VolumeUp));
    r.tap(Hit::Key(Key::VolumeDown));
    let up = Command::AdjustVolume { target: VolumeTarget::Current, delta: 1 };
    let down = Command::AdjustVolume { target: VolumeTarget::Current, delta: -1 };
    assert_eq!(r.audio_cmds(), vec![up, down]);
    // A touch beside the popup takes it down.
    assert!(r.has(Hit::Dismiss));
    r.sim.touch_tap(400, 1200);
    r.ui.pump();
    assert!(!r.has(Hit::Key(Key::VolumeUp)));
}

#[test]
fn a_phone_with_no_account_says_so_and_does_not_dial() {
    let mut r = Rig::new();
    r.store.update::<Accounts, _>(|a| *a = Accounts::default());
    r.store.update::<Keys, _>(|k| k.line_keys.clear());
    r.ui.pump();
    r.check_regions("idle with no account");
    r.check_apart("idle with no account");
    assert!(r.region(NO_ACCOUNT).w > 700);
    r.tap(NEW_CALL);
    r.digits("45");
    r.tap(Hit::Soft(1));
    assert!(r.call_cmds().is_empty());
    assert_eq!(r.screen(), "idle");
    let notices = r.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].text.contains("no account"));
    r.tap(Hit::Dismiss);
    // The tile leads to the accounts, and, on a phone used without a
    // server, back to its setup.
    r.tap(NO_ACCOUNT);
    assert_eq!(r.screen(), "accounts");
    r.tap(Hit::Key(Key::Cancel));
    let provisioning = r.store.handle(Domain::Provisioning);
    r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Standalone);
    r.ui.pump();
    r.tap(NO_ACCOUNT);
    assert_eq!(Rig::drain(&provisioning), vec![Command::StartProvisioning { server: None }]);
}

#[test]
fn a_number_is_dialed_by_touching_the_dial_pad() {
    let mut r = Rig::new();
    r.tap(NEW_CALL);
    assert_eq!(r.screen(), "dialer");
    r.check_regions("dialer");
    r.check_apart("dialer");
    // No softkeys: the dialer's buttons are its own.
    assert!(!r.ui.canvas().hits().iter().any(|(rect, _)| rect.y >= 1160 && rect.h == 96));
    r.digits("1002*#");
    assert_eq!(r.dial_buffer(), "1002*#");
    // On-hook digits play no tone and do not dial by themselves.
    assert!(r.audio_cmds().is_empty());
    assert!(r.call_cmds().is_empty());
    r.tap(DELETE);
    r.tap(DELETE);
    assert_eq!(r.dial_buffer(), "1002");
    r.check_regions("dialer with digits");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "1002".into(), account: None, path: None }]);
    assert_eq!(r.screen(), "idle");
    // The contacts are beside the number.
    r.tap(NEW_CALL);
    r.tap(Hit::Soft(5));
    assert_eq!(Rig::drain(&r.apps), vec![Command::LaunchApp { name: "contacts".into() }]);
}

#[test]
fn the_dialer_deletes_all_when_the_finger_stays_and_leaves_by_its_back_button() {
    let mut r = Rig::new();
    r.tap(NEW_CALL);
    // With nothing typed there is nothing to delete.
    assert!(!r.has(DELETE));
    r.digits("555");
    let del = r.region(DELETE);
    let (x, y) = ((del.x + del.w / 2) as u16, (del.y + del.h / 2) as u16);
    r.sim.touch(TouchPhase::Down, x, y);
    r.ui.pump();
    r.ui.set_fixed_time(Some((MONO + opendeskphone_ui::touch::LONG_PRESS_MS, UNIX)));
    r.ui.pump();
    r.sim.touch(TouchPhase::Up, x, y);
    r.ui.pump();
    assert_eq!(r.dial_buffer(), "");
    assert_eq!(r.screen(), "dialer");
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "idle");
    assert!(r.call_cmds().is_empty());
}

#[test]
fn the_dialer_calls_on_the_account_chosen() {
    let mut r = Rig::new();
    r.tap(NEW_CALL);
    r.digits("300");
    // Choosing the account does not dial.
    r.tap(account(1));
    assert!(r.call_cmds().is_empty());
    assert_eq!(r.screen(), "dialer");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "300".into(), account: Some(AccountId(1)), path: None }]);
}

#[test]
fn the_dialer_with_no_number_redials_and_its_speaker_button_is_the_key() {
    let mut r = Rig::new();
    r.tap(NEW_CALL);
    // With no call placed yet there is nothing to call again.
    assert!(!r.has(Hit::Soft(1)));
    r.store.update::<CallLog, _>(|l| l.push(log_entry(CallLogKind::Placed, "1001")));
    r.ui.pump();
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "1001".into(), account: Some(AccountId(0)), path: None }]);
    assert_eq!(r.screen(), "idle");
    // With a number, Speaker calls it on the speaker.
    r.tap(NEW_CALL);
    r.digits("41");
    r.tap(Hit::Key(Key::Speaker));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "41".into(), account: None, path: Some(AudioPath::Speaker) }]);
    // With none, it goes off-hook there: dial tone, and the digits dial
    // by themselves after the timeout.
    r.tap(NEW_CALL);
    r.tap(Hit::Key(Key::Headset));
    assert_eq!(
        r.audio_cmds(),
        vec![Command::PlayTone { tone: Tone::Dial, duration_ms: 0 }, Command::SetAudioPath { path: AudioPath::Headset }]
    );
    r.digits("7");
    assert_eq!(
        r.audio_cmds(),
        vec![Command::StopTone, Command::PlayTone { tone: Tone::Dtmf('7'), duration_ms: 100 }]
    );
    assert!(r.call_cmds().is_empty());
    r.ui.set_fixed_time(Some((MONO + 4_100, UNIX + 4)));
    r.ui.pump();
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "7".into(), account: None, path: Some(AudioPath::Headset) }]);
}

#[test]
fn the_handset_dials_off_hook_with_the_dial_pad() {
    let mut r = Rig::new();
    r.sim.set_hook(true);
    r.ui.pump();
    assert_eq!(r.screen(), "dialer");
    assert_eq!(
        r.audio_cmds(),
        vec![Command::SetAudioPath { path: AudioPath::Handset }, Command::PlayTone { tone: Tone::Dial, duration_ms: 0 }]
    );
    r.digits("10");
    let tones = r.audio_cmds();
    assert_eq!(tones[0], Command::StopTone);
    assert!(tones.contains(&Command::PlayTone { tone: Tone::Dtmf('0'), duration_ms: 100 }));
    // Before the inter-digit timeout nothing is dialed; after it, the
    // number is, on the handset.
    r.ui.set_fixed_time(Some((MONO + 3_000, UNIX + 3)));
    r.ui.pump();
    assert!(r.call_cmds().is_empty());
    r.ui.set_fixed_time(Some((MONO + 4_100, UNIX + 4)));
    r.ui.pump();
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "10".into(), account: None, path: Some(AudioPath::Handset) }]);
    // Hanging up a dialer that was lifted into closes it.
    r.sim.set_hook(false);
    r.ui.pump();
    r.sim.set_hook(true);
    r.ui.pump();
    assert_eq!(r.screen(), "dialer");
    r.sim.set_hook(false);
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
}

#[test]
fn a_ringing_call_has_a_screen_of_its_own() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Ringing, "1002");
    assert_eq!(r.screen(), "call");
    r.check_regions("ringing call");
    r.check_apart("ringing call");
    // What is not for a ringing call is not there.
    for hit in [Hit::Key(Key::Hold), Hit::Key(Key::Mute), Hit::Key(Key::Conference), KEYPAD, VOLUME_UP, Hit::Soft(10), Hit::Soft(15)] {
        assert!(!r.has(hit), "{hit:?}");
    }
    // Answer is as wide as the screen's content and the tallest button;
    // Silence and Decline are under it, apart.
    let th = Theme::for_model(Model::D80);
    let (a, s, d) = (r.region(answer(id)), r.region(silence(id)), r.region(decline(id)));
    assert!(a.w > 700 && a.h >= 160 && a.bottom() <= s.y);
    assert!(s.h >= th.row_h && d.x - s.right() >= 24 && s.y == d.y);
    r.tap(answer(id));
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: None }]);
    r.tap(decline(id));
    assert_eq!(r.call_cmds(), vec![Command::Reject { call: id, busy: true }]);
    // Silenced: the button is gone (the screen says that the ringer is
    // silenced), and Decline has its row.
    r.tap(silence(id));
    assert_eq!(r.call_cmds(), vec![Command::Silence { call: id }]);
    assert!(!r.has(silence(id)));
    assert!(r.region(decline(id)).w > 700);
    // To another number: the transfer's dialer.
    r.tap(send_to(id));
    assert_eq!(r.screen(), "dialer");
    r.digits("1009");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::TransferBlind { call: id, target: "1009".into() }]);
    // To voicemail, where the account has a number for it.
    assert!(!r.has(to_voicemail(id)));
    r.store.update::<Accounts, _>(|a| a.accounts[0].voicemail_number = "*97".into());
    r.ui.pump();
    r.tap(to_voicemail(id));
    assert_eq!(r.call_cmds(), vec![Command::SendToVoicemail { call: id }]);
    r.check_regions("ringing call, with voicemail");
    // The other way to answer: in headset mode Answer is for the headset,
    // and a smaller button, the Speaker key, answers on the speaker.
    assert!(!r.has(Hit::Key(Key::Speaker)) && !r.has(Hit::Key(Key::Headset)));
    r.store.update::<Audio, _>(|a| a.headset_mode = true);
    r.ui.pump();
    r.check_regions("ringing call, in headset mode");
    r.check_apart("ringing call, in headset mode");
    r.tap(Hit::Key(Key::Speaker));
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: Some(AudioPath::Speaker) }]);
    // The call is gone: so is the call screen.
    r.store.update::<Calls, _>(|c| c.calls.clear());
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
}

#[test]
fn a_call_that_rings_during_a_call_has_its_buttons_on_its_card() {
    let mut r = Rig::new();
    let first = r.add_call(CallState::Active, "1002");
    let second = r.add_call(CallState::Ringing, "1003");
    r.check_regions("call waiting");
    r.check_apart("call waiting");
    // The buttons below are still for the call that is heard.
    r.tap(Hit::Key(Key::Hold));
    r.tap(Hit::Soft(10));
    assert_eq!(r.call_cmds(), vec![Command::Hold { call: first }, Command::Hangup { call: first }]);
    // The ringing call's are on its card, under the first call's.
    assert!(r.region(card(first)).y < r.region(answer(second)).y && r.region(answer(second)).bottom() < r.region(VOLUME_UP).y);
    r.tap(answer(second));
    r.tap(decline(second));
    r.tap(silence(second));
    assert_eq!(
        r.call_cmds(),
        vec![
            Command::Answer { call: second, path: None },
            Command::Reject { call: second, busy: true },
            Command::Silence { call: second }
        ]
    );
    assert!(!r.has(silence(second)) && r.has(answer(second)));
    // It takes the dial pad's place while it rings.
    r.store.update::<Calls, _>(|c| c.calls.retain(|call| call.id != second));
    r.ui.pump();
    r.tap(KEYPAD);
    assert!(r.has(Hit::Key(Key::Digit(1))));
    let third = r.add_call(CallState::Ringing, "1004");
    assert!(!r.has(Hit::Key(Key::Digit(1))) && r.has(answer(third)));
}

#[test]
fn a_call_is_held_resumed_muted_and_ended() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    r.store.update::<Audio, _>(|a| a.path = AudioPath::Handset);
    r.ui.pump();
    r.check_regions("active call");
    r.check_apart("active call");
    // No softkeys: the call screen's buttons are its own.  No lines, and
    // with one call no button on its card.
    assert!(!r.has(Hit::Soft(1)) && !r.has(end(id)));
    assert!(!(1..=Model::D80.lines()).any(|n| r.has(Hit::Key(Key::Line(n)))));

    r.tap(Hit::Key(Key::Hold));
    assert_eq!(r.call_cmds(), vec![Command::Hold { call: id }]);
    r.set_state(id, CallState::Held { local: true, remote: false });
    r.tap(Hit::Key(Key::Hold));
    assert_eq!(r.call_cmds(), vec![Command::Resume { call: id }]);
    r.set_state(id, CallState::Active);

    r.tap(Hit::Key(Key::Mute));
    assert_eq!(r.audio_cmds(), vec![Command::SetMute { on: true }]);
    r.store.update::<Audio, _>(|a| a.muted = true);
    r.ui.pump();
    r.tap(Hit::Key(Key::Mute));
    assert_eq!(r.audio_cmds(), vec![Command::SetMute { on: false }]);

    r.tap(Hit::Key(Key::Speaker));
    assert_eq!(r.audio_cmds(), vec![Command::SetAudioPath { path: AudioPath::Speaker }]);
    r.tap(Hit::Key(Key::Headset));
    assert_eq!(r.audio_cmds(), vec![Command::SetAudioPath { path: AudioPath::Headset }]);

    r.tap(Hit::Soft(10));
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: id }]);
}

#[test]
fn the_buttons_show_what_is_on_and_a_muted_microphone_is_in_the_warning_color() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    r.store.update::<Audio, _>(|a| a.path = AudioPath::Handset);
    r.ui.pump();
    let th = Theme::for_model(Model::D80);
    // A button that is on is drawn in the label's color: its corner
    // region is that color, and the softkeys' fill when it is not.
    let on = |r: &Rig, hit: Hit| {
        let rect = r.region(hit);
        let c = r.ui.canvas();
        let p = c.pixel(rect.x + 20, rect.y + 12);
        assert!(p == c.pixel(rect.right() - 20, rect.bottom() - 12), "{hit:?}");
        p != c.pixel(r.region(Hit::Key(Key::Transfer)).x + 20, r.region(Hit::Key(Key::Transfer)).y + 12)
    };
    for key in [Key::Mute, Key::Speaker, Key::Headset, Key::Hold] {
        assert!(!on(&r, Hit::Key(key)), "{key:?}");
    }
    // Nothing on the screen is in the warning color until the microphone
    // is muted; then the button is, and so is a mark on the call's card.
    let warn = |r: &Rig| r.ui.canvas().pixels().iter().filter(|p| **p == th.warn).count();
    assert_eq!(warn(&r), 0);
    r.store.update::<Audio, _>(|a| {
        a.muted = true;
        a.path = AudioPath::Speaker;
    });
    r.ui.pump();
    assert!(on(&r, Hit::Key(Key::Mute)));
    assert!(on(&r, Hit::Key(Key::Speaker)));
    assert!(!on(&r, Hit::Key(Key::Headset)));
    let button = r.count(Hit::Key(Key::Mute), th.warn);
    assert!(button > 5000 && warn(&r) > button + 1000, "{button} {}", warn(&r));
    r.store.update::<Audio, _>(|a| {
        a.muted = false;
        a.path = AudioPath::Headset;
    });
    r.set_state(id, CallState::Held { local: true, remote: false });
    assert!(!on(&r, Hit::Key(Key::Mute)));
    assert!(!on(&r, Hit::Key(Key::Speaker)));
    assert!(on(&r, Hit::Key(Key::Headset)));
    assert!(on(&r, Hit::Key(Key::Hold)));
    assert_eq!(warn(&r), 0);
}

#[test]
fn the_volume_is_changed_on_the_call_screen_without_a_popup() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    let regions = r.ui.canvas().hits().len();
    r.tap(VOLUME_UP);
    r.tap(VOLUME_UP);
    r.tap(VOLUME_DOWN);
    let up = Command::AdjustVolume { target: VolumeTarget::Current, delta: 1 };
    let down = Command::AdjustVolume { target: VolumeTarget::Current, delta: -1 };
    assert_eq!(r.audio_cmds(), vec![up.clone(), up.clone(), down]);
    // Nothing came up over the screen; nor does the volume keys' popup
    // (the phone has no such keys, but they can be sent to it).
    assert_eq!(r.ui.canvas().hits().len(), regions);
    r.sim.tap(Key::VolumeUp);
    r.ui.pump();
    assert_eq!(r.audio_cmds(), vec![up]);
    assert_eq!(r.ui.canvas().hits().len(), regions);
    assert!(!r.has(Hit::Dismiss));
    // The level shown is the level of what sounds: more of the meter is
    // lit when it is higher.
    let th = Theme::for_model(Model::D80);
    let (minus, plus) = (r.region(VOLUME_DOWN), r.region(VOLUME_UP));
    let lit = |r: &Rig| {
        let c = r.ui.canvas();
        // (The first segment is lit at every level but none; it begins
        // after the icon of what sounds.)
        let y = minus.y + minus.h / 2;
        let color = c.pixel(minus.right() + 2 * th.pad + th.icon + th.pad, y);
        (minus.right()..plus.x).filter(|x| c.pixel(*x, y) == color).count()
    };
    r.store.update::<Audio, _>(|a| {
        a.path = AudioPath::Handset;
        a.handset_volume = 2;
    });
    r.ui.pump();
    let low = lit(&r);
    r.store.update::<Audio, _>(|a| a.handset_volume = 8);
    r.ui.pump();
    assert!(lit(&r) > low && low > 0, "{low} {}", lit(&r));
    // On hold nothing is heard: no volume to set.
    r.set_state(id, CallState::Held { local: true, remote: false });
    assert!(!r.has(VOLUME_UP) && !r.has(VOLUME_DOWN));
}

#[test]
fn dtmf_is_sent_from_the_dial_pad_of_the_call_screen() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    assert!(!r.has(Hit::Key(Key::Digit(1))));
    r.tap(KEYPAD);
    r.check_regions("call with the dial pad");
    r.digits("19#");
    assert_eq!(
        r.call_cmds(),
        vec![
            Command::Dtmf { call: id, digit: '1' },
            Command::Dtmf { call: id, digit: '9' },
            Command::Dtmf { call: id, digit: '#' }
        ]
    );
    assert_eq!(r.screen(), "call");
    // The call can be ended from here, and the buttons come back.
    assert!(r.has(Hit::Soft(10)));
    assert!(!r.has(Hit::Key(Key::Mute)));
    r.tap(KEYPAD);
    assert!(!r.has(Hit::Key(Key::Digit(1))));
    assert!(r.has(Hit::Key(Key::Mute)));
}

#[test]
fn a_call_is_transferred_now_or_after_asking_first() {
    let mut r = Rig::new();
    r.store.update::<CallLog, _>(|l| {
        l.push(log_entry(CallLogKind::Received, "1002"));
        l.push(log_entry(CallLogKind::Placed, "1008"));
        l.push(log_entry(CallLogKind::Missed, "1008"));
        l.push(log_entry(CallLogKind::Placed, "1007"));
    });
    let id = r.add_call(CallState::Active, "1002");
    r.tap(Hit::Key(Key::Transfer));
    assert_eq!(r.screen(), "dialer");
    r.check_regions("transfer dialer");
    r.check_apart("transfer dialer");
    // Nothing to transfer to yet.  The account is the call's: there is
    // none to choose.  The contacts are there.
    assert!(!r.has(Hit::Soft(1)) && !r.has(Hit::Soft(2)));
    assert!(!r.has(account(0)) && r.has(Hit::Soft(5)));
    // The recent numbers: each once, the newest first, without the party
    // being transferred.  A touch enters one.
    assert!(r.has(recent(0)) && r.has(recent(1)) && !r.has(recent(2)));
    r.tap(recent(1));
    assert_eq!(r.dial_buffer(), "1008");
    assert!(r.call_cmds().is_empty());
    r.tap(recent(0));
    assert_eq!(r.dial_buffer(), "1007");
    // "Transfer now" is at the right, "Ask first" at the left.
    assert!(r.region(Hit::Soft(2)).x < r.region(Hit::Soft(1)).x);
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::TransferBlind { call: id, target: "1007".into() }]);
    assert_eq!(r.screen(), "call");
    // Ask first: a call to the target; with the two calls up, the screen
    // offers to complete the transfer, and the Transfer button is not
    // one.
    assert!(!r.has(COMPLETE));
    r.tap(Hit::Key(Key::Transfer));
    r.digits("1004");
    r.tap(Hit::Soft(2));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "1004".into(), account: Some(AccountId(0)), path: None }]);
    r.set_state(id, CallState::Held { local: true, remote: false });
    let consult = r.add_call(CallState::Ringback { early_media: false }, "1004");
    assert_eq!(r.screen(), "call");
    // (Not before the other party has answered.)
    assert!(!r.has(COMPLETE));
    r.set_state(consult, CallState::Active);
    r.check_regions("consultation");
    r.check_apart("consultation");
    assert!(!r.has(Hit::Key(Key::Transfer)));
    let th = Theme::for_model(Model::D80);
    assert!(r.count(COMPLETE, th.sk_go) > 10_000, "green");
    r.tap(COMPLETE);
    assert_eq!(r.call_cmds(), vec![Command::TransferAttended { call: id, consult }]);
}

#[test]
fn park_is_a_button_on_a_phone_with_a_parking_extension() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    assert!(!r.has(Hit::Soft(11)));
    r.ui.handle().set_config(UiConfig { backlight_timeout_secs: 0, park: true, ..UiConfig::for_model(Model::D80) });
    r.ui.pump();
    r.check_regions("call with Park");
    r.check_apart("call with Park");
    r.tap(Hit::Soft(11));
    assert_eq!(r.call_cmds(), vec![Command::Park { call: id }]);
}

#[test]
fn the_cards_of_several_calls_keep_their_order_and_a_touch_goes_to_a_call() {
    let mut r = Rig::new();
    let first = r.add_call(CallState::Active, "1002");
    // New Call: the dialer for a call beside this one.
    r.tap(Hit::Soft(15));
    assert_eq!(r.screen(), "dialer");
    r.check_regions("conference dialer");
    r.digits("1005");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "1005".into(), account: Some(AccountId(0)), path: None }]);
    r.set_state(first, CallState::Held { local: true, remote: false });
    let second = r.add_call(CallState::Active, "1005");
    r.check_regions("two calls");
    // The cards are in the calls' order, whichever is heard; there is no
    // Swap.
    assert!(r.region(card(first)).y < r.region(card(second)).y);
    assert!(!r.has(Hit::Soft(14)));
    // The buttons below act on the call that is heard.
    r.tap(Hit::Key(Key::Hold));
    r.tap(Hit::Soft(10));
    assert_eq!(r.call_cmds(), vec![Command::Hold { call: second }, Command::Hangup { call: second }]);
    // Each call has a button that ends it.
    r.tap(end(first));
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: first }]);
    // A touch on the held call's card takes it back, which holds the
    // other; once it has, it is the one heard, and its card has not
    // moved.
    r.tap(card(first));
    assert_eq!(r.call_cmds(), vec![Command::Resume { call: first }]);
    r.store.update::<Calls, _>(|c| {
        c.get_mut(first).unwrap().state = CallState::Active;
        c.get_mut(second).unwrap().state = CallState::Held { local: true, remote: false };
        c.focused = Some(first);
    });
    r.ui.pump();
    assert!(r.region(card(first)).y < r.region(card(second)).y);
    r.tap(Hit::Soft(10));
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: first }]);
    // Join makes the conference; two calls that did not come about by a
    // transfer begun here can be connected too, and the button says who
    // goes to whom.
    r.tap(Hit::Key(Key::Conference));
    assert_eq!(r.call_cmds(), vec![Command::Conference { a: first, b: second }]);
    let th = Theme::for_model(Model::D80);
    assert_eq!(r.count(COMPLETE, th.sk_go), 0);
    r.tap(COMPLETE);
    assert_eq!(r.call_cmds(), vec![Command::TransferAttended { call: second, consult: first }]);
    // In the conference, Split takes its place.
    r.store.update::<Calls, _>(|c| {
        for call in &mut c.calls {
            call.state = CallState::Active;
            call.in_conference = true;
        }
    });
    r.ui.pump();
    r.tap(Hit::Soft(13));
    assert_eq!(r.call_cmds(), vec![Command::SplitConference { keep: first }]);
}

#[test]
fn the_models_with_keys_keep_their_softkeys_and_register_nothing() {
    for model in [Model::D40, Model::D65, Model::D70] {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        store.update::<Accounts, _>(|a| a.accounts.push(Account::new(AccountId(0))));
        let calls = store.handle(Domain::Calls);
        // The softkeys of the idle screen are those of a phone with keys.
        assert!(UiConfig::for_model(model) == UiConfig::default());
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..UiConfig::for_model(model) });
        ui.pump();
        for d in [1, 2] {
            sim.tap(Key::Digit(d));
        }
        ui.pump();
        assert_eq!(ui.screen_name(), "dialer");
        assert!(ui.canvas().hits().is_empty(), "{model}");
        sim.tap(Key::Soft(1));
        ui.pump();
        assert_eq!(Rig::drain(&calls), vec![Command::Dial { target: "12".into(), account: None, path: None }], "{model}");
        // With no number the softkey is Redial, as ever.
        sim.tap(Key::Digit(1));
        sim.tap(Key::Left);
        sim.tap(Key::Soft(1));
        ui.pump();
        assert_eq!(Rig::drain(&calls), vec![Command::Redial], "{model}");
        let id = store.next_call_id();
        store.update::<Calls, _>(|c| {
            let mut call = Call::new(id, AccountId(0), Direction::Incoming, Party::default());
            call.state = CallState::Active;
            c.calls.push(call);
            c.focused = Some(id);
        });
        ui.pump();
        assert_eq!(ui.screen_name(), "call");
        assert!(ui.canvas().hits().is_empty(), "{model}");
        sim.tap(Key::Soft(4));
        ui.pump();
        assert_eq!(Rig::drain(&calls), vec![Command::Hangup { call: id }], "{model}");
    }
}
