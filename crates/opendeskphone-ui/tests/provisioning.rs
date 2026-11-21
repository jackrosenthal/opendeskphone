//! The server menu of a phone with no saved server (first start, factory
//! reset): shown as soon as the phone waits for a server, with stock's
//! countdown to a lone entry (`choosePbx.js`, `startSingleCountdown`).

use opendeskphone_hal::{Hal, Screen};
use opendeskphone_state::*;
use opendeskphone_ui::{Ui, UiConfig};

const T0: u64 = 1_000_000;
const UNIX: u64 = 1_794_607_087;

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    provisioning: CommandReceiver,
}

fn server(n: u8) -> DiscoveredServer {
    DiscoveredServer { source: DiscoverySource::Mdns, name: format!("PBX {n}"), address: format!("sip:proxy@10.0.0.{n}:5060") }
}

impl Rig {
    /// A phone that waits for a server, at monotonic time [`T0`].
    fn new(model: Model, found: Vec<DiscoveredServer>) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Provisioning);
        store.update::<Provisioning, _>(|p| {
            p.phase = ProvisionPhase::AwaitingServer;
            p.discovered = found;
        });
        let provisioning = store.handle(Domain::Provisioning);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.set_fixed_time(Some((T0, UNIX)));
        ui.pump();
        Rig { store, ui, sim, provisioning }
    }
    /// Moves the clock to `ms` after [`T0`].
    fn at(&mut self, ms: u64) {
        self.ui.set_fixed_time(Some((T0 + ms, UNIX + ms / 1000)));
        self.ui.pump();
    }
    fn tap(&mut self, key: Key) {
        self.sim.tap(key);
        self.ui.pump();
    }
    fn found(&mut self, found: Vec<DiscoveredServer>) {
        self.store.update::<Provisioning, _>(|p| p.discovered = found);
        self.ui.pump();
    }
    fn commands(&self) -> Vec<Command> {
        let mut v = Vec::new();
        while let Some(env) = self.provisioning.try_recv() {
            v.push(env.command.clone());
            env.done();
        }
        v
    }
    fn frame(&self) -> Vec<u8> {
        self.sim.frame(Screen::Main).expect("main frame").pixels
    }
}

fn pick(n: u8) -> Vec<Command> {
    vec![Command::StartProvisioning { server: Some(server(n).address) }]
}

#[test]
fn a_lone_server_is_selected_after_six_seconds() {
    for model in [Model::D40, Model::D60] {
        let mut r = Rig::new(model, vec![server(5)]);
        // The menu is on the display before anything is selected.
        assert_eq!(r.ui.screen_name(), "provisioning", "{model:?}");
        let mut frames = vec![r.frame()];
        assert!(frames[0].iter().any(|p| *p != frames[0][0]), "{model:?}: blank menu");
        // 5, 4, 3, 2, 1, 0: the number changes every second, and nothing
        // is selected while it counts.
        for s in 1..=5 {
            r.at(s * 1000);
            assert!(r.commands().is_empty(), "{model:?}: selected after {s} s");
            let f = r.frame();
            assert_ne!(&f, frames.last().unwrap(), "{model:?}: the count did not change at {s} s");
            frames.push(f);
        }
        r.at(5999);
        assert!(r.commands().is_empty(), "{model:?}");
        r.at(6000);
        assert_eq!(r.commands(), pick(5), "{model:?}");
        // Once only.
        r.at(7000);
        r.at(13_000);
        assert!(r.commands().is_empty(), "{model:?}");
    }
}

#[test]
fn the_count_starts_when_the_server_is_listed() {
    // Nothing found yet: the menu waits, with its two own rows.
    let mut r = Rig::new(Model::D40, Vec::new());
    assert_eq!(r.ui.screen_name(), "provisioning");
    r.at(20_000);
    assert!(r.commands().is_empty());
    // The server is announced 20 seconds in: six seconds from then.
    r.found(vec![server(5)]);
    r.at(25_999);
    assert!(r.commands().is_empty());
    r.at(26_000);
    assert_eq!(r.commands(), pick(5));
}

#[test]
fn a_key_stops_the_count() {
    for key in [Key::Down, Key::Up, Key::Digit(1), Key::Soft(4)] {
        let mut r = Rig::new(Model::D40, vec![server(5)]);
        r.at(2000);
        let counting = r.frame();
        r.tap(key);
        if key == Key::Soft(4) {
            // Status covers the menu; coming back does not resume.
            assert_ne!(r.ui.screen_name(), "provisioning");
            r.tap(Key::Cancel);
            assert_eq!(r.ui.screen_name(), "provisioning");
        }
        assert_ne!(r.frame(), counting, "{key:?}: the count is no longer shown");
        let stopped = r.frame();
        r.at(6000);
        r.at(60_000);
        assert!(r.commands().is_empty(), "{key:?}");
        assert_eq!(r.frame(), stopped, "{key:?}: nothing counts");
        // The list following discovery does not start it again.
        r.found(vec![server(5), server(6)]);
        r.found(vec![server(5)]);
        r.at(120_000);
        assert!(r.commands().is_empty(), "{key:?}");
    }
}

