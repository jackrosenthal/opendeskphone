//! The D80's start-up, recovery, provisioning, update, Bluetooth, and
//! alarm screens, operated by touch through the HAL simulator's
//! touchscreen: everything their softkeys and keys do on the other models
//! is done by touching them, and what is touched is large enough to touch.

use opendeskphone_hal::Hal;
use opendeskphone_state::*;
use opendeskphone_ui::gfx::IRect;
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::{Hit, Ui, UiConfig};

const T0: u64 = 1_000_000;
const UNIX: u64 = 1_794_607_087;

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    /// The domains whose commands these screens send.
    handles: Vec<CommandReceiver>,
}

impl Rig {
    /// A D80 in the state `setup` makes, at monotonic time [`T0`].
    fn new(setup: impl FnOnce(&Store)) -> Rig {
        let mut hal = Hal::sim(Model::D80);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(Model::D80);
        setup(&store);
        let handles = [Domain::Provisioning, Domain::System, Domain::Bluetooth, Domain::Alarms].into_iter().map(|d| store.handle(d)).collect();
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.set_fixed_time(Some((T0, UNIX)));
        ui.pump();
        Rig { store, ui, sim, handles }
    }
    /// A D80 at its idle screen.
    fn running() -> Rig {
        Rig::new(|s| s.update::<System, _>(|s| s.phase = Phase::Running))
    }
    /// Moves the clock to `ms` after [`T0`].
    fn at(&mut self, ms: u64) {
        self.ui.set_fixed_time(Some((T0 + ms, UNIX + ms / 1000)));
        self.ui.pump();
    }
    fn update<T: TopicState>(&mut self, f: impl FnOnce(&mut T)) {
        self.store.update::<T, _>(f);
        self.ui.pump();
    }
    fn show(&mut self, screen: &str) {
        let p = self.store.send(Command::ShowScreen { name: screen.into() });
        self.ui.pump();
        assert_eq!(p.wait(), Ok(Reply::Done), "{screen}");
    }
    fn screen(&self) -> String {
        self.ui.screen_name()
    }
    fn hits(&self) -> Vec<(IRect, Hit)> {
        self.ui.canvas().hits().to_vec()
    }
    /// The region registered with `hit` in the picture on the screen.
    fn region(&self, hit: Hit) -> IRect {
        self.ui.canvas().hit_rect(hit).unwrap_or_else(|| panic!("no region {hit:?} on {}", self.screen()))
    }
    fn has(&self, hit: Hit) -> bool {
        self.ui.canvas().hit_rect(hit).is_some()
    }
    /// A finger down and up in the middle of `r`.
    fn tap_in(&mut self, r: IRect) {
        self.sim.touch_tap((r.x + r.w / 2) as u16, (r.y + r.h / 2) as u16);
        self.ui.pump();
    }
    fn tap(&mut self, hit: Hit) {
        self.tap_in(self.region(hit));
    }
    fn tap_at(&mut self, x: i32, y: i32) {
        self.sim.touch_tap(x as u16, y as u16);
        self.ui.pump();
    }
    /// The softkey buttons, in their order.
    fn softkeys(&self) -> Vec<IRect> {
        self.hits().into_iter().filter_map(|(r, h)| matches!(h, Hit::Soft(_)).then_some(r)).collect()
    }
    /// Softkey button `n` (the first is 0).
    fn soft(&mut self, n: usize) {
        let keys = self.softkeys();
        let r = *keys.get(n).unwrap_or_else(|| panic!("{} has {} softkeys", self.screen(), keys.len()));
        self.tap_in(r);
    }
    /// The buttons of the screen's own (not a softkey, a row, or a key).
    fn custom(&self) -> Vec<(IRect, u32)> {
        self.hits().into_iter().filter_map(|(r, h)| if let Hit::Custom(id) = h { Some((r, id)) } else { None }).collect()
    }
    fn commands(&self) -> Vec<Command> {
        let mut v = Vec::new();
        for h in &self.handles {
            while let Some(env) = h.try_recv() {
                v.push(env.command.clone());
                env.done();
            }
        }
        v
    }
    /// Every region on the screen is large enough to touch.
    fn large_enough(&self) {
        let th = Theme::for_model(Model::D80);
        for (r, hit) in self.hits() {
            assert!(r.w >= th.touch_min && r.h >= th.touch_min, "{}: {hit:?} is {r:?}", self.screen());
        }
    }
}

