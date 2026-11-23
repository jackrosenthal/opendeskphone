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

/// The setup's actions (the softkeys' on the other models, and the touch
/// skin's own).
const SK_RETRY: Hit = Hit::Soft(2);
const SK_MANUAL: Hit = Hit::Soft(3);
const SK_SKIP: Hit = Hit::Soft(5);
const SK_MORE: Hit = Hit::Soft(7);
const SK_CANCEL: Hit = Hit::Soft(8);

/// The buttons of the screen that are as wide as it, or half as wide.
fn wide(r: &Rig, hit: Hit) -> bool {
    let th = Theme::for_model(Model::D80);
    r.region(hit).w >= th.width * 2 / 5
}

/// More, from the setup screen on: its rows, and each one's screen.
fn more_leads_to(r: &mut Rig, rows: &[&str]) {
    r.tap(SK_MORE);
    assert_eq!(r.screen(), "provision-more");
    assert!(!r.has(row(rows.len() as u32)));
    for (i, name) in rows.iter().enumerate() {
        r.tap(row(i as u32));
        assert_eq!(r.screen(), *name);
        if *name != "provisioning" {
            r.tap(Hit::Key(Key::Cancel));
            assert_eq!(r.screen(), "provision-more");
        }
    }
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "provisioning");
}

#[test]
fn a_server_is_chosen_by_touching_it() {
    let mut r = awaiting_server(vec![server(1), server(2)]);
    assert_eq!(r.screen(), "provisioning");
    // Two servers and "Enter address", and no way back; "Use without a
    // server" is a button of its own across the bottom, beside More.
    for n in 0..3 {
        assert!(r.has(row(n)), "row {n}");
    }
    assert!(!r.has(row(3)));
    assert!(!r.has(Hit::Key(Key::Cancel)));
    assert_eq!(r.softkeys().len(), 2);
    assert!(wide(&r, SK_SKIP) && wide(&r, SK_MORE));
    assert!(r.region(SK_SKIP).y > r.region(row(2)).bottom() + Theme::for_model(Model::D80).row_h);
    r.large_enough();
    r.tap(row(1));
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: Some(server(2).address) }]);
    // The address is typed.
    r.tap(row(2));
    assert_eq!(r.screen(), "provision-server");
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "provisioning");
    r.tap(SK_SKIP);
    assert_eq!(r.commands(), vec![Command::SkipProvisioning]);
    // More: the network's settings, and the phone's details.
    more_leads_to(&mut r, &["network", "about"]);
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
    r.tap(Hit::Key(Key::Cancel));
    more_leads_to(&mut r, &["network", "about"]);
}

#[test]
fn the_users_are_narrowed_by_what_is_typed() {
    use opendeskphone_ui::osk::Cap;
    let mut r = Rig::new(|s| {
        s.update::<System, _>(|s| s.phase = Phase::Provisioning);
        s.update::<Provisioning, _>(|p| {
            p.phase = ProvisionPhase::AwaitingUser;
            p.users = (0..40).map(|i| ProvisionUser { id: format!("1{i:03}"), name: format!("User {i}"), needs_pin: false }).collect();
            p.users.push(ProvisionUser { id: "2000".into(), name: "Jordan Example".into(), needs_pin: false });
        });
    });
    const FILTER: Hit = Hit::Custom(2);
    // No keyboard until the field is touched; More is at the bottom.
    assert!(!r.has(Cap::Done.hit()));
    assert!(r.has(SK_MORE));
    r.large_enough();
    r.tap(FILTER);
    assert!(r.has(Cap::Done.hit()) && !r.has(SK_MORE));
    // By name, in any case: the one row, chosen by a touch.
    for ch in "jor".chars() {
        r.tap(Cap::Char(ch).hit());
    }
    assert!(r.has(row(0)) && !r.has(row(1)));
    r.tap(row(0));
    assert_eq!(r.commands(), vec![Command::SelectUser { id: "2000".into(), pin: String::new() }]);
    // By extension: 1020 to 1029, with the keyboard put away.
    for _ in 0..3 {
        r.tap(Cap::Backspace.hit());
    }
    r.tap(Cap::Symbols.hit());
    for ch in "102".chars() {
        r.tap(Cap::Char(ch).hit());
    }
    r.tap(Cap::Done.hit());
    assert!(!r.has(Cap::Done.hit()));
    r.tap(row(3));
    assert_eq!(r.commands(), vec![Command::SelectUser { id: "1023".into(), pin: String::new() }]);
    // Nothing matches: no rows, and the field is still there.
    r.tap(FILTER);
    r.tap(Cap::Char('z').hit());
    assert!(!r.has(row(0)) && r.has(FILTER));
}