#[test]
fn select_picks_at_once() {
    let mut r = Rig::new(Model::D40, vec![server(5)]);
    r.at(1000);
    r.tap(Key::Select);
    assert_eq!(r.commands(), pick(5));
    r.at(60_000);
    assert!(r.commands().is_empty(), "the count does not select a second time");
}

#[test]
fn a_second_server_cancels_the_count() {
    let mut r = Rig::new(Model::D40, vec![server(5)]);
    r.at(3000);
    r.found(vec![server(5), server(6)]);
    r.at(60_000);
    assert!(r.commands().is_empty());
    // With several entries the phone waits for a choice.
    r.tap(Key::Down);
    r.tap(Key::Select);
    assert_eq!(r.commands(), pick(6));
}

#[test]
fn several_servers_wait_for_a_choice() {
    let mut r = Rig::new(Model::D40, vec![server(5), server(6)]);
    let shown = r.frame();
    r.at(60_000);
    assert!(r.commands().is_empty());
    assert_eq!(r.frame(), shown);
}

#[test]
fn the_count_starts_over_when_the_menu_is_shown_again() {
    let mut r = Rig::new(Model::D40, vec![server(5)]);
    r.at(6000);
    assert_eq!(r.commands(), pick(5));
    // The server refuses; the phone is later told to forget it and waits
    // for a server again (stock: `choosePbx.show` once more).
    r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Failed { reason: "no answer".into(), code: None, detail: String::new() });
    r.at(10_000);
    r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::AwaitingServer);
    r.at(10_001);
    r.at(16_000);
    assert!(r.commands().is_empty());
    r.at(16_001);
    assert_eq!(r.commands(), pick(5));
}

/// A phone in its setup shows the setup screens from the start: before the
/// DPMA client has started (no address yet) there is no idle screen.
#[test]
fn setup_shows_before_the_client_has_started() {
    let mut r = Rig::new(Model::D40, Vec::new());
    r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Idle);
    r.at(1000);
    assert_eq!(r.ui.screen_name(), "provisioning");
    r.at(120_000);
    assert_eq!(r.ui.screen_name(), "provisioning");
    assert!(r.commands().is_empty());
    // The client starts and waits for a server: the list, then the count.
    r.store.update::<Provisioning, _>(|p| {
        p.phase = ProvisionPhase::AwaitingServer;
        p.discovered = vec![server(5)];
    });
    r.at(120_001);
    r.at(126_000);
    assert!(r.commands().is_empty());
    r.at(126_001);
    assert_eq!(r.commands(), pick(5));
}

/// A phone in its setup with no network (the client has not started) is
/// not trapped: it can be used without a server, given an address, and its
/// network settings can be reached, coming back to setup afterward.
#[test]
fn setup_with_no_network_is_not_a_trap() {
    for model in [Model::D40, Model::D60] {
        let mut r = Rig::new(model, Vec::new());
        r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Idle);
        r.at(1000);
        assert_eq!(r.ui.screen_name(), "provisioning", "{model:?}");

        // Network: the phone's network settings, and back.
        r.tap(Key::Soft(2));
        assert_eq!(r.ui.screen_name(), "network", "{model:?}");
        r.tap(Key::Cancel);
        assert_eq!(r.ui.screen_name(), "provisioning", "{model:?}");

        // Manual: an address.
        r.tap(Key::Soft(1));
        assert_eq!(r.ui.screen_name(), "provision-server", "{model:?}");
        r.tap(Key::Cancel);
        assert_eq!(r.ui.screen_name(), "provisioning", "{model:?}");

        // Skip: use without a server.
        r.tap(Key::Soft(3));
        assert_eq!(r.commands(), vec![Command::SkipProvisioning], "{model:?}");
        r.store.update::<Provisioning, _>(|p| p.phase = ProvisionPhase::Standalone);
        r.at(2000);
        assert_eq!(r.ui.screen_name(), "idle", "{model:?}");
    }
}

/// The network settings are reachable from the server list too; going
/// there stops the count, as stock's Settings softkey does.
#[test]
fn network_settings_from_the_server_list() {
    let mut r = Rig::new(Model::D40, vec![server(5)]);
    r.at(2000);
    r.tap(Key::Soft(2));
    assert_eq!(r.ui.screen_name(), "network");
    r.at(30_000);
    r.tap(Key::Cancel);
    assert_eq!(r.ui.screen_name(), "provisioning");
    r.at(60_000);
    assert!(r.commands().is_empty(), "the count was stopped");
    r.tap(Key::Select);
    assert_eq!(r.commands(), pick(5));
}