fn row(row: u32) -> Hit {
    Hit::Row { list: 0, row }
}

fn server(n: u8) -> DiscoveredServer {
    DiscoveredServer { source: DiscoverySource::Mdns, name: format!("PBX {n}"), address: format!("sip:proxy@10.0.0.{n}:5060") }
}

fn awaiting_server(found: Vec<DiscoveredServer>) -> Rig {
    Rig::new(|s| {
        s.update::<System, _>(|s| s.phase = Phase::Provisioning);
        s.update::<Provisioning, _>(|p| {
            p.phase = ProvisionPhase::AwaitingServer;
            p.discovered = found;
        });
    })
}

// ----------------------------------------------------------- start-up

#[test]
fn starting_up_nothing_is_touched_and_waiting_for_the_network_has_its_buttons() {
    let mut r = Rig::new(|_| {});
    assert_eq!(r.screen(), "boot");
    assert!(r.hits().is_empty(), "{:?}", r.hits());
    r.tap_at(400, 640);
    assert_eq!(r.screen(), "boot");
    // Waiting for the network: Network and Status, as on the softkeys.
    r.update::<System>(|s| s.phase = Phase::NetworkWait);
    assert_eq!(r.softkeys().len(), 2);
    r.large_enough();
    r.soft(0);
    assert_eq!(r.screen(), "network");
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "boot");
    r.soft(1);
    assert_eq!(r.screen(), "about");
}

#[test]
fn recovery_is_operated_by_its_buttons_and_has_no_way_back() {
    let mut r = Rig::new(|s| s.update::<System, _>(|s| s.phase = Phase::Recovery));
    assert_eq!(r.screen(), "recovery");
    // A base screen: its title has no back button.
    assert!(!r.has(Hit::Key(Key::Cancel)));
    r.large_enough();
    r.soft(0);
    assert_eq!(r.screen(), "about");
    r.tap(Hit::Key(Key::Cancel));
    r.soft(1);
    assert_eq!(r.screen(), "network_status");
    r.tap(Hit::Key(Key::Cancel));
    r.soft(2);
    assert_eq!(r.screen(), "confirm");
    r.soft(0);
    assert_eq!(r.commands(), vec![Command::Reboot { reason: "recovery screen".into() }]);
}

#[test]
fn an_update_takes_no_touch() {
    let mut r = Rig::running();
    r.update::<Update>(|u| {
        u.phase = UpdatePhase::WritingRootfs { percent: 42 };
        u.incoming_version = "3_0_1".into();
    });
    assert_eq!(r.screen(), "update");
    assert!(r.hits().is_empty(), "{:?}", r.hits());
    for (x, y) in [(400, 640), (100, 1200), (50, 50)] {
        r.tap_at(x, y);
    }
    assert_eq!(r.screen(), "update");
    assert!(r.commands().is_empty());
}

// ------------------------------------------------------------ provisioning

#[test]
fn a_server_is_chosen_by_touching_it() {
    let mut r = awaiting_server(vec![server(1), server(2)]);
    assert_eq!(r.screen(), "provisioning");
    // Two servers, "Enter address", and "Use without a server", and no
    // way back; the softkeys are Network and Status (no Select).
    for n in 0..4 {
        assert!(r.has(row(n)), "row {n}");
    }
    assert!(!r.has(Hit::Key(Key::Cancel)));
    assert_eq!(r.softkeys().len(), 2);
    r.large_enough();
    r.tap(row(1));
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: Some(server(2).address) }]);
    // The address is typed.
    r.tap(row(2));
    assert_eq!(r.screen(), "provision-server");
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "provisioning");
    r.tap(row(3));
    assert_eq!(r.commands(), vec![Command::SkipProvisioning]);
    // Network and Status.
    r.soft(0);
    assert_eq!(r.screen(), "network");
    r.tap(Hit::Key(Key::Cancel));
    r.soft(1);
    assert_eq!(r.screen(), "about");
}

