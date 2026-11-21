//! Behavior tests: key dispatch, hook handling, navigation, softkey
//! paging, the app view contract, lamps, backlight, and damage tracking,
//! all through the HAL simulator.

use opendeskphone_hal::{Hal, Lamp, Screen};
use opendeskphone_state::*;
use opendeskphone_ui::appview::{self as av, AppEvent, AppView, KeyClaims, WidgetKind};
use opendeskphone_ui::{Ui, UiConfig};

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    calls: CommandReceiver,
    audio: CommandReceiver,
    apps: CommandReceiver,
    config: CommandReceiver,
}

impl Rig {
    fn new(model: Model) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        // (A phone with no account does not dial: it says that it has none.)
        store.update::<Accounts, _>(|a| a.accounts.push(Account::new(AccountId(0))));
        let calls = store.handle(Domain::Calls);
        let audio = store.handle(Domain::Audio);
        let apps = store.handle(Domain::Apps);
        let config = store.handle(Domain::Config);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.pump();
        Rig { store, ui, sim, calls, audio, apps, config }
    }
    fn tap(&mut self, key: Key) {
        self.sim.tap(key);
        self.ui.pump();
    }
    fn hook(&mut self, off: bool) {
        self.sim.set_hook(off);
        self.ui.pump();
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
    fn add_call(&mut self, state: CallState) -> CallId {
        let id = self.store.next_call_id();
        self.store.update::<Calls, _>(|c| {
            let mut call = Call::new(id, AccountId(0), Direction::Incoming, Party { number: "1002".into(), ..Default::default() });
            call.state = state;
            c.calls.push(call);
            c.focused = Some(id);
        });
        self.ui.pump();
        id
    }
    fn screen(&self) -> String {
        self.ui.screen_name()
    }
}

#[test]
fn on_hook_dialing() {
    let mut r = Rig::new(Model::D60);
    assert_eq!(r.screen(), "idle");
    for d in [1, 0, 0, 2] {
        r.tap(Key::Digit(d));
    }
    assert_eq!(r.screen(), "dialer");
    assert_eq!(r.store.get::<opendeskphone_state::Ui>().dial_buffer, "1002");
    // On-hook digits play no tone and do not dial by themselves.
    assert!(r.audio_cmds().is_empty());
    assert!(r.call_cmds().is_empty());
    r.tap(Key::Left); // backspace
    r.tap(Key::Soft(1)); // Dial
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "100".into(), account: None, path: None }]);
    assert_eq!(r.screen(), "idle");
    assert_eq!(r.store.get::<opendeskphone_state::Ui>().screen, "idle");
    // Events were re-emitted for other listeners.
}

#[test]
fn off_hook_dialing_times_out_into_a_call() {
    let mut r = Rig::new(Model::D40);
    r.hook(true);
    assert_eq!(r.screen(), "dialer");
    assert_eq!(
        r.audio_cmds(),
        vec![
            Command::SetAudioPath { path: AudioPath::Handset },
            Command::PlayTone { tone: Tone::Dial, duration_ms: 0 }
        ]
    );
    r.ui.set_fixed_time(Some((1_000_000, 1_794_607_087)));
    r.tap(Key::Digit(7));
    let a = r.audio_cmds();
    assert_eq!(a[0], Command::StopTone);
    assert_eq!(a[1], Command::PlayTone { tone: Tone::Dtmf('7'), duration_ms: 100 });
    // Before the inter-digit timeout nothing is dialed.
    r.ui.set_fixed_time(Some((1_003_000, 1_794_607_090)));
    r.ui.pump();
    assert!(r.call_cmds().is_empty());
    r.ui.set_fixed_time(Some((1_004_100, 1_794_607_091)));
    r.ui.pump();
    assert_eq!(
        r.call_cmds(),
        vec![Command::Dial { target: "7".into(), account: None, path: Some(AudioPath::Handset) }]
    );
    assert_eq!(r.screen(), "idle");
}

/// A D40 off-hook with one account (`021`) whose dial plan is `plan`,
/// at a fixed time; returns the rig and that time in milliseconds.
fn off_hook_with_plan(plan: &str) -> (Rig, u64) {
    use opendeskphone_ui::dialplan::DialPlan;
    let mut r = Rig::new(Model::D40);
    r.store.update::<Accounts, _>(|a| {
        let mut acct = Account::new(AccountId(0));
        acct.config_id = "021".into();
        acct.user = "021".into();
        a.accounts = vec![acct];
        a.default_account = Some(AccountId(0));
    });
    r.ui.handle().set_config(UiConfig {
        backlight_timeout_secs: 0,
        dial_plans: vec![("021".into(), Some(DialPlan::compile(plan).unwrap()))],
        // Another plan: the account's own is the one that counts.
        default_dial_plan: Some(DialPlan::compile("x").unwrap()),
        ..Default::default()
    });
    let t0 = 2_000_000;
    r.ui.set_fixed_time(Some((t0, 1_794_607_087)));
    r.ui.pump();
    r.hook(true);
    assert_eq!(r.screen(), "dialer");
    (r, t0)
}

fn type_digits(r: &mut Rig, digits: &str) {
    for d in digits.bytes() {
        r.tap(Key::Digit(d - b'0'));
    }
}

/// The dial plan the PBX provisions (`digit_map` of the line).
const PBX_DIAL_PLAN: &str = "0xx|[2-9]xxxxxxxxx|1[2-9]xxxxxxxxxx";

#[test]
fn the_dial_plan_starts_the_call_at_the_last_digit() {
    // An extension and a ten-digit number: complete matches.
    for number in ["021", "2565551234"] {
        let (mut r, _) = off_hook_with_plan(PBX_DIAL_PLAN);
        type_digits(&mut r, &number[..number.len() - 1]);
        assert!(r.call_cmds().is_empty(), "{number} dialed early");
        assert_eq!(r.screen(), "dialer");
        type_digits(&mut r, &number[number.len() - 1..]);
        assert_eq!(
            r.call_cmds(),
            vec![Command::Dial { target: number.into(), account: None, path: Some(AudioPath::Handset) }],
            "{number}"
        );
        assert_eq!(r.screen(), "idle");
    }
    // Digits no alternative can match are dialed at once (stock).
    let (mut r, _) = off_hook_with_plan(PBX_DIAL_PLAN);
    type_digits(&mut r, "11");
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "11".into(), account: None, path: Some(AudioPath::Handset) }]);
}

#[test]
fn eleven_digits_with_the_provisioned_plan() {
    // The provisioned `1[2-9]xxxxxxxxxx` is twelve digits long, so
    // 18004377950 is not a complete match: it is dialed by the inter-digit
    // timeout, not at its last digit.
    let (mut r, t0) = off_hook_with_plan(PBX_DIAL_PLAN);
    type_digits(&mut r, "18004377950");
    assert!(r.call_cmds().is_empty());
    assert_eq!(r.screen(), "dialer");
    r.ui.set_fixed_time(Some((t0 + 3_900, 1_794_607_090)));
    r.ui.pump();
    assert!(r.call_cmds().is_empty());
    r.ui.set_fixed_time(Some((t0 + 4_000, 1_794_607_091)));
    r.ui.pump();
    assert_eq!(
        r.call_cmds(),
        vec![Command::Dial { target: "18004377950".into(), account: None, path: Some(AudioPath::Handset) }]
    );
    assert_eq!(r.screen(), "idle");

    // With the eleven-digit alternative the call starts at the last digit.
    let (mut r, _) = off_hook_with_plan("0xx|[2-9]xxxxxxxxx|1[2-9]xxxxxxxxx");
    type_digits(&mut r, "1800437795");
    assert!(r.call_cmds().is_empty());
    type_digits(&mut r, "0");
    assert_eq!(
        r.call_cmds(),
        vec![Command::Dial { target: "18004377950".into(), account: None, path: Some(AudioPath::Handset) }]
    );
}

