//! The D80's idle screen, dialer, and call screen, by touch, through the
//! HAL simulator's touchscreen: a number is dialed by touching the dial
//! pad, and a call is answered, held, muted, transferred, sent DTMF, and
//! ended by touching the call screen's buttons, which do what the keys of
//! a phone that has keys do.

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
        Rig::with_config(UiConfig { backlight_timeout_secs: 0, ..Default::default() })
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

/// The screens' own regions: the dialer's delete button and accounts, and
/// the call screen's dial pad button, volume buttons, and cards.
const DELETE: Hit = Hit::Custom(1);
const KEYPAD: Hit = Hit::Custom(1);
const VOLUME_DOWN: Hit = Hit::Custom(2);
const VOLUME_UP: Hit = Hit::Custom(3);
fn account(i: u32) -> Hit {
    Hit::Custom(100 + i)
}
fn card(id: CallId) -> Hit {
    Hit::Custom(1000 + id.0)
}
/// The idle screen's button that opens the dialer.
const NEW_CALL: Hit = Hit::Custom(1);

#[test]
fn the_idle_screen_shows_the_lines_in_use_and_each_is_its_line_key() {
    let mut r = Rig::new();
    assert_eq!(r.screen(), "idle");
    r.check_regions("idle");
    // The two accounts and the watched contact; not the unused lines.
    let lines: Vec<u8> = (1..=Model::D80.lines()).filter(|n| r.has(Hit::Key(Key::Line(*n)))).collect();
    assert_eq!(lines, vec![1, 2, 3]);
    // The tiles do not overlap one another, the dialer's button, or the
    // softkey buttons.
    let hits = r.ui.canvas().hits().to_vec();
    for (i, (a, ha)) in hits.iter().enumerate() {
        for (b, hb) in &hits[i + 1..] {
            assert!(a.intersect(b).is_empty(), "{ha:?} {a:?} and {hb:?} {b:?}");
        }
    }
    // A watched contact's line calls the contact, as its key does.
    r.tap(Hit::Key(Key::Line(3)));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "200".into(), account: None, path: None }]);
    // An account's line goes off-hook on that account, on the speaker.
    r.tap(Hit::Key(Key::Line(2)));
    assert_eq!(r.screen(), "dialer");
    assert_eq!(
        r.audio_cmds(),
        vec![Command::SetAudioPath { path: AudioPath::Speaker }, Command::PlayTone { tone: Tone::Dial, duration_ms: 0 }]
    );
    r.digits("12");
    r.tap(Hit::Soft(1));
    assert_eq!(
        r.call_cmds(),
        vec![Command::Dial { target: "12".into(), account: Some(AccountId(1)), path: Some(AudioPath::Speaker) }]
    );
}

#[test]
fn a_line_shows_what_its_lamp_shows() {
    let mut r = Rig::new();
    let th = Theme::for_model(Model::D80);
    let tile = r.region(Hit::Key(Key::Line(1)));
    let count = |r: &Rig, color: u32| {
        let c = r.ui.canvas();
        (tile.y..tile.bottom()).flat_map(|y| (tile.x..tile.right()).map(move |x| (x, y))).filter(|(x, y)| c.pixel(*x, *y) == color).count()
    };
    assert_eq!(count(&r, th.good), 0);
    assert_eq!(count(&r, th.error), 0);
    r.store.update::<Keys, _>(|k| k.line_keys[0].lamp = LampState::solid(LampColor::Green));
    r.ui.pump();
    assert!(count(&r, th.good) > 100);
    // A blinking lamp is dark for the dark part of its blinking.
    r.store.update::<Keys, _>(|k| k.line_keys[0].lamp = LampState::blinking(LampColor::Red, Blink::Slow));
    r.ui.set_fixed_time(Some((MONO + 100, UNIX)));
    r.ui.pump();
    assert!(count(&r, th.error) > 100);
    r.ui.set_fixed_time(Some((MONO + 600, UNIX)));
    r.ui.pump();
    assert_eq!(count(&r, th.error), 0);
}

#[test]
fn a_number_is_dialed_by_touching_the_dial_pad() {
    let mut r = Rig::new();
    r.tap(NEW_CALL);
    assert_eq!(r.screen(), "dialer");
    r.check_regions("dialer");
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
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Redial]);
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
fn a_ringing_call_is_answered_ignored_or_rejected() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Ringing, "1002");
    assert_eq!(r.screen(), "call");
    r.check_regions("ringing call");
    // What cannot be done to a ringing call is not a region.
    assert!(!r.has(Hit::Key(Key::Hold)));
    assert!(!r.has(KEYPAD));
    assert!(!r.has(Hit::Soft(10)));
    r.tap(Hit::Soft(2));
    assert_eq!(r.call_cmds(), vec![Command::Silence { call: id }]);
    r.tap(Hit::Soft(3));
    assert_eq!(r.call_cmds(), vec![Command::Reject { call: id, busy: true }]);
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: None }]);
    // The speaker button is the Speaker key: it answers on the speaker.
    r.tap(Hit::Key(Key::Speaker));
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: Some(AudioPath::Speaker) }]);
    // The call is gone: so is the call screen.
    r.store.update::<Calls, _>(|c| c.calls.clear());
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
}