#[test]
fn the_count_to_a_lone_server() {
    // Left alone, the count chooses the server after six seconds.
    let mut r = awaiting_server(vec![server(5)]);
    r.at(5_900);
    assert!(r.commands().is_empty());
    r.at(6_000);
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: Some(server(5).address) }]);
    // A touch on the server chooses it at once.
    let mut r = awaiting_server(vec![server(5)]);
    r.at(2_000);
    r.tap(row(0));
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: Some(server(5).address) }]);
    r.at(7_000);
    assert!(r.commands().is_empty());
    // A touch away from the rows stops the count, as a key does.
    let mut r = awaiting_server(vec![server(5)]);
    r.at(2_000);
    r.tap_at(400, 900);
    assert_eq!(r.screen(), "provisioning");
    r.at(10_000);
    assert!(r.commands().is_empty());
    // And so does a finger that scrolls the list.
    let mut r = awaiting_server(vec![server(5)]);
    r.at(2_000);
    let first = r.region(row(0));
    r.sim.touch_drag((400, (first.y + 10) as u16), (400, (first.y + 200) as u16), 6);
    r.ui.pump();
    r.at(10_000);
    assert!(r.commands().is_empty());
}

#[test]
fn a_user_is_chosen_by_touching_it_and_a_pin_is_typed() {
    let mut r = Rig::new(|s| {
        s.update::<System, _>(|s| s.phase = Phase::Provisioning);
        s.update::<Provisioning, _>(|p| {
            p.phase = ProvisionPhase::AwaitingUser;
            p.users = vec![
                ProvisionUser { id: "100".into(), name: "Front Desk".into(), needs_pin: false },
                ProvisionUser { id: "101".into(), name: "Jordan".into(), needs_pin: true },
            ];
        });
    });
    assert_eq!(r.screen(), "provisioning");
    assert!(!r.has(Hit::Key(Key::Cancel)));
    r.large_enough();
    r.tap(row(0));
    assert_eq!(r.commands(), vec![Command::SelectUser { id: "100".into(), pin: String::new() }]);
    r.tap(row(1));
    assert_eq!(r.screen(), "provision-pin");
}

#[test]
fn a_failed_setup_and_the_search_have_their_buttons() {
    let mut r = Rig::new(|s| {
        s.update::<System, _>(|s| s.phase = Phase::Provisioning);
        s.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Failed { reason: "The server did not answer".into(), code: Some(-398) });
    });
    // Retry, Manual, Skip, Network, Status.
    assert_eq!(r.softkeys().len(), 5);
    r.large_enough();
    r.soft(0);
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: None }]);
    r.soft(2);
    assert_eq!(r.commands(), vec![Command::SkipProvisioning]);
    r.soft(1);
    assert_eq!(r.screen(), "provision-server");
    r.tap(Hit::Key(Key::Cancel));
    r.soft(3);
    assert_eq!(r.screen(), "network");
    r.tap(Hit::Key(Key::Cancel));
    r.soft(4);
    assert_eq!(r.screen(), "about");
    // Looking for a server: Manual, Network, Skip, Status.
    r.tap(Hit::Key(Key::Cancel));
    r.update::<Provisioning>(|p| p.phase = ProvisionPhase::Discovering);
    assert_eq!(r.softkeys().len(), 4);
    r.soft(2);
    assert_eq!(r.commands(), vec![Command::SkipProvisioning]);
    r.soft(0);
    assert_eq!(r.screen(), "provision-server");
}

// --------------------------------------------------------------- Bluetooth

fn dev(addr: &str, name: &str) -> BluetoothDevice {
    BluetoothDevice { addr: addr.into(), name: name.into(), rssi: -60 }
}

const JABRA: &str = "00:1B:DC:0F:A1:02";
const VOYAGER: &str = "5C:F3:70:11:22:33";
const OFFICE: &str = "00:0D:44:AA:BB:CC";

/// The pairing question's answers (the actions of its softkeys on the
/// other models) and its PIN field.
const ACCEPT: Hit = Hit::Soft(7);
const REFUSE: Hit = Hit::Soft(8);
const PIN_FIELD: Hit = Hit::Custom(1);