#[test]
fn dial_plan_timers_dial_after_their_seconds() {
    // `*xx.T3`: 3 seconds after the last key; a complete match that could
    // still grow (`1xx` against `1xxx`): 2 seconds.
    for (digits, secs) in [("172", 3u64), ("123", 2)] {
        let (mut r, t0) = off_hook_with_plan("7xx|17x.T3|12xx|123");
        type_digits(&mut r, digits);
        assert!(r.call_cmds().is_empty(), "{digits}");
        r.ui.set_fixed_time(Some((t0 + secs * 1000 - 100, 1_794_607_090)));
        r.ui.pump();
        assert!(r.call_cmds().is_empty(), "{digits}");
        r.ui.set_fixed_time(Some((t0 + secs * 1000, 1_794_607_091)));
        r.ui.pump();
        assert_eq!(
            r.call_cmds(),
            vec![Command::Dial { target: digits.into(), account: None, path: Some(AudioPath::Handset) }],
            "{digits}"
        );
    }
}

#[test]
fn the_dial_plan_does_not_dial_on_hook_digits() {
    let (mut r, _) = off_hook_with_plan(PBX_DIAL_PLAN);
    r.hook(false);
    assert_eq!(r.screen(), "idle");
    let _ = r.call_cmds();
    type_digits(&mut r, "021");
    assert_eq!(r.screen(), "dialer");
    assert!(r.call_cmds().is_empty());
}

#[test]
fn speaker_and_headset_keys_go_off_hook_on_their_path() {
    for (key, path) in [(Key::Speaker, AudioPath::Speaker), (Key::Headset, AudioPath::Headset)] {
        let mut r = Rig::new(Model::D60);
        r.tap(key);
        assert_eq!(r.screen(), "dialer", "{key:?}");
        assert_eq!(
            r.audio_cmds(),
            vec![Command::SetAudioPath { path }, Command::PlayTone { tone: Tone::Dial, duration_ms: 0 }],
            "{key:?}"
        );
    }
}

#[test]
fn hanging_up_an_empty_dialer_closes_it() {
    let mut r = Rig::new(Model::D60);
    r.hook(true);
    r.audio_cmds();
    r.hook(false);
    assert_eq!(r.screen(), "idle");
    assert_eq!(r.audio_cmds(), vec![Command::StopTone, Command::SetAudioPath { path: AudioPath::None }]);
}

#[test]
fn incoming_call_keys() {
    let mut r = Rig::new(Model::D60);
    // Bury the idle screen under a menu: a ringing call still comes up.
    r.tap(Key::Select);
    assert_eq!(r.screen(), "menu");
    let id = r.add_call(CallState::Ringing);
    assert_eq!(r.screen(), "call");
    r.tap(Key::Soft(2)); // Ignore
    assert_eq!(r.call_cmds(), vec![Command::Silence { call: id }]);
    r.tap(Key::Cancel); // stock: Cancel rejects
    assert_eq!(r.call_cmds(), vec![Command::Reject { call: id, busy: true }]);
    r.tap(Key::Speaker);
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: Some(AudioPath::Speaker) }]);
    r.hook(true);
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: Some(AudioPath::Handset) }]);
    // Stock layout: Answer, Ignore, Transfer, and an empty fourth key.
    r.tap(Key::Soft(4));
    assert!(r.call_cmds().is_empty());
    assert_eq!(r.screen(), "call");
    r.tap(Key::Soft(3)); // Transfer the ringing call
    assert_eq!(r.screen(), "dialer");
    r.tap(Key::Cancel);
    assert_eq!(r.screen(), "call");
    // The call ends: back to where the user was (the menu was replaced by
    // the call; the stack below the call screen is kept).
    r.store.update::<Calls, _>(|c| c.calls.clear());
    r.ui.pump();
    assert_eq!(r.screen(), "menu");
}

#[test]
fn active_call_keys() {
    let mut r = Rig::new(Model::D50);
    let id = r.add_call(CallState::Active);
    r.store.update::<Audio, _>(|a| a.path = AudioPath::Handset);
    r.hook(true);
    r.call_cmds();
    r.audio_cmds();
    r.tap(Key::Digit(5));
    assert_eq!(r.call_cmds(), vec![Command::Dtmf { call: id, digit: '5' }]);
    r.tap(Key::Hold);
    assert_eq!(r.call_cmds(), vec![Command::Hold { call: id }]);
    r.tap(Key::Mute);
    assert_eq!(r.audio_cmds(), vec![Command::SetMute { on: true }]);
    r.tap(Key::VolumeUp);
    assert_eq!(r.audio_cmds(), vec![Command::AdjustVolume { target: VolumeTarget::Current, delta: 1 }]);
    r.tap(Key::Speaker);
    assert_eq!(r.audio_cmds(), vec![Command::SetAudioPath { path: AudioPath::Speaker }]);
    // Transfer opens the transfer dialer; its first softkey is a blind
    // transfer.
    r.tap(Key::Transfer);
    assert_eq!(r.screen(), "dialer");
    r.tap(Key::Digit(9));
    r.tap(Key::Soft(1));
    assert_eq!(r.call_cmds(), vec![Command::TransferBlind { call: id, target: "9".into() }]);
    assert_eq!(r.screen(), "call");
    // With a second call, the Conference key joins them.
    let second = r.add_call(CallState::Active);
    r.store.update::<Calls, _>(|c| c.get_mut(id).unwrap().state = CallState::Held { local: true, remote: false });
    r.ui.pump();
    r.tap(Key::Conference);
    assert_eq!(r.call_cmds(), vec![Command::Conference { a: second, b: id }]);
    r.tap(Key::Transfer);
    assert_eq!(r.call_cmds(), vec![Command::TransferAttended { call: id, consult: second }]);
    // Hanging up the handset ends the call on the handset.
    r.hook(false);
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: second }]);
    // On the speaker, the hook does nothing and the speaker key hangs up.
    r.store.update::<Audio, _>(|a| a.path = AudioPath::Speaker);
    r.hook(true);
    r.audio_cmds();
    r.hook(false);
    assert!(r.call_cmds().is_empty());
    r.tap(Key::Speaker);
    assert_eq!(r.call_cmds(), vec![Command::Hangup { call: second }]);
}

#[test]
fn idle_softkeys_follow_the_configuration() {
    use opendeskphone_ui::IdleSoftkey;
    let mut r = Rig::new(Model::D40);
    // The default: Call Log, Contacts, Status; Menu, Apps, DND; Forward.
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(1));
    assert_eq!(r.screen(), "forward");
    r.tap(Key::Cancel);
    r.tap(Key::Redial);
    assert_eq!(r.call_cmds(), vec![Command::Redial]);

    // A configured list: functions and apps.  An app the phone does not
    // have (absent, or switched off) gets no softkey, so four keys are
    // left, and there is no "More..." page.
    r.store.update::<Apps, _>(|a| a.installed.push(AppInfo { name: "parking".into(), ..Default::default() }));
    let app = |name: &str| IdleSoftkey::App(name.into());
    let keys = vec![app("parking"), app("calendar"), IdleSoftkey::Redial, IdleSoftkey::Dnd, IdleSoftkey::Menu];
    r.ui.handle().set_config(UiConfig { backlight_timeout_secs: 0, idle_softkeys: keys, ..Default::default() });
    r.ui.pump();
    r.tap(Key::Soft(1));
    assert_eq!(Rig::drain(&r.apps), vec![Command::LaunchApp { name: "parking".into() }]);
    r.tap(Key::Soft(2));
    r.tap(Key::Soft(3));
    assert_eq!(r.call_cmds(), vec![Command::Redial, Command::SetDnd { on: true }]);
    r.tap(Key::Soft(4));
    assert_eq!(r.screen(), "menu");
    r.tap(Key::Cancel);
    // The app arrives: its softkey is second, and the five page.
    r.store.update::<Apps, _>(|a| a.installed.push(AppInfo { name: "calendar".into(), ..Default::default() }));
    r.ui.pump();
    r.tap(Key::Soft(2));
    assert_eq!(Rig::drain(&r.apps), vec![Command::LaunchApp { name: "calendar".into() }]);
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(2));
    assert_eq!(r.screen(), "menu");
}