#[test]
fn a_failed_setup_says_what_to_do_and_has_its_buttons() {
    let mut r = Rig::new(|s| {
        s.update::<System, _>(|s| s.phase = Phase::Provisioning);
        s.update::<Provisioning, _>(|p| {
            p.phase = ProvisionPhase::Failed {
                reason: "The server at \u{201c}tu\u{201d} did not answer. Check the address and the network cable.".into(),
                code: None,
                detail: "transport: the MESSAGE was refused: 408".into(),
            }
        });
    });
    // Try again, Enter a different address, and More, one under the
    // other across the screen.
    assert_eq!(r.softkeys().len(), 3);
    r.large_enough();
    for hit in [SK_RETRY, SK_MANUAL, SK_MORE] {
        assert!(r.region(hit).w >= Theme::for_model(Model::D80).width * 9 / 10, "{hit:?}");
    }
    assert!(r.region(SK_RETRY).bottom() <= r.region(SK_MANUAL).y && r.region(SK_MANUAL).bottom() <= r.region(SK_MORE).y);
    r.tap(SK_RETRY);
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: None }]);
    r.tap(SK_MANUAL);
    assert_eq!(r.screen(), "provision-server");
    r.tap(Hit::Key(Key::Cancel));
    // More: going on without a server too.
    r.tap(SK_MORE);
    r.tap(row(1));
    assert_eq!(r.screen(), "network");
    r.tap(Hit::Key(Key::Cancel));
    r.tap(row(2));
    assert_eq!(r.screen(), "about");
    r.tap(Hit::Key(Key::Cancel));
    r.tap(row(0));
    assert_eq!(r.commands(), vec![Command::SkipProvisioning]);
    assert_eq!(r.screen(), "provisioning");

    // Looking for a server: an address, going on without a server, and
    // More.
    r.update::<Provisioning>(|p| p.phase = ProvisionPhase::Discovering);
    assert_eq!(r.softkeys().len(), 3);
    r.large_enough();
    r.tap(SK_SKIP);
    assert_eq!(r.commands(), vec![Command::SkipProvisioning]);
    r.tap(SK_MANUAL);
    assert_eq!(r.screen(), "provision-server");
    r.tap(Hit::Key(Key::Cancel));
    more_leads_to(&mut r, &["network", "about"]);
}

#[test]
fn contacting_a_server_is_canceled_and_its_list_does_not_count_again() {
    // A lone server, chosen by the count.
    let mut r = awaiting_server(vec![server(5)]);
    r.at(6_000);
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: Some(server(5).address) }]);
    r.update::<Provisioning>(|p| {
        p.phase = ProvisionPhase::Handshaking;
        p.server = Some(server(5));
    });
    // Contacting it: one button, Cancel.
    assert_eq!(r.softkeys().len(), 1);
    assert!(wide(&r, SK_CANCEL));
    r.large_enough();
    r.tap(SK_CANCEL);
    assert_eq!(r.commands(), vec![Command::CancelProvisioning]);
    // The list again, with the same lone server: it is not chosen again
    // by itself.
    r.update::<Provisioning>(|p| p.phase = ProvisionPhase::AwaitingServer);
    r.at(7_000);
    r.at(20_000);
    assert!(r.commands().is_empty());
    // A touch on it still chooses it.
    r.tap(row(0));
    assert_eq!(r.commands(), vec![Command::StartProvisioning { server: Some(server(5).address) }]);
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

/// The Bluetooth search's button (the action of the Search softkey).
const SEARCH: Hit = Hit::Soft(5);