/// The Bluetooth application open: two paired headsets (the first
/// connected) and one found.
fn bluetooth() -> Rig {
    let mut r = Rig::running();
    r.update::<Bluetooth>(|b| {
        b.present = true;
        b.powered = true;
        b.paired = vec![dev(VOYAGER, "Plantronics Voyager"), dev(OFFICE, "Office Headset")];
        b.connected = VOYAGER.into();
        b.found = vec![dev(JABRA, "Jabra Evolve 65")];
    });
    r.show("bluetooth");
    assert_eq!(r.screen(), "bluetooth");
    // Opening it starts a search.
    assert_eq!(r.commands(), vec![Command::BluetoothDiscovery { on: true }]);
    r.update::<Bluetooth>(|b| b.discovering = false);
    r
}

/// The buttons of device row `i`, from the left: their regions.
fn buttons_of(r: &Rig, i: u32) -> Vec<IRect> {
    let mut v: Vec<IRect> = r.custom().into_iter().filter(|(_, id)| id & 0xFFFF == i).map(|(rect, _)| rect).collect();
    v.sort_by_key(|rect| rect.x);
    v
}

#[test]
fn bluetooth_devices_are_operated_by_their_rows_and_buttons() {
    let mut r = bluetooth();
    r.large_enough();
    // Search is the one softkey; the title bar has the way back.
    assert_eq!(r.softkeys().len(), 1);
    assert!(r.has(Hit::Key(Key::Cancel)));
    // The connected headset: Disconnect and Forget.
    assert_eq!(buttons_of(&r, 0).len(), 2);
    r.tap_in(buttons_of(&r, 0)[0]);
    assert_eq!(r.commands(), vec![Command::BluetoothDisconnect]);
    // The other: Connect and Forget.
    r.tap_in(buttons_of(&r, 1)[0]);
    assert_eq!(r.commands(), vec![Command::BluetoothConnect { addr: OFFICE.into() }]);
    r.tap_in(buttons_of(&r, 1)[1]);
    assert_eq!(r.screen(), "confirm");
    r.soft(0);
    assert_eq!(r.commands(), vec![Command::BluetoothUnpair { addr: OFFICE.into() }]);
    assert_eq!(r.screen(), "bluetooth");
    // The one found: Pair, which stops the search first.
    assert_eq!(buttons_of(&r, 2).len(), 1);
    r.tap_in(buttons_of(&r, 2)[0]);
    assert_eq!(r.commands(), vec![Command::BluetoothDiscovery { on: false }, Command::BluetoothPair { addr: JABRA.into() }]);
    // A touch on a row elsewhere does what Select does to it.
    r.tap_in(IRect { w: 60, ..r.region(row(1)) });
    assert_eq!(r.commands(), vec![Command::BluetoothConnect { addr: OFFICE.into() }]);
    r.tap_in(IRect { w: 60, ..r.region(row(2)) });
    assert_eq!(r.commands(), vec![Command::BluetoothDiscovery { on: false }, Command::BluetoothPair { addr: JABRA.into() }]);
    // While a pairing is under way, no other: the found one has no button.
    r.update::<Bluetooth>(|b| b.pairing_with = JABRA.into());
    assert!(buttons_of(&r, 2).is_empty());
    r.tap(row(2));
    assert!(r.commands().is_empty());
    // Search.
    r.soft(0);
    assert_eq!(r.commands(), vec![Command::BluetoothDiscovery { on: true }]);
    r.update::<Bluetooth>(|b| b.discovering = true);
    assert!(r.softkeys().is_empty());
    // Back.
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "idle");
}

#[test]
fn a_finger_scrolls_the_bluetooth_list() {
    let mut r = bluetooth();
    r.update::<Bluetooth>(|b| {
        for i in 0..12 {
            b.found.push(dev(&format!("00:00:00:00:00:{i:02}"), &format!("Headset {}", i + 1)));
        }
    });
    let first = r.region(row(0));
    assert!(!r.has(row(14)));
    // Up by a few rows: the rows follow the finger.
    r.sim.touch_drag((400, 1000), (400, 600), 8);
    r.ui.pump();
    assert!(!r.has(row(0)));
    assert!(r.has(row(3)) && !r.has(row(14)));
    // Up by more than there is: the last row is the last one shown.
    for _ in 0..3 {
        r.sim.touch_drag((400, 1000), (400, 200), 8);
        r.ui.pump();
    }
    let last = r.region(row(14));
    assert!(last.bottom() <= r.softkeys()[0].y, "{last:?}");
    assert!(r.commands().is_empty(), "a drag does nothing to a device");
    // Back to the top, and no farther.
    for _ in 0..3 {
        r.sim.touch_drag((400, 200), (400, 1100), 8);
        r.ui.pump();
    }
    assert_eq!(r.region(row(0)), first);
}