#[test]
fn menu_shortcuts_and_settings() {
    let mut r = Rig::new(Model::D60);
    // Idle softkeys page behind "More...": Call Log, Contacts, Status,
    // then Menu, Apps, DND.
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(1)); // Menu
    assert_eq!(r.screen(), "menu");
    r.tap(Key::Digit(4)); // Do Not Disturb toggles in place
    assert_eq!(r.call_cmds(), vec![Command::SetDnd { on: true }]);
    r.tap(Key::Digit(3)); // Call Forward
    assert_eq!(r.screen(), "forward");
    r.tap(Key::Select); // Always
    for d in [1, 0, 9, 9] {
        r.tap(Key::Digit(d));
    }
    r.tap(Key::Select);
    assert_eq!(
        r.call_cmds(),
        vec![Command::SetForward { kind: ForwardKind::Always, target: "1099".into(), no_answer_secs: None }]
    );
    assert_eq!(r.screen(), "forward");
    r.tap(Key::Cancel);
    r.tap(Key::Cancel);
    assert_eq!(r.screen(), "idle");
    // Settings write stock setting IDs.
    r.tap(Key::Select);
    r.tap(Key::Digit(6)); // Settings
    r.tap(Key::Digit(1)); // Network
    r.tap(Key::Digit(1)); // Addressing: DHCP -> static
    let cmds = Rig::drain(&r.config);
    assert_eq!(cmds, vec![Command::SetSettings { settings: vec![("network_enable_dhcp".into(), "0".into())] }]);
    // Brightness adjusts live and is persisted when the screen closes.
    let p = r.store.send(Command::ShowScreen { name: "settings".into() });
    r.ui.pump();
    p.wait().unwrap();
    r.tap(Key::Digit(3));
    r.tap(Key::Digit(1));
    r.tap(Key::Right);
    r.ui.pump();
    assert_eq!(r.sim.brightness(Screen::Main), Some(20)); // the stock level of brightness 6
    assert!(Rig::drain(&r.config).is_empty());
    r.tap(Key::Cancel);
    assert_eq!(Rig::drain(&r.config), vec![Command::SetSettings { settings: vec![("brightness".into(), "6".into())] }]);
}

#[test]
fn injected_keys_and_events() {
    let mut r = Rig::new(Model::D60);
    let sub = r.store.subscribe_events(TopicSet::NONE);
    let p = r.store.send(Command::InjectKey { key: Key::Digit(4), down: true });
    r.ui.pump();
    assert_eq!(p.wait(), Ok(Reply::Done));
    assert_eq!(r.screen(), "dialer");
    let wake = sub.try_take().unwrap();
    assert!(wake.events.contains(&Event::Key { key: Key::Digit(4), down: true, source: InputSource::Injected }));
    let p = r.store.send(Command::ShowScreen { name: "nonsense".into() });
    r.ui.pump();
    assert!(matches!(p.wait(), Err(CommandError::Invalid(_))));
}

/// The Messages key opens the voicemail app only where the app has the
/// application account it reaches the server with; otherwise it calls the
/// voicemail number of an account that has one, and a phone with neither
/// says so.
#[test]
fn messages_key_opens_the_voicemail_app_or_calls_the_voicemail_number() {
    let mut r = Rig::new(Model::D60);
    r.tap(Key::Messages);
    assert!(r.call_cmds().is_empty());
    let notices = r.store.get::<opendeskphone_state::Ui>().notices.clone();
    assert_eq!(notices.len(), 1, "no voicemail number: {notices:?}");
    r.tap(Key::Select);
    r.store.update::<Accounts, _>(|a| a.accounts[0].voicemail_number = "*97".into());
    r.tap(Key::Messages);
    assert_eq!(r.call_cmds(), vec![Command::DialVoicemail { account: Some(AccountId(0)) }]);
    // The app without its account cannot reach the server: the number.
    r.store.update::<Apps, _>(|a| a.installed.push(AppInfo { name: "voicemail".into(), ..Default::default() }));
    r.tap(Key::Messages);
    assert_eq!(r.call_cmds(), vec![Command::DialVoicemail { account: Some(AccountId(0)) }]);
    assert!(Rig::drain(&r.apps).is_empty());
    r.ui.handle().set_config(UiConfig { backlight_timeout_secs: 0, voicemail_app_account: true, ..Default::default() });
    r.ui.pump();
    r.tap(Key::Messages);
    assert_eq!(Rig::drain(&r.apps), vec![Command::LaunchApp { name: "voicemail".into() }]);
    assert!(r.call_cmds().is_empty());
}

/// A phone with no account does not dial: it says why.
#[test]
fn dialing_with_no_account_raises_a_notice() {
    let mut r = Rig::new(Model::D60);
    r.store.update::<Accounts, _>(|a| a.accounts.clear());
    r.ui.pump();
    for d in [4, 5] {
        r.tap(Key::Digit(d));
    }
    r.tap(Key::Soft(1));
    assert!(r.call_cmds().is_empty());
    assert_eq!(r.screen(), "idle");
    let notices = r.store.get::<opendeskphone_state::Ui>().notices.clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].text.contains("no account"), "{notices:?}");
}

#[test]
fn rapid_dial_keys_dial_or_pick_up() {
    let mut r = Rig::new(Model::D70);
    r.store.update::<Presence, _>(|p| {
        p.entries.push(BlfEntry { id: "a".into(), number: "201".into(), state: BlfState::Idle, ..Default::default() });
        // Both ring; only "b" is a contact whose calls may be picked up.
        p.entries.push(BlfEntry {
            id: "b".into(),
            number: "202".into(),
            state: BlfState::Ringing,
            pickup_ringing: true,
            ..Default::default()
        });
        p.entries.push(BlfEntry { id: "c".into(), number: "203".into(), state: BlfState::Ringing, ..Default::default() });
    });
    r.store.update::<Keys, _>(|k| {
        k.rapid_pages = 2;
        for (i, e) in ["a", "b", "c"].into_iter().enumerate() {
            k.rapid_dial.push(RapidDialKey {
                index: i as u8,
                page: 0,
                entry: e.into(),
                label: e.into(),
                number: String::new(),
                lamp: LampState::solid(LampColor::Green),
            });
        }
    });
    r.ui.pump();
    r.tap(Key::Blf(1));
    r.tap(Key::Blf(2));
    r.tap(Key::Blf(3));
    r.tap(Key::BlfPageRight);
    assert_eq!(
        r.call_cmds(),
        vec![
            Command::Dial { target: "201".into(), account: None, path: None },
            Command::Pickup { blf_entry: "b".into() },
            Command::Dial { target: "203".into(), account: None, path: None },
            Command::SetRapidDialPage { page: 1 },
        ]
    );
    // Lamps follow the key state; the side display was drawn.
    assert_eq!(r.sim.lamp(Lamp::Blf(0)), LampColor::Green);
    assert_eq!(r.sim.lamp(Lamp::Blf(5)), LampColor::Off);
    assert!(r.sim.frame(Screen::Side).is_some());
}

#[test]
fn lamps_follow_key_state() {
    let mut r = Rig::new(Model::D60);
    r.store.update::<Keys, _>(|k| {
        k.line_keys.push(LineKey {
            index: 0,
            binding: LineKeyBinding::Account(AccountId(0)),
            label: "100".into(),
            lamp: LampState::solid(LampColor::Green),
            calls: vec![],
        });
        k.message_lamp = LampState::solid(LampColor::Red);
        k.mute_lamp = LampState::solid(LampColor::Red);
    });
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Line(0)), LampColor::Green);
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Red);
    assert_eq!(r.sim.lamp(Lamp::Mute), LampColor::Red);
    assert_eq!(r.sim.lamp(Lamp::Speaker), LampColor::Off);
    r.store.update::<Keys, _>(|k| k.message_lamp = LampState::OFF);
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Off);
}