#[test]
fn bluetooth_devices_are_operated_by_their_rows_and_buttons() {
    let th = Theme::for_model(Model::D80);
    let mut r = bluetooth();
    r.large_enough();
    // The search is the one button, across the bottom; the title bar has
    // the way back.
    assert_eq!(r.softkeys().len(), 1);
    assert!(r.region(SEARCH).w >= th.width * 9 / 10);
    assert!(r.has(Hit::Key(Key::Cancel)));
    // The connected headset: Disconnect, and the way to its own screen.
    assert_eq!(buttons_of(&r, 0).len(), 2);
    r.tap_in(buttons_of(&r, 0)[0]);
    assert_eq!(r.commands(), vec![Command::BluetoothDisconnect]);
    // The other: Connect, and its screen.
    r.tap_in(buttons_of(&r, 1)[0]);
    assert_eq!(r.commands(), vec![Command::BluetoothConnect { addr: OFFICE.into() }]);
    r.tap_in(buttons_of(&r, 1)[1]);
    assert_eq!(r.screen(), "bluetooth-device");
    r.tap(Hit::Key(Key::Cancel));
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
    // Search: while it searches, the button stays, and does nothing.
    r.tap(SEARCH);
    assert_eq!(r.commands(), vec![Command::BluetoothDiscovery { on: true }]);
    r.update::<Bluetooth>(|b| b.discovering = true);
    assert!(r.softkeys().is_empty());
    r.tap_at(400, 1200);
    assert!(r.commands().is_empty());
    // Back.
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "idle");
}

#[test]
fn a_headset_is_forgotten_from_its_own_screen() {
    let th = Theme::for_model(Model::D80);
    let mut r = bluetooth();
    r.tap_in(buttons_of(&r, 0)[1]);
    assert_eq!(r.screen(), "bluetooth-device");
    r.large_enough();
    // Connected: Disconnect; and Forget at the bottom, apart, in red.
    const DISCONNECT: Hit = Hit::Soft(3);
    const CONNECT: Hit = Hit::Soft(2);
    const FORGET: Hit = Hit::Soft(6);
    assert!(!r.has(CONNECT));
    let (disconnect, forget) = (r.region(DISCONNECT), r.region(FORGET));
    assert!(forget.w >= th.width * 9 / 10 && forget.y >= disconnect.bottom() + th.row_h / 2);
    let red = r.ui.canvas().pixel(forget.x + th.pad, forget.y + forget.h / 2);
    assert_eq!(red, th.in_wallpaper(opendeskphone_ui::wallpaper::default()).sk_stop, "Forget is not red");
    r.tap(DISCONNECT);
    assert_eq!(r.commands(), vec![Command::BluetoothDisconnect]);
    r.update::<Bluetooth>(|b| b.connected.clear());
    r.tap(CONNECT);
    assert_eq!(r.commands(), vec![Command::BluetoothConnect { addr: VOYAGER.into() }]);
    // Forget asks first; No leaves it.
    r.tap(FORGET);
    assert_eq!(r.screen(), "confirm");
    r.tap(Hit::Soft(4));
    assert_eq!(r.screen(), "bluetooth-device");
    r.tap(FORGET);
    r.tap(Hit::Soft(1));
    assert_eq!(r.commands(), vec![Command::BluetoothUnpair { addr: VOYAGER.into() }]);
    assert_eq!(r.screen(), "bluetooth-device");
    // Once it is no longer paired, back to the list.
    r.update::<Bluetooth>(|b| b.paired.retain(|d| d.addr != VOYAGER));
    r.ui.pump();
    assert_eq!(r.screen(), "bluetooth");
}

#[test]
fn bluetooth_without_a_controller_says_what_to_do() {
    let mut r = Rig::running();
    r.show("bluetooth");
    assert_eq!(r.screen(), "bluetooth");
    // Nothing to touch but the ways back and home.
    assert_eq!(r.hits().len(), 2, "{:?}", r.hits());
    assert!(r.has(Hit::Key(Key::Cancel)));
    assert!(r.has(Hit::Home));
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
    let fifth = r.region(row(5));
    assert!(!r.has(row(14)));
    // The finger goes up by a few rows, and the rows follow it by the
    // pixel.
    r.sim.touch_drag((400, 1000), (400, 600), 8);
    r.ui.pump();
    assert_eq!(r.region(row(5)), IRect { y: fifth.y - 400, ..fifth });
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