#[test]
fn a_pairing_question_is_answered_by_its_buttons() {
    let mut r = bluetooth();
    r.update::<Bluetooth>(|b| {
        b.pairing = Some(BluetoothPairing { addr: JABRA.into(), name: "Jabra Evolve 65".into(), passkey: Some(42), pin_needed: false });
    });
    r.large_enough();
    // Nothing else can be touched: not the rows, nor the way back.
    let back = r.region(Hit::Key(Key::Cancel));
    assert_eq!(r.ui.canvas().hit_at(back.x + back.w / 2, back.y + back.h / 2), Some(Hit::Nothing));
    r.tap_in(IRect { w: 60, ..r.region(row(0)) });
    assert!(r.commands().is_empty());
    assert_eq!(r.screen(), "bluetooth");
    // Pair, and Cancel.
    r.tap(ACCEPT);
    assert_eq!(r.commands(), vec![Command::BluetoothAnswerPairing { addr: JABRA.into(), accept: true, pin: None }]);
    r.tap(REFUSE);
    assert_eq!(r.commands(), vec![Command::BluetoothAnswerPairing { addr: JABRA.into(), accept: false, pin: None }]);
}

#[test]
fn a_pin_is_typed_in_its_own_screen() {
    let mut r = bluetooth();
    r.update::<Bluetooth>(|b| {
        b.pairing = Some(BluetoothPairing { addr: JABRA.into(), name: "Jabra Evolve 65".into(), passkey: None, pin_needed: true });
    });
    r.large_enough();
    // OK with no PIN typed: the PIN's screen.
    r.tap(ACCEPT);
    assert_eq!(r.screen(), "bluetooth-pin");
    assert!(r.commands().is_empty());
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "bluetooth");
    // The field too; the PIN typed there answers the question.
    r.tap(PIN_FIELD);
    assert_eq!(r.screen(), "bluetooth-pin");
    for _ in 0..4 {
        r.sim.tap(Key::Digit(0));
    }
    r.sim.tap(Key::Select);
    r.ui.pump();
    assert_eq!(r.commands(), vec![Command::BluetoothAnswerPairing { addr: JABRA.into(), accept: true, pin: Some("0000".into()) }]);
    assert_eq!(r.screen(), "bluetooth");
}

// ------------------------------------------------------------------- alarm

#[test]
fn a_ringing_alarm_has_two_large_buttons() {
    let mut r = Rig::running();
    r.update::<Alarms>(|a| a.ringing = Some(AlarmRinging { id: 1, hour: 8, minute: 0 }));
    assert_eq!(r.screen(), "alarm");
    let th = Theme::for_model(Model::D80);
    // No softkeys at the bottom and no way back: Snooze and Dismiss, the
    // width of the screen, Snooze the larger.
    assert!(!r.has(Hit::Key(Key::Cancel)));
    // (The actions of the Snooze and Dismiss softkeys on the other models.)
    let (snooze, dismiss) = (r.region(Hit::Soft(1)), r.region(Hit::Soft(4)));
    assert_eq!(r.hits().len(), 2, "{:?}", r.hits());
    for b in [snooze, dismiss] {
        assert!(b.w >= th.width * 3 / 4 && b.h >= th.touch_min, "{b:?}");
    }
    assert!(snooze.h > dismiss.h && snooze.bottom() <= dismiss.y);
    r.tap_in(snooze);
    assert_eq!(r.commands(), vec![Command::SnoozeAlarm]);
    r.tap_in(dismiss);
    assert_eq!(r.commands(), vec![Command::DismissAlarm]);
}