#[test]
fn app_view_contract() {
    let mut r = Rig::new(Model::D60);
    let (view, events) = AppView::new("demo", Model::D60);
    let m = view.metrics();
    assert_eq!((m.width, m.height, m.row_height), (480, 243, 32));
    let group = view.create(WidgetKind::Group, None, av::Rect::new(0, 0, m.width, m.height));
    let list = view.create(WidgetKind::List, Some(group), av::Rect::new(0, 0, m.width, 100));
    view.list_set_rows(list, (0..5).map(|i| vec![av::Cell::text(format!("row {i}"))]).collect());
    let input = view.create(WidgetKind::Input, Some(group), av::Rect::new(0, 110, m.width, 36));
    view.focus(Some(list));
    view.set_softkeys(None, (1..=6).map(|i| av::Softkey::new(i, format!("A{i}"))).collect());
    view.set_softkeys(Some(group), vec![av::Softkey::new(100, "Group")]);
    r.ui.handle().open_app(view.clone());
    r.ui.pump();
    assert_eq!(r.screen(), "app:demo");
    assert_eq!(r.store.get::<opendeskphone_state::Ui>().screen, "app:demo");
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));

    // List navigation and activation.
    r.tap(Key::Down);
    r.tap(Key::Select);
    assert_eq!(events.try_recv(), Some(AppEvent::ListSelected { widget: list, row: 1 }));
    assert_eq!(events.try_recv(), Some(AppEvent::ListActivated { widget: list, row: 1 }));
    r.tap(Key::Up);
    r.tap(Key::Up); // wraps
    assert_eq!(view.list_selected(list), Some(4));
    while events.try_recv().is_some() {}

    // Softkeys resolve by walking up from the focused widget: the group's
    // set wins over the window's.
    r.tap(Key::Soft(1));
    assert_eq!(events.try_recv(), Some(AppEvent::Softkey { id: 100 }));
    // Without the group's set, the window's six actions page behind More.
    view.set_softkeys(Some(group), vec![]);
    r.tap(Key::Soft(3));
    assert_eq!(events.try_recv(), Some(AppEvent::Softkey { id: 3 }));
    r.tap(Key::Soft(4)); // More
    r.tap(Key::Soft(1));
    assert_eq!(events.try_recv(), Some(AppEvent::Softkey { id: 4 }));

    // Claims: the group claims Right; the window claims the digits.
    view.set_key_claims(group, KeyClaims::keys([Key::Right]));
    view.set_window_key_claims(KeyClaims { digits: true, ..Default::default() });
    r.tap(Key::Right);
    r.tap(Key::Digit(3));
    assert_eq!(events.try_recv(), Some(AppEvent::Key { widget: Some(group), key: Key::Right }));
    assert_eq!(events.try_recv(), Some(AppEvent::Key { widget: None, key: Key::Digit(3) }));
    view.set_window_key_claims(KeyClaims::none());

    // Input: multi-tap text, softkey 3 deletes, Select submits.
    view.focus(Some(input));
    view.set_softkeys(None, vec![av::Softkey::new(1, "OK")]);
    r.tap(Key::Digit(2));
    r.tap(Key::Digit(2)); // a -> b
    r.tap(Key::Digit(3)); // d
    assert_eq!(view.text(input).as_deref(), Some("bd"));
    r.tap(Key::Soft(3)); // Delete
    assert_eq!(view.text(input).as_deref(), Some("b"));
    r.tap(Key::Select);
    let got: Vec<AppEvent> = std::iter::from_fn(|| events.try_recv()).collect();
    assert_eq!(
        got,
        vec![
            AppEvent::InputChanged { widget: input, value: "a".into() },
            AppEvent::InputChanged { widget: input, value: "b".into() },
            AppEvent::InputChanged { widget: input, value: "bd".into() },
            AppEvent::InputChanged { widget: input, value: "b".into() },
            AppEvent::InputSubmitted { widget: input, value: "b".into() },
        ]
    );
    // Up from the input moves focus to the list.
    r.tap(Key::Up);
    assert_eq!(view.focused(), Some(list));
    assert_eq!(events.try_recv(), Some(AppEvent::Focused { widget: list }));

    // Unclaimed Cancel asks the app to close; volume keys never reach it.
    r.tap(Key::Cancel);
    assert_eq!(events.try_recv(), Some(AppEvent::CloseRequested));
    r.tap(Key::VolumeDown);
    assert_eq!(events.try_recv(), None);
    assert_eq!(r.audio_cmds(), vec![Command::AdjustVolume { target: VolumeTarget::Current, delta: -1 }]);

    // An incoming call covers the app; ending it brings the app back.
    let _ = r.add_call(CallState::Ringing);
    assert_eq!(r.screen(), "call");
    assert_eq!(events.try_recv(), Some(AppEvent::Background));
    r.store.update::<Calls, _>(|c| c.calls.clear());
    r.ui.pump();
    assert_eq!(r.screen(), "app:demo");
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));

    // Line 1 backgrounds a nonprivileged app (stock rule).
    r.tap(Key::Line(1));
    assert_eq!(r.screen(), "idle");
    assert_eq!(events.try_recv(), Some(AppEvent::Background));

    // Destroying a widget destroys its children and drops focus.
    view.destroy(group);
    assert!(!view.exists(list) && !view.exists(input));
    assert_eq!(view.focused(), None);
}

#[test]
fn damage_is_tracked() {
    let mut r = Rig::new(Model::D40);
    r.ui.pump();
    let g0 = r.sim.frame_generation();
    // Nothing changed: no flush.
    r.ui.pump();
    assert_eq!(r.sim.frame_generation(), g0);
    // A state change that alters pixels flushes once.
    r.store.update::<Features, _>(|f| f.dnd = true);
    r.ui.pump();
    assert!(r.sim.frame_generation() > g0);
    // The gray panel only ever receives the four levels.
    let frame = r.sim.frame(Screen::Main).unwrap();
    assert!(frame.pixels.iter().all(|p| matches!(p, 0x00 | 0x55 | 0xAA | 0xFF)));
}

#[test]
fn backlight_dims_and_wakes() {
    let model = Model::D60;
    let mut hal = Hal::sim(model);
    let sim = hal.sim.clone().unwrap();
    let store = Store::new(model);
    store.update::<System, _>(|s| s.phase = Phase::Running);
    let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 1, brightness: 10, ..Default::default() });
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(32));
    assert!(store.get::<opendeskphone_state::Ui>().backlight_on);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(6)); // the stock level of dim level 2
    sim.tap(Key::Up);
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(32));
    // Dim level 0 is off.
    ui.handle().set_config(UiConfig { backlight_timeout_secs: 1, brightness: 10, backlight_dim_level: 0, ..Default::default() });
    ui.pump();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(0));
    assert!(!store.get::<opendeskphone_state::Ui>().backlight_on);
    // With dimming off the backlight is lit again, as in stock.
    ui.handle().set_config(UiConfig { backlight_timeout_secs: 1, brightness: 10, dim_backlight: false, ..Default::default() });
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(32));
    // A ringing call wakes a dimmed backlight.
    ui.handle().set_config(UiConfig { backlight_timeout_secs: 1, brightness: 10, ..Default::default() });
    ui.pump();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(6));
    store.update::<Calls, _>(|c| {
        let mut call = Call::new(CallId(1), AccountId(0), Direction::Incoming, Party::default());
        call.state = CallState::Ringing;
        c.calls.push(call);
    });
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(32));
}

#[test]
fn clock_and_call_timer_repaint_on_their_own() {
    let mut r = Rig::new(Model::D60);
    // 21:58:07 UTC: the idle screen shows 9:58.
    r.ui.set_fixed_time(Some((1_000_000, 1_794_607_087)));
    r.ui.pump();
    let g0 = r.sim.frame_generation();
    // Same minute: nothing to repaint.
    r.ui.set_fixed_time(Some((1_010_000, 1_794_607_097)));
    r.ui.pump();
    let before = r.sim.frame(Screen::Main).unwrap();
    // The next minute: the clock changes without any input.
    r.ui.set_fixed_time(Some((1_060_000, 1_794_607_147)));
    r.ui.pump();
    let after = r.sim.frame(Screen::Main).unwrap();
    assert!(r.sim.frame_generation() > g0);
    assert_ne!(before.pixels, after.pixels);
}