#[test]
fn a_call_is_held_resumed_muted_and_ended() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    r.store.update::<Audio, _>(|a| a.path = AudioPath::Handset);
    r.ui.pump();
    r.check_regions("active call");
    // No softkeys: the call screen's buttons are its own.
    assert!(!r.has(Hit::Soft(1)));

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
fn the_buttons_show_what_is_on() {
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
    let _ = th;
    for key in [Key::Mute, Key::Speaker, Key::Headset, Key::Hold] {
        assert!(!on(&r, Hit::Key(key)), "{key:?}");
    }
    r.store.update::<Audio, _>(|a| {
        a.muted = true;
        a.path = AudioPath::Speaker;
    });
    r.ui.pump();
    assert!(on(&r, Hit::Key(Key::Mute)));
    assert!(on(&r, Hit::Key(Key::Speaker)));
    assert!(!on(&r, Hit::Key(Key::Headset)));
    r.store.update::<Audio, _>(|a| {
        a.muted = false;
        a.path = AudioPath::Headset;
    });
    r.set_state(id, CallState::Held { local: true, remote: false });
    assert!(!on(&r, Hit::Key(Key::Mute)));
    assert!(!on(&r, Hit::Key(Key::Speaker)));
    assert!(on(&r, Hit::Key(Key::Headset)));
    assert!(on(&r, Hit::Key(Key::Hold)));
}

#[test]
fn the_volume_is_changed_on_the_call_screen_without_a_popup() {
    let mut r = Rig::new();
    r.add_call(CallState::Active, "1002");
    let regions = r.ui.canvas().hits().len();
    r.tap(VOLUME_UP);
    r.tap(VOLUME_UP);
    r.tap(VOLUME_DOWN);
    let up = Command::AdjustVolume { target: VolumeTarget::Current, delta: 1 };
    let down = Command::AdjustVolume { target: VolumeTarget::Current, delta: -1 };
    assert_eq!(r.audio_cmds(), vec![up.clone(), up, down]);
    // Nothing came up over the screen.
    assert_eq!(r.ui.canvas().hits().len(), regions);
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
fn a_call_is_transferred_blind_and_with_consultation() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Active, "1002");
    r.tap(Hit::Key(Key::Transfer));
    assert_eq!(r.screen(), "dialer");
    r.check_regions("transfer dialer");
    // Nothing to transfer to yet.
    assert!(!r.has(Hit::Soft(1)));
    r.digits("1003");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::TransferBlind { call: id, target: "1003".into() }]);
    assert_eq!(r.screen(), "call");
    // Consult: a call to the target; with the two calls up, Transfer
    // connects them.
    r.tap(Hit::Key(Key::Transfer));
    r.digits("1004");
    r.tap(Hit::Soft(2));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "1004".into(), account: Some(AccountId(0)), path: None }]);
    r.set_state(id, CallState::Held { local: true, remote: false });
    let consult = r.add_call(CallState::Active, "1004");
    assert_eq!(r.screen(), "call");
    r.tap(Hit::Key(Key::Transfer));
    assert_eq!(r.call_cmds(), vec![Command::TransferAttended { call: id, consult }]);
}

#[test]
fn a_second_call_is_made_and_the_two_are_selected_swapped_and_joined() {
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
    // The focused call's card is first.
    assert!(r.region(card(second)).y < r.region(card(first)).y);
    // Swap focuses the other call; Join makes the conference.
    r.tap(Hit::Soft(14));
    assert_eq!(r.call_cmds(), vec![Command::Focus { call: first }]);
    r.tap(card(second));
    r.tap(Hit::Key(Key::Conference));
    assert_eq!(r.call_cmds(), vec![Command::Conference { a: second, b: first }]);
    // A touch on a card selects its call: the buttons act on it.
    r.tap(card(first));
    assert!(r.region(card(first)).y < r.region(card(second)).y);
    r.tap(Hit::Key(Key::Hold));
    assert_eq!(r.call_cmds(), vec![Command::Resume { call: first }]);
    r.tap(Hit::Soft(10));
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: first }]);
    r.tap(card(second));
    r.tap(Hit::Soft(10));
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: second }]);
    // In the conference, Split takes its place.
    r.store.update::<Calls, _>(|c| {
        for call in &mut c.calls {
            call.state = CallState::Active;
            call.in_conference = true;
        }
    });
    r.ui.pump();
    r.tap(Hit::Soft(13));
    assert_eq!(r.call_cmds(), vec![Command::SplitConference { keep: second }]);
}

#[test]
fn the_lines_of_the_call_screen_are_the_line_keys() {
    let mut r = Rig::new();
    let id = r.add_call(CallState::Held { local: true, remote: false }, "1002");
    r.store.update::<Keys, _>(|k| k.line_keys[0].calls = vec![id]);
    r.ui.pump();
    // The line the call is on resumes it.
    r.tap(Hit::Key(Key::Line(1)));
    assert_eq!(r.call_cmds(), vec![Command::Resume { call: id }]);
    // A free line makes a new call on its account.
    r.tap(Hit::Key(Key::Line(2)));
    assert_eq!(r.screen(), "dialer");
    r.digits("8");
    r.tap(Hit::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "8".into(), account: Some(AccountId(1)), path: None }]);
}

#[test]
fn the_message_waiting_icon_is_the_messages_key() {
    let mut r = Rig::new();
    assert!(!r.has(Hit::Key(Key::Messages)));
    r.store.update::<Mwi, _>(|m| {
        m.accounts.push(MwiAccount { account: AccountId(0), waiting: true, new: 2, old: 0, urgent_new: 0, urgent_old: 0 })
    });
    r.ui.pump();
    r.check_regions("idle with messages");
    r.tap(Hit::Key(Key::Messages));
    assert_eq!(r.call_cmds(), vec![Command::DialVoicemail { account: None }]);
    let _ = Rig::drain(&r.apps);
}

#[test]
fn the_models_with_keys_keep_their_softkeys_and_register_nothing() {
    for model in [Model::D40, Model::D65, Model::D70] {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        let calls = store.handle(Domain::Calls);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
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