#[test]
fn run_loop_on_its_own_thread() {
    let model = Model::D60;
    let mut hal = Hal::sim(model);
    let sim = hal.sim.clone().unwrap();
    let store = Store::new(model);
    store.update::<System, _>(|s| s.phase = Phase::Running);
    let ui = Ui::from_hal(&mut hal, store.clone(), UiConfig::default());
    let handle = ui.handle();
    let thread = std::thread::spawn(move || ui.run());
    let wait_screen = |want: &str| {
        for _ in 0..200 {
            if store.get::<opendeskphone_state::Ui>().screen == want {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("screen never became {want:?}: {:?}", store.get::<opendeskphone_state::Ui>().screen);
    };
    wait_screen("idle");
    // A physical key through the blocking input thread.
    sim.tap(Key::Digit(1));
    wait_screen("dialer");
    // A command through the command thread.
    assert_eq!(store.send(Command::ShowScreen { name: "menu".into() }).wait(), Ok(Reply::Done));
    wait_screen("menu");
    // A state change.
    store.update::<Calls, _>(|c| {
        let mut call = Call::new(CallId(7), AccountId(0), Direction::Incoming, Party::default());
        call.state = CallState::Ringing;
        c.calls.push(call);
    });
    wait_screen("call");
    assert!(store.last_heartbeat(Subsystem::Ui).is_some());
    handle.quit();
    thread.join().unwrap();
}

#[test]
fn paging_screen_pages_a_group_and_stops_a_page() {
    let mut r = Rig::new(Model::D60);
    let paging = r.store.handle(Domain::Paging);
    r.store.update::<Paging, _>(|p| {
        p.broadcasts = vec![PageGroup { id: "dock".into(), name: "Loading dock".into(), addr: "239.1.1.1".into(), port: 5000 }];
    });
    let shown = r.store.send(Command::ShowScreen { name: "paging".into() });
    r.ui.pump();
    assert_eq!(shown.wait(), Ok(Reply::Done));
    assert_eq!(r.screen(), "paging");
    // Selecting the group pages it.
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&paging), [Command::StartPage { id: "dock".into() }]);
    // While a page is in progress the first row stops it, whether this
    // phone is paging or listening.
    for activity in [
        PageActivity::Sending { group: "dock".into(), name: "Loading dock".into() },
        PageActivity::Receiving { group: "1".into(), name: "Everyone".into() },
    ] {
        r.store.update::<Paging, _>(|p| p.activity = activity.clone());
        r.ui.pump();
        r.tap(Key::Select);
        assert_eq!(Rig::drain(&paging), [Command::StopPage]);
    }
}

/// A D65 with a Bluetooth controller in the state `f` makes, showing the
/// Bluetooth application; returns the rig and the Bluetooth domain's
/// commands, with the search the application starts already taken.
fn bluetooth_rig(f: impl FnOnce(&mut Bluetooth)) -> (Rig, CommandReceiver) {
    let mut r = Rig::new(Model::D65);
    let bt = r.store.handle(Domain::Bluetooth);
    r.store.update::<Bluetooth, _>(|b| {
        b.present = true;
        f(b);
    });
    let shown = r.store.send(Command::ShowScreen { name: "bluetooth".into() });
    r.ui.pump();
    assert_eq!(shown.wait(), Ok(Reply::Done));
    assert_eq!(r.screen(), "bluetooth");
    Rig::drain(&bt);
    (r, bt)
}

fn bt_device(addr: &str, name: &str) -> BluetoothDevice {
    BluetoothDevice { addr: addr.into(), name: name.into(), rssi: -60 }
}

const HEADSET: &str = "00:1B:DC:0F:A1:02";

#[test]
fn bluetooth_application_searches_when_opened() {
    let mut r = Rig::new(Model::D65);
    let bt = r.store.handle(Domain::Bluetooth);
    r.store.update::<Bluetooth, _>(|b| b.present = true);
    let shown = r.store.send(Command::ShowScreen { name: "bluetooth".into() });
    r.ui.pump();
    assert_eq!(shown.wait(), Ok(Reply::Done));
    assert_eq!(Rig::drain(&bt), [Command::BluetoothDiscovery { on: true }]);
}

#[test]
fn bluetooth_application_searches_again_with_the_search_softkey() {
    // The search ended and found nothing.
    let (mut r, bt) = bluetooth_rig(|_| {});
    r.tap(Key::Soft(1));
    assert_eq!(Rig::drain(&bt), [Command::BluetoothDiscovery { on: true }]);
}

#[test]
fn bluetooth_application_pairs_with_the_device_selected() {
    let (mut r, bt) = bluetooth_rig(|b| b.found = vec![bt_device("5C:F3:70:11:22:33", "Voyager"), bt_device(HEADSET, "Evolve")]);
    r.tap(Key::Down);
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothDiscovery { on: false }, Command::BluetoothPair { addr: HEADSET.into() }]);
}

#[test]
fn bluetooth_highlight_stays_on_its_device_as_others_are_found() {
    let (mut r, bt) = bluetooth_rig(|b| b.found = vec![bt_device(HEADSET, "Evolve")]);
    // A stronger device comes first.
    r.store.update::<Bluetooth, _>(|b| b.found.insert(0, bt_device("5C:F3:70:11:22:33", "Voyager")));
    r.ui.pump();
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothDiscovery { on: false }, Command::BluetoothPair { addr: HEADSET.into() }]);
}

#[test]
fn bluetooth_application_connects_and_disconnects_a_paired_headset() {
    let (mut r, bt) = bluetooth_rig(|b| b.paired = vec![bt_device(HEADSET, "Evolve")]);
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothConnect { addr: HEADSET.into() }]);
    r.store.update::<Bluetooth, _>(|b| b.connected = HEADSET.into());
    r.ui.pump();
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothDisconnect]);
}

#[test]
fn bluetooth_application_forgets_a_paired_headset_once_confirmed() {
    let (mut r, bt) = bluetooth_rig(|b| b.paired = vec![bt_device(HEADSET, "Evolve")]);
    r.tap(Key::Soft(3)); // Forget
    assert_eq!(r.screen(), "confirm");
    r.tap(Key::Soft(1)); // Yes
    assert_eq!(Rig::drain(&bt), [Command::BluetoothUnpair { addr: HEADSET.into() }]);
    assert_eq!(r.screen(), "bluetooth");
}

#[test]
fn bluetooth_pairing_question_is_answered_with_select_or_cancel() {
    // A headset asks to pair, with six digits to compare.
    let question = BluetoothPairing { addr: HEADSET.into(), name: "Evolve".into(), passkey: Some(42), pin_needed: false };
    let (mut r, bt) = bluetooth_rig(|b| b.pairing = Some(question));
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothAnswerPairing { addr: HEADSET.into(), accept: true, pin: None }]);
    r.tap(Key::Cancel);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothAnswerPairing { addr: HEADSET.into(), accept: false, pin: None }]);
}

#[test]
fn bluetooth_pin_is_typed_for_a_device_that_needs_one() {
    let question = BluetoothPairing { addr: HEADSET.into(), name: "Evolve".into(), passkey: None, pin_needed: true };
    let (mut r, bt) = bluetooth_rig(|b| b.pairing = Some(question));
    for d in [1, 2, 3, 5] {
        r.tap(Key::Digit(d));
    }
    r.tap(Key::Left);
    r.tap(Key::Digit(4));
    r.tap(Key::Select);
    assert_eq!(Rig::drain(&bt), [Command::BluetoothAnswerPairing { addr: HEADSET.into(), accept: true, pin: Some("1234".into()) }]);
}

/// A view with one text widget, for the idle screen.
fn idle_view(name: &str, model: Model) -> (opendeskphone_ui::AppView, opendeskphone_ui::AppEvents) {
    let (view, events) = opendeskphone_ui::AppView::new(name, model);
    let line = view.create(opendeskphone_ui::appview::WidgetKind::Text, None, opendeskphone_ui::appview::Rect::new(0, 0, 100, 16));
    view.set_text(line, name);
    (view, events)
}

fn lifecycle(events: &opendeskphone_ui::AppEvents) -> Vec<opendeskphone_ui::AppEvent> {
    std::iter::from_fn(|| events.try_recv()).collect()
}

#[test]
fn idle_views_are_told_when_the_idle_screen_shows_and_hides() {
    use opendeskphone_ui::AppEvent::{Background, Foreground};
    let mut r = Rig::new(Model::D60);
    let (view, events) = idle_view("status", Model::D60);
    r.ui.handle().add_idle_view(view.clone());
    r.ui.pump();
    assert_eq!(lifecycle(&events), [Foreground], "the idle screen is showing");
    // A menu covers the idle screen; leaving it uncovers it.
    r.tap(Key::Menu);
    assert_eq!(lifecycle(&events), [Background]);
    r.tap(Key::Cancel);
    assert_eq!(r.screen(), "idle");
    assert_eq!(lifecycle(&events), [Foreground]);
    // So does a call.
    let call = r.add_call(CallState::Ringing);
    assert_eq!(lifecycle(&events), [Background]);
    r.store.update::<Calls, _>(|c| c.calls.retain(|x| x.id != call));
    r.ui.pump();
    assert_eq!(lifecycle(&events), [Foreground]);
    // An app's hidden call has no screen: the idle screen stays.
    r.store.update::<Calls, _>(|c| {
        let mut hidden = Call::new(CallId(900), AccountId(0), Direction::Incoming, Party::default());
        hidden.hidden = true;
        c.calls.push(hidden);
    });
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
    assert_eq!(lifecycle(&events), []);
    // A view with nothing in it is not shown; withdrawn, it hears nothing more.
    view.clear();
    r.ui.pump();
    assert_eq!(lifecycle(&events), [Background]);
    r.ui.handle().remove_idle_view("status");
    r.ui.pump();
    assert_eq!(lifecycle(&events), []);
}

#[test]
fn several_idle_views_take_turns() {
    use opendeskphone_ui::AppEvent::{Background, Foreground};
    let model = Model::D60;
    let mut hal = Hal::sim(model);
    let store = Store::new(model);
    store.update::<System, _>(|s| s.phase = Phase::Running);
    let cfg = UiConfig { backlight_timeout_secs: 0, idle_cycle_secs: 1, ..Default::default() };
    let mut ui = Ui::from_hal(&mut hal, store.clone(), cfg.clone());
    ui.pump();
    let (a, a_events) = idle_view("a", model);
    let (b, b_events) = idle_view("b", model);
    let (empty, empty_events) = opendeskphone_ui::AppView::new("empty", model);
    ui.handle().add_idle_view(a);
    ui.handle().add_idle_view(empty);
    ui.handle().add_idle_view(b);
    ui.pump();
    assert_eq!((lifecycle(&a_events), lifecycle(&b_events)), (vec![Foreground], vec![]));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ui.pump();
    assert_eq!((lifecycle(&a_events), lifecycle(&b_events)), (vec![Background], vec![Foreground]));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ui.pump();
    assert_eq!((lifecycle(&a_events), lifecycle(&b_events)), (vec![Foreground], vec![Background]));
    // A cycle time of 0 shows no idle view at all.
    ui.handle().set_config(UiConfig { idle_cycle_secs: 0, ..cfg });
    ui.pump();
    assert_eq!((lifecycle(&a_events), lifecycle(&b_events)), (vec![Background], vec![]));
    assert_eq!(lifecycle(&empty_events), [], "a view without widgets never shows");
}

#[test]
fn an_app_can_keep_the_backlight_on() {
    let model = Model::D60;
    let mut hal = Hal::sim(model);
    let sim = hal.sim.clone().unwrap();
    let store = Store::new(model);
    store.update::<System, _>(|s| s.phase = Phase::Running);
    let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 1, brightness: 10, ..Default::default() });
    ui.pump();
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(600));
        store.post(Command::ResetIdleTimer);
        ui.pump();
        assert_eq!(sim.brightness(Screen::Main), Some(32), "kept awake past the 1 s timeout");
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(6));
    store.post(Command::ResetIdleTimer);
    ui.pump();
    assert_eq!(sim.brightness(Screen::Main), Some(32), "and woken");
}

/// A view that captures input gets every key and hookswitch change, and
/// the phone does nothing of its own with them; Cancel three times in a
/// row still leaves the app.  The phone has no test screens of its own.
#[test]
fn a_capturing_app_gets_every_key_and_the_hook() {
    let mut r = Rig::new(Model::D60);
    let (view, events) = AppView::new("diagnostics", Model::D60);
    r.ui.handle().open_app(view.clone());
    r.ui.pump();
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));
    let key = |key| Some(AppEvent::Key { widget: None, key });

    // Without capture the phone keeps these.
    r.tap(Key::VolumeUp);
    assert_eq!(events.try_recv(), None);
    assert!(matches!(r.audio_cmds()[..], [Command::AdjustVolume { .. }]));

    view.set_capture(true);
    for k in [Key::VolumeUp, Key::Line(1), Key::Mute, Key::Speaker, Key::Soft(1), Key::Digit(5), Key::Cancel, Key::Cancel, Key::Hold] {
        r.tap(k);
        assert_eq!(events.try_recv(), key(k));
    }
    r.hook(true);
    assert_eq!(events.try_recv(), Some(AppEvent::Hook { off_hook: true }));
    r.hook(false);
    assert_eq!(events.try_recv(), Some(AppEvent::Hook { off_hook: false }));
    assert_eq!(r.screen(), "app:diagnostics");
    assert_eq!((r.audio_cmds(), r.call_cmds()), (vec![], vec![]), "the phone did nothing with them");

    // Cancel, Cancel, Cancel: the third leaves the app.
    r.tap(Key::Cancel);
    r.tap(Key::Cancel);
    assert_eq!(r.screen(), "app:diagnostics");
    r.tap(Key::Cancel);
    assert_eq!(r.screen(), "idle");
    assert_eq!(events.try_recv(), key(Key::Cancel));
    assert_eq!(events.try_recv(), key(Key::Cancel));
    assert_eq!(events.try_recv(), Some(AppEvent::Background));

    // Under another screen the view captures nothing, and without
    // capture keys go the usual way again.
    r.tap(Key::VolumeUp);
    assert_eq!(events.try_recv(), None);
    r.ui.handle().open_app(view.clone());
    r.ui.pump();
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));
    view.set_capture(false);
    r.tap(Key::Line(1));
    assert_eq!(r.screen(), "idle");

    for name in ["key_test", "lamp_test", "display_test", "diagnostics"] {
        let p = r.store.send(Command::ShowScreen { name: name.into() });
        r.ui.pump();
        assert!(p.wait().is_err(), "{name}");
    }
}

/// The lamps an app's view sets show its colors while the view is the
/// top screen, and the phone's otherwise.
#[test]
fn an_app_sets_lamps_while_it_is_on_screen() {
    use opendeskphone_hal::Lamp;
    use opendeskphone_state::{Keys, LampColor, LampState};
    let mut r = Rig::new(Model::D60);
    r.store.update::<Keys, _>(|k| k.message_lamp = LampState::solid(LampColor::Red));
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Red);

    let (view, _events) = AppView::new("diagnostics", Model::D60);
    view.set_lamp(Lamp::Message, LampColor::Green);
    view.set_lamp(Lamp::Line(0), LampColor::Amber);
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Red, "the view is not on screen");

    r.ui.handle().open_app(view.clone());
    r.ui.pump();
    assert_eq!((r.sim.lamp(Lamp::Message), r.sim.lamp(Lamp::Line(0))), (LampColor::Green, LampColor::Amber));
    assert_eq!(r.sim.lamp(Lamp::Speaker), LampColor::Off);
    // A change shows without a key.
    view.set_lamp(Lamp::Line(0), LampColor::Off);
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Line(0)), LampColor::Off);

    // Released, or off the screen: the phone's lamps.
    view.release_lamps();
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Red);
    view.set_lamp(Lamp::Message, LampColor::Green);
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Green);
    r.ui.handle().close_app("diagnostics");
    r.ui.pump();
    assert_eq!(r.sim.lamp(Lamp::Message), LampColor::Red);
}

#[test]
fn volume_popup_while_the_ringer_sounds() {
    let rig = || {
        let mut r = Rig::new(Model::D60);
        r.ui.set_fixed_time(Some((1_000_000, 1_700_000_000)));
        r.store.update::<Audio, _>(|a| {
            a.ringer_volume = 3;
            a.speaker_volume = 9;
        });
        r.ui.pump();
        r
    };
    let shot = |r: &Rig| r.ui.canvas().pixels().to_vec();
    let mut r = rig();
    let idle = shot(&r);
    r.tap(Key::VolumeUp);
    assert_eq!(r.audio_cmds(), vec![Command::AdjustVolume { target: VolumeTarget::Current, delta: 1 }]);
    let popup = shot(&r);
    assert_ne!(popup, idle);
    // The audio subsystem sounds the ringer, on the speaker, for the user
    // to hear the volume: the popup stays the ringer's.
    r.store.update::<Audio, _>(|a| {
        a.path = AudioPath::Speaker;
        a.ringing = true;
    });
    r.ui.pump();
    assert_eq!(r.store.get::<Audio>().volume(VolumeTarget::Current), 3);
    assert!(shot(&r) == popup, "the ringer's popup while the ringer sounds");
    // The speaker open for anything else is the speaker's volume.
    r.store.update::<Audio, _>(|a| a.ringing = false);
    r.ui.pump();
    assert_eq!(r.store.get::<Audio>().volume(VolumeTarget::Current), 9);
    assert!(shot(&r) != popup, "the speaker's popup");
    r.store.update::<Audio, _>(|a| a.path = AudioPath::None);
    r.ui.pump();
    assert!(shot(&r) == popup);

    // Another key takes the popup down (the time is fixed: it did not
    // run out), and so does the hookswitch.
    r.tap(Key::Cancel);
    assert!(shot(&r) == idle, "Cancel takes the popup down");
    r.tap(Key::VolumeDown);
    assert!(shot(&r) == popup);
    r.hook(true);
    let mut plain = rig();
    plain.hook(true);
    assert_eq!(r.screen(), plain.screen());
    assert!(shot(&r) == shot(&plain), "the hookswitch takes the popup down");
}

// ------------------------------------------------------- the trial cover

const COVER_MS: u64 = opendeskphone_ui::TRIAL_COVER_MAX_SECS * 1000;

/// A phone whose state is set before the UI starts, as on a real start:
/// the main role seeds the trial before the UI comes up.
fn rig_with(model: Model, f: impl FnOnce(&Store)) -> Rig {
    let mut hal = Hal::sim(model);
    let sim = hal.sim.clone().unwrap();
    let store = Store::new(model);
    store.update::<System, _>(|s| s.phase = Phase::Running);
    store.update::<Accounts, _>(|a| a.accounts.push(Account::new(AccountId(0))));
    f(&store);
    let calls = store.handle(Domain::Calls);
    let audio = store.handle(Domain::Audio);
    let apps = store.handle(Domain::Apps);
    let config = store.handle(Domain::Config);
    let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
    ui.pump();
    Rig { store, ui, sim, calls, audio, apps, config }
}

fn on_trial(store: &Store) {
    store.update::<Update, _>(|u| u.trial = TrialBoot::Pending { boots_left: 2 });
}

fn confirm_trial(r: &mut Rig) {
    r.store.update::<Update, _>(|u| u.trial = TrialBoot::None);
    r.ui.pump();
}

fn main_frame(r: &Rig) -> Vec<u8> {
    r.sim.frame(Screen::Main).expect("main frame").pixels.clone()
}

#[test]
fn a_pending_trial_covers_the_idle_screen_until_it_is_confirmed() {
    for model in [Model::D40, Model::D60] {
        let mut r = rig_with(model, on_trial);
        assert_eq!(r.screen(), "update-trial");
        assert_eq!(r.store.get::<opendeskphone_state::Ui>().screen, "update-trial");
        // Nothing on the cover leads into the menus.
        for key in [Key::Select, Key::Menu, Key::Soft(1), Key::Soft(4), Key::Digit(5), Key::Up, Key::Cancel] {
            r.tap(key);
            assert_eq!(r.screen(), "update-trial", "{model:?} {key:?}");
        }
        assert!(r.call_cmds().is_empty());
        // Confirmed (the main role is healthy): the idle screen, by itself.
        confirm_trial(&mut r);
        assert_eq!(r.screen(), "idle");
        r.tap(Key::Select);
        assert_eq!(r.screen(), "menu");
    }
}

#[test]
fn the_idle_screen_has_no_trial_icon() {
    // Past the cover's time the idle screen shows with the trial still
    // pending, and it is the idle screen of a phone with no trial.
    // (A monotonic time far beyond any real one: the UI started on the
    // real clock.)
    const LATER: (u64, u64) = (1 << 50, 1_794_607_087);
    for model in [Model::D40, Model::D60] {
        let mut plain = Rig::new(model);
        plain.ui.set_fixed_time(Some(LATER));
        plain.ui.pump();
        let mut r = rig_with(model, on_trial);
        assert_eq!(r.screen(), "update-trial");
        r.ui.set_fixed_time(Some(LATER));
        r.ui.pump();
        assert_eq!(r.screen(), "idle");
        assert!(main_frame(&r) == main_frame(&plain), "{model:?}: the idle screen shows the trial");
    }
}

#[test]
fn the_trial_cover_ends_by_itself_after_its_time() {
    let mut hal = Hal::sim(Model::D60);
    let store = Store::new(Model::D60);
    let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig::default());
    ui.set_fixed_time(Some((1_000_000, 1_794_607_087)));
    ui.pump();
    assert_eq!(ui.screen_name(), "boot");
    // The time counts from when the UI first sees the trial.
    on_trial(&store);
    ui.pump();
    assert_eq!(ui.screen_name(), "boot");
    ui.set_fixed_time(Some((1_030_000, 1_794_607_087)));
    store.update::<System, _>(|s| s.phase = Phase::Running);
    ui.pump();
    assert_eq!(ui.screen_name(), "update-trial");
    ui.set_fixed_time(Some((1_000_000 + COVER_MS - 20_000, 1_794_607_087)));
    ui.pump();
    assert_eq!(ui.screen_name(), "update-trial");
    // No state change and no key: the time alone ends it.
    ui.set_fixed_time(Some((1_000_000 + COVER_MS, 1_794_607_087)));
    ui.pump();
    assert_eq!(ui.screen_name(), "idle");
    // The trial is still there for the reset menu and the web interface.
    assert_eq!(store.get::<Update>().trial, TrialBoot::Pending { boots_left: 2 });
    // And the cover does not come back.
    store.update::<Update, _>(|u| u.trial = TrialBoot::Pending { boots_left: 1 });
    ui.pump();
    assert_eq!(ui.screen_name(), "idle");
}

#[test]
fn an_incoming_call_shows_over_the_trial_cover() {
    let mut r = rig_with(Model::D60, on_trial);
    let id = r.add_call(CallState::Ringing);
    assert_eq!(r.screen(), "call");
    r.hook(true);
    assert_eq!(r.call_cmds(), vec![Command::Answer { call: id, path: Some(AudioPath::Handset) }]);
    r.store.update::<Calls, _>(|c| c.calls[0].state = CallState::Active);
    r.ui.pump();
    assert_eq!(r.screen(), "call");
    // The trial is confirmed during the call: the call screen stays.
    confirm_trial(&mut r);
    assert_eq!(r.screen(), "call");
    r.store.update::<Calls, _>(|c| c.calls.clear());
    r.ui.pump();
    assert_eq!(r.screen(), "idle");

    // A call that ends while the trial is still pending: back to the cover.
    let mut r = rig_with(Model::D40, on_trial);
    r.add_call(CallState::Ringing);
    assert_eq!(r.screen(), "call");
    r.store.update::<Calls, _>(|c| c.calls.clear());
    r.ui.pump();
    assert_eq!(r.screen(), "update-trial");
}

#[test]
fn lifting_the_handset_on_the_trial_cover_gives_a_dial_tone() {
    let mut r = rig_with(Model::D60, on_trial);
    r.hook(true);
    assert_eq!(r.screen(), "dialer");
    assert_eq!(
        r.audio_cmds(),
        vec![
            Command::SetAudioPath { path: AudioPath::Handset },
            Command::PlayTone { tone: Tone::Dial, duration_ms: 0 }
        ]
    );
    r.tap(Key::Digit(1));
    r.audio_cmds();
    // The trial is confirmed while the user dials: the dialer and what
    // was typed stay, and no tone is cut off.
    confirm_trial(&mut r);
    assert_eq!(r.screen(), "dialer");
    assert_eq!(r.store.get::<opendeskphone_state::Ui>().dial_buffer, "1");
    assert!(r.audio_cmds().is_empty());
    r.hook(false);
    assert_eq!(r.screen(), "idle");

    // Hanging up with the trial still pending: back to the cover.  The
    // speaker key does the same as the handset.
    let mut r = rig_with(Model::D60, on_trial);
    r.tap(Key::Speaker);
    assert_eq!(r.screen(), "dialer");
    r.tap(Key::Speaker);
    assert_eq!(r.screen(), "update-trial");
}

#[test]
fn the_trial_cover_comes_before_the_server_menu() {
    // A factory-fresh phone on its first boot of a new image: the server
    // menu waits for the trial, and comes up when it is confirmed.
    let mut r = rig_with(Model::D60, |store| {
        on_trial(store);
        store.update::<System, _>(|s| s.phase = Phase::Provisioning);
        store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::AwaitingServer);
    });
    assert_eq!(r.screen(), "update-trial");
    // No account yet: there is nothing to dial with.
    r.hook(true);
    assert_eq!(r.screen(), "update-trial");
    assert!(r.audio_cmds().is_empty());
    r.hook(false);
    confirm_trial(&mut r);
    assert_eq!(r.screen(), "provisioning");
}

#[test]
fn the_trial_cover_leaves_start_up_and_installing_alone() {
    // Waiting for the network: the start-up screen, with its way into
    // the network settings.
    let mut r = rig_with(Model::D60, |store| {
        on_trial(store);
        store.update::<System, _>(|s| s.phase = Phase::NetworkWait);
    });
    assert_eq!(r.screen(), "boot");
    r.tap(Key::Soft(1));
    assert_eq!(r.screen(), "network");
    r.tap(Key::Cancel);
    r.store.update::<System, _>(|s| s.phase = Phase::Running);
    r.ui.pump();
    assert_eq!(r.screen(), "update-trial");
    // Another image arrives during the trial: the installing screen, on
    // which the handset does nothing.
    r.store.update::<Update, _>(|u| u.phase = UpdatePhase::WritingKernel { percent: 10 });
    r.ui.pump();
    assert_eq!(r.screen(), "update");
    r.hook(true);
    assert_eq!(r.screen(), "update");
    assert!(r.audio_cmds().is_empty());
}

fn ring_alarm(r: &mut Rig, id: Option<u32>) {
    r.store.update::<Alarms, _>(|a| a.ringing = id.map(|id| AlarmRinging { id, hour: 8, minute: 0 }));
    r.ui.pump();
}

/// The alarm clock is the main role's; the UI shows the alarm that rings
/// over whatever is on screen, an app included, and its softkeys ask the
/// alarm clock to snooze or dismiss it.
#[test]
fn a_ringing_alarm_shows_over_an_app_with_snooze_and_dismiss() {
    let mut r = Rig::new(Model::D60);
    let alarms = r.store.handle(Domain::Alarms);
    let (view, events) = AppView::new("demo", Model::D60);
    r.ui.handle().open_app(view.clone());
    r.ui.pump();
    assert_eq!(r.screen(), "app:demo");
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));

    ring_alarm(&mut r, Some(3));
    assert_eq!(r.screen(), "alarm");
    assert_eq!(events.try_recv(), Some(AppEvent::Background));
    r.tap(Key::Soft(1));
    assert_eq!(Rig::drain(&alarms), [Command::SnoozeAlarm]);
    // The screen stays until the alarm clock says the alarm stopped; the
    // app is then on screen again.
    assert_eq!(r.screen(), "alarm");
    ring_alarm(&mut r, None);
    assert_eq!(r.screen(), "app:demo");
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));

    // It rings again: Dismiss, and Cancel snoozes.
    ring_alarm(&mut r, Some(3));
    assert_eq!(r.screen(), "alarm");
    r.tap(Key::Soft(4));
    assert_eq!(Rig::drain(&alarms), [Command::DismissAlarm]);
    r.tap(Key::Cancel);
    assert_eq!(Rig::drain(&alarms), [Command::SnoozeAlarm]);
    ring_alarm(&mut r, None);
    assert_eq!(r.screen(), "app:demo");
}

#[test]
fn a_ringing_alarm_shows_on_every_model_and_lights_the_screen() {
    for model in [Model::D40, Model::D50, Model::D60, Model::D70] {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 1, brightness: 10, ..Default::default() });
        ui.pump();
        let lit = sim.brightness(Screen::Main);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        ui.pump();
        assert_ne!(sim.brightness(Screen::Main), lit, "{model:?}: dimmed");
        store.update::<Alarms, _>(|a| a.ringing = Some(AlarmRinging { id: 1, hour: 6, minute: 30 }));
        ui.pump();
        assert_eq!(ui.screen_name(), "alarm", "{model:?}");
        assert_eq!(sim.brightness(Screen::Main), lit, "{model:?}: lit again");
        store.update::<Alarms, _>(|a| a.ringing = None);
        ui.pump();
        assert_eq!(ui.screen_name(), "idle", "{model:?}");
    }
}

/// A user who leaves the ringing alarm's screen is not brought back to it
/// while that alarm rings.
#[test]
fn leaving_the_ringing_alarm_is_possible() {
    let mut r = Rig::new(Model::D60);
    ring_alarm(&mut r, Some(1));
    assert_eq!(r.screen(), "alarm");
    r.tap(Key::Line(1));
    r.store.update::<Features, _>(|f| f.dnd = !f.dnd);
    r.ui.pump();
    assert_ne!(r.screen(), "alarm");
    // The next alarm shows again.
    ring_alarm(&mut r, None);
    ring_alarm(&mut r, Some(2));
    assert_eq!(r.screen(), "alarm");
}

/// A screen asked for as the phone comes up is not lost to the idle
/// screen: the command may be taken before the UI has heard of the state
/// it was sent in.
#[test]
fn a_screen_shown_as_the_state_changes_stays() {
    for model in [Model::D60, Model::D80] {
        let mut hal = Hal::sim(model);
        let store = Store::new(model);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.pump();
        assert_ne!(ui.screen_name(), "idle");
        // The phone is running, and the menu is asked for, before the UI
        // has its next turn.
        store.update::<System, _>(|s| s.phase = Phase::Running);
        let shown = store.send(Command::ShowScreen { name: "menu".into() });
        ui.pump();
        assert_eq!(shown.wait(), Ok(Reply::Done));
        assert_eq!(ui.screen_name(), "menu", "{model:?}");
        // The same for an app's view.
        store.update::<System, _>(|s| s.phase = Phase::NetworkWait);
        ui.pump();
        store.update::<System, _>(|s| s.phase = Phase::Running);
        let (view, _events) = opendeskphone_ui::appview::AppView::new("demo", model);
        ui.handle().open_app(view);
        ui.pump();
        assert_eq!(ui.screen_name(), "app:demo", "{model:?}");
    }
}
