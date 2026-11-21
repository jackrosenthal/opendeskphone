//! The touch skin's (D80's) menus and text entry, through the HAL
//! simulator's touchscreen: the menu from the idle screen, settings
//! edited with the on-screen keyboard, the call log's filters and
//! details, apps launched from the grid, levels, themes, and ringtones.

use opendeskphone_hal::Hal;
use opendeskphone_state::*;
use opendeskphone_ui::gfx::IRect;
use opendeskphone_ui::osk::Cap;
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::{Hit, Ui, UiConfig};
use std::net::Ipv4Addr;

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    calls: CommandReceiver,
    config: CommandReceiver,
    audio: CommandReceiver,
    apps: CommandReceiver,
    provisioning: CommandReceiver,
    system: CommandReceiver,
}

impl Rig {
    fn new() -> Rig {
        let model = Model::D80;
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        store.update::<Accounts, _>(|a| {
            let mut acct = Account::new(AccountId(0));
            acct.label = "100".into();
            a.accounts.push(acct);
        });
        store.update::<Network, _>(|n| {
            n.phase = NetPhase::Up;
            n.lan.up = true;
            n.ipv4 = Ipv4State {
                source: AddrSource::Dhcp,
                address: Some(Ipv4Addr::new(198, 18, 15, 87)),
                prefix_len: 24,
                ..Default::default()
            };
        });
        let handle = |d| store.handle(d);
        let (calls, config, audio, apps, provisioning) =
            (handle(Domain::Calls), handle(Domain::Config), handle(Domain::Audio), handle(Domain::Apps), handle(Domain::Provisioning));
        let system = handle(Domain::System);
        let cfg = UiConfig {
            backlight_timeout_secs: 0,
            ringtone: "Flutey Phone".into(),
            ringtones: vec!["Flutey Phone".into(), "Acheron".into(), "Andromeda".into()],
            ..Default::default()
        };
        let mut ui = Ui::from_hal(&mut hal, store.clone(), cfg);
        ui.set_fixed_time(Some((5_000_000, 1_794_607_087)));
        ui.pump();
        Rig { store, ui, sim, calls, config, audio, apps, provisioning, system }
    }
    fn show(&mut self, screen: &str) {
        let p = self.store.send(Command::ShowScreen { name: screen.into() });
        self.ui.pump();
        assert_eq!(p.wait(), Ok(Reply::Done), "{screen}");
    }
    fn screen(&self) -> String {
        self.ui.screen_name()
    }
    /// The region registered with `hit` in the picture on the screen.
    fn region(&self, hit: Hit) -> IRect {
        self.ui.canvas().hit_rect(hit).unwrap_or_else(|| panic!("no region {hit:?} on {}", self.screen()))
    }
    fn tap_at(&mut self, x: i32, y: i32) {
        self.sim.touch_tap(x as u16, y as u16);
        self.ui.pump();
    }
    /// A finger down and up in the middle of the region with `hit`.
    fn tap(&mut self, hit: Hit) {
        let r = self.region(hit);
        self.tap_at(r.x + r.w / 2, r.y + r.h / 2);
    }
    /// Touches the keys of the on-screen keyboard that type `s`, as they
    /// are shown.
    fn type_keys(&mut self, s: &str) {
        for ch in s.chars() {
            self.tap(Cap::Char(ch).hit());
        }
    }
    /// The rows of list 0 on the screen.
    fn rows(&self) -> Vec<u32> {
        self.ui.canvas().hits().iter().filter_map(|(_, h)| if let Hit::Row { list: 0, row } = h { Some(*row) } else { None }).collect()
    }
    fn regions(&self) -> Vec<(IRect, Hit)> {
        self.ui.canvas().hits().to_vec()
    }
}

/// The commands a domain's handler got, each answered.
fn taken(rx: &CommandReceiver) -> Vec<Command> {
    let mut v = Vec::new();
    while let Some(env) = rx.try_recv() {
        v.push(env.command.clone());
        env.done();
    }
    v
}

fn row(row: u32) -> Hit {
    Hit::Row { list: 0, row }
}

/// The main menu's rows on the D80 (which has Bluetooth).
const MENU_CALL_LOG: u32 = 0;
const MENU_FORWARD: u32 = 2;
const MENU_SETTINGS: u32 = 6;
/// The settings' rows.
const SETTINGS_NETWORK: u32 = 0;
const SETTINGS_DISPLAY: u32 = 2;
const SETTINGS_SOUNDS: u32 = 3;
const SETTINGS_RESET: u32 = 4;

fn th() -> &'static Theme {
    Theme::for_model(Model::D80)
}

/// The theme the screens are drawn in: the skin's, in the palette of the
/// default wallpaper.
fn shown_th() -> &'static Theme {
    th().in_wallpaper(opendeskphone_ui::wallpaper::default())
}

/// Every region at least `touch_min` both ways.
fn all_large(r: &Rig) {
    for (rect, hit) in r.regions() {
        assert!(rect.w >= th().touch_min && rect.h >= th().touch_min, "{}: {hit:?} is {rect:?}", r.screen());
    }
}

#[test]
fn a_setting_is_edited_with_the_number_pad_from_the_menu() {
    let mut r = Rig::new();
    assert_eq!(r.screen(), "idle");
    r.show("menu");
    r.tap(row(MENU_SETTINGS));
    assert_eq!(r.screen(), "settings");
    r.tap(row(SETTINGS_NETWORK));
    assert_eq!(r.screen(), "network");
    // The IP address.
    r.tap(row(1));
    assert_eq!(r.screen(), "setting_edit");
    // No softkeys, no cursor keys: the field, its buttons, and the pad,
    // all large; the pad is at the bottom of the screen.
    all_large(&r);
    assert!(r.ui.canvas().hits().iter().all(|(_, h)| !matches!(h, Hit::Key(k) if *k != Key::Cancel)));
    let done = r.region(Cap::Done.hit());
    assert_eq!(done.bottom(), th().height - th().px(4));
    // A pad of numbers has no letters.
    assert!(r.ui.canvas().hit_rect(Cap::Char('a').hit()).is_none());
    for _ in 0.."198.18.15.87".len() {
        r.tap(Cap::Backspace.hit());
    }
    r.type_keys("10.0.0.5");
    assert!(taken(&r.config).is_empty());
    // OK, beside the field.
    r.tap(Hit::Soft(1));
    assert_eq!(taken(&r.config), vec![Command::SetSettings { settings: vec![("network_static_ip_address".into(), "10.0.0.5".into())] }]);
    assert_eq!(r.screen(), "network");

    // Cancel, and the back button, leave the setting as it was.
    r.tap(row(1));
    r.type_keys("9");
    r.tap(Hit::Soft(4));
    assert_eq!(r.screen(), "network");
    r.tap(row(1));
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "network");
    assert!(taken(&r.config).is_empty());
}

#[test]
fn text_takes_capitals_symbols_and_a_finger_that_places_the_cursor() {
    let mut r = Rig::new();
    r.store.update::<System, _>(|s| s.phase = Phase::Provisioning);
    r.store.update::<Provisioning, _>(|p| {
        p.phase = ProvisionPhase::AwaitingServer;
        p.discovered = vec![
            DiscoveredServer { source: DiscoverySource::Mdns, name: "Office PBX".into(), address: "198.18.15.2:5060".into() },
            DiscoveredServer { source: DiscoverySource::Mdns, name: "Lab PBX".into(), address: "198.18.15.3:5060".into() },
        ];
    });
    r.ui.pump();
    // "Enter address...", after the two servers.
    r.tap(row(2));
    assert_eq!(r.screen(), "provision-server");
    // The letters are as wide as ten in a row allow, and tall enough.
    for (rect, hit) in r.regions() {
        if matches!(hit, Hit::Custom(_)) {
            assert!(rect.h >= th().touch_min && rect.w >= 70, "{hit:?} is {rect:?}");
        }
    }
    r.tap(Cap::Shift.hit());
    r.type_keys("P");
    // One capital, then lower case again.
    assert!(r.ui.canvas().hit_rect(Cap::Char('B').hit()).is_none());
    r.type_keys("bx.example");
    r.tap(Cap::Symbols.hit());
    r.type_keys(":5060");
    // A finger in the field, at the left end of the text: the cursor goes
    // there, and what is typed goes in there.
    let field = r.region(Hit::Custom(1));
    r.tap_at(field.x + th().pad + 2, field.y + field.h / 2);
    r.tap(Cap::Letters.hit());
    r.type_keys("x");
    r.tap(Cap::Backspace.hit());
    r.tap(Cap::Done.hit());
    assert_eq!(taken(&r.provisioning), vec![Command::StartProvisioning { server: Some("Pbx.example:5060".into()) }]);
}

#[test]
fn the_call_log_has_tabs_details_and_clear() {
    let mut r = Rig::new();
    r.store.update::<CallLog, _>(|l| {
        for (i, kind) in [CallLogKind::Placed, CallLogKind::Missed, CallLogKind::Received, CallLogKind::Missed].into_iter().enumerate() {
            l.entries.push_back(CallLogEntry {
                id: i as u64,
                kind,
                party: Party { name: format!("Party {i}"), number: format!("20{i}"), uri: String::new() },
                account: AccountId(0),
                start_unix: 1_794_600_000,
                duration_secs: 10,
            });
        }
    });
    r.show("menu");
    r.tap(row(MENU_CALL_LOG));
    assert_eq!(r.screen(), "call_log");
    all_large(&r);
    assert_eq!(r.rows(), vec![0, 1, 2, 3]);
    // The Missed tab: the two missed calls.
    r.tap(Hit::Custom(2));
    assert_eq!(r.rows(), vec![0, 1]);
    // The details of the second: from them, it is dialed.
    r.tap(Hit::Custom(0x1_0000 + 1));
    assert_eq!(r.screen(), "call_log_entry");
    r.tap(Hit::Soft(1));
    assert_eq!(taken(&r.calls), vec![Command::Dial { target: "203".into(), account: Some(AccountId(0)), path: None }]);
    assert_eq!(r.screen(), "idle");

    // A tap on an entry dials it.
    r.show("call_log");
    r.tap(Hit::Custom(3));
    assert_eq!(r.rows(), vec![0]);
    r.tap(row(0));
    assert_eq!(taken(&r.calls), vec![Command::Dial { target: "202".into(), account: Some(AccountId(0)), path: None }]);

    // Clear asks first.
    r.show("call_log");
    r.tap(Hit::Soft(3));
    assert_eq!(r.screen(), "confirm");
    r.tap(Hit::Soft(1));
    assert_eq!(taken(&r.calls), vec![Command::ClearCallLog]);
}

#[test]
fn an_app_is_launched_from_the_grid_and_the_grid_scrolls() {
    let mut r = Rig::new();
    r.store.update::<Apps, _>(|a| {
        for i in 0..20 {
            a.installed.push(AppInfo { name: format!("app{i}"), display_name: format!("App {i}"), ..Default::default() });
        }
    });
    r.show("apps");
    all_large(&r);
    // Three to a row, the second row under the first.
    let (a, b, d) = (r.region(Hit::Custom(0)), r.region(Hit::Custom(1)), r.region(Hit::Custom(3)));
    assert_eq!(a.y, b.y);
    assert!(b.x > a.x && d.x == a.x && d.y > a.y);
    r.tap(Hit::Custom(4));
    assert_eq!(taken(&r.apps), vec![Command::LaunchApp { name: "app4".into() }]);
    // A finger up the screen: the first row goes, the last one comes.
    assert!(r.ui.canvas().hit_rect(Hit::Custom(19)).is_none());
    for _ in 0..3 {
        r.sim.touch_drag((400, 1100), (400, 300), 8);
        r.ui.pump();
    }
    assert!(r.ui.canvas().hit_rect(Hit::Custom(0)).is_none());
    assert!(r.region(Hit::Custom(19)).bottom() <= th().height);
    assert!(taken(&r.apps).is_empty());
}

/// The setting a command of the configuration's sets.
fn setting(cmds: Vec<Command>) -> Vec<(String, String)> {
    cmds.into_iter()
        .flat_map(|c| match c {
            Command::SetSettings { settings } => settings,
            other => panic!("{other:?}"),
        })
        .collect()
}

fn pair(id: &str, value: &str) -> (String, String) {
    (id.into(), value.into())
}

#[test]
fn a_ringtone_is_heard_and_chosen_by_a_tap() {
    let mut r = Rig::new();
    r.show("menu");
    r.tap(row(MENU_SETTINGS));
    r.tap(row(SETTINGS_SOUNDS));
    assert_eq!(r.screen(), "sounds");
    r.tap(row(0));
    assert_eq!(r.screen(), "ringtones");
    // No buttons: a tap sounds the ringtone and makes it the ringtone.
    assert!(r.regions().iter().all(|(_, h)| !matches!(h, Hit::Soft(_))));
    let check = |r: &Rig, i: u32| {
        let at = r.region(row(i));
        // The check mark, in the accent, at the left of the row.
        let th = shown_th();
        (at.x..at.x + th.back_w).any(|x| (at.y..at.bottom()).any(|y| r.ui.canvas().pixel(x, y) == th.accent))
    };
    assert!(check(&r, 0) && !check(&r, 1));
    r.tap(row(1));
    assert_eq!(taken(&r.audio), vec![Command::PlaySound { name: "Acheron".into() }]);
    assert_eq!(setting(taken(&r.config)), vec![pair("default_ringtone", "Acheron")]);
    assert!(check(&r, 1) && !check(&r, 0));
    r.tap(row(2));
    assert_eq!(taken(&r.audio), vec![Command::PlaySound { name: "Andromeda".into() }]);
    assert_eq!(setting(taken(&r.config)), vec![pair("default_ringtone", "Andromeda")]);
    // The back button stops the sound and keeps the ringtone.
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(taken(&r.audio), vec![Command::StopSound]);
    assert!(taken(&r.config).is_empty());
    assert_eq!(r.screen(), "sounds");
}

#[test]
fn a_level_is_set_with_its_buttons_and_slider() {
    let mut r = Rig::new();
    r.store.update::<opendeskphone_state::Audio, _>(|a| a.ringer_volume = 5);
    r.show("menu");
    r.tap(row(MENU_SETTINGS));
    r.tap(row(SETTINGS_SOUNDS));
    r.tap(row(1));
    assert_eq!(r.screen(), "level");
    all_large(&r);
    assert!(r.ui.canvas().hits().iter().all(|(_, h)| !matches!(h, Hit::Soft(_))));
    let level = |r: &Rig| -> Vec<u8> {
        taken(&r.audio)
            .into_iter()
            .map(|c| match c {
                Command::SetVolume { target: VolumeTarget::Ringer, level } => level,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    r.tap(Hit::Custom(2));
    r.tap(Hit::Custom(2));
    r.tap(Hit::Custom(1));
    assert_eq!(level(&r), vec![6, 7, 6]);
    // The slider's ends are none and the most.
    let track = r.region(Hit::Custom(3));
    r.tap_at(track.x + 1, track.y + track.h / 2);
    r.tap_at(track.right() - 1, track.y + track.h / 2);
    assert_eq!(level(&r), vec![0, VOLUME_MAX]);
    // A finger along the slider, from the middle to the left, by about
    // three tenths of it.
    let mid = (track.x + track.w / 2, track.y + track.h / 2);
    r.tap_at(mid.0, mid.1);
    assert_eq!(level(&r), vec![5]);
    r.sim.touch_drag((mid.0 as u16, mid.1 as u16), ((mid.0 - track.w * 3 / 10) as u16, mid.1 as u16), 10);
    r.ui.pump();
    let steps = level(&r);
    assert_eq!(steps.last(), Some(&2), "{steps:?}");
    // The back button leaves it there.
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "sounds");
}

#[test]
fn a_theme_is_chosen_and_shown_by_a_tap() {
    let mut r = Rig::new();
    r.show("menu");
    r.tap(row(MENU_SETTINGS));
    r.tap(row(SETTINGS_DISPLAY));
    assert_eq!(r.screen(), "display");
    // Brightness, Dim when idle, Backlight timeout, Theme.
    r.tap(row(3));
    assert_eq!(r.screen(), "theme");
    all_large(&r);
    // No buttons: a tap is the choice.
    assert!(r.regions().iter().all(|(_, h)| !matches!(h, Hit::Soft(_))));
    // The idle screen above the sheet is only to be looked at: its lines
    // are not line keys here.
    let line = th().tab_rect(0);
    assert_eq!(r.ui.canvas().hit_at(line.x + line.w / 2, line.y + line.h / 2), Some(Hit::Nothing));
    // The theme in use is the second row shown: the first is another.
    let shown = r.rows();
    let other = shown[0];
    let title = r.region(Hit::Key(Key::Cancel));
    let before = r.ui.canvas().pixel(title.right() + 4, title.y + 4);
    r.tap(row(other));
    assert_eq!(r.screen(), "theme");
    let after = r.ui.canvas().pixel(title.right() + 4, title.y + 4);
    assert_ne!(before, after, "the sheet's title bar is not in the theme touched");
    let id = opendeskphone_ui::wallpaper::all()[other as usize].id;
    assert_eq!(taken(&r.config), vec![Command::SetTheme { name: id.into() }]);
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "display");
    assert!(taken(&r.config).is_empty());
}

#[test]
fn forwarding_is_switched_and_keeps_its_number() {
    let mut r = Rig::new();
    r.store.update::<Features, _>(|f| f.forward.always = "1099".into());
    r.show("menu");
    r.tap(row(MENU_FORWARD));
    assert_eq!(r.screen(), "forward");
    all_large(&r);
    // Always (on), its number under it, When busy, No answer: no Do Not
    // Disturb.
    assert_eq!(r.rows(), vec![0, 1, 2, 3]);
    let fwd = |kind, target: &str| Command::SetForward { kind, target: target.into(), no_answer_secs: None };
    // Off, and on again to the same number.
    r.tap(row(0));
    assert_eq!(taken(&r.calls), vec![fwd(ForwardKind::Always, "")]);
    r.store.update::<Features, _>(|f| f.forward.always.clear());
    r.ui.pump();
    assert_eq!(r.rows(), vec![0, 1, 2, 3], "the number is kept");
    r.tap(row(0));
    assert_eq!(taken(&r.calls), vec![fwd(ForwardKind::Always, "1099")]);
    // A rule with no number asks for one, and forwards to it.
    r.tap(row(2));
    assert_eq!(r.screen(), "forward_edit");
    r.type_keys("1234");
    r.tap(opendeskphone_ui::osk::Cap::Done.hit());
    assert_eq!(taken(&r.calls), vec![fwd(ForwardKind::Busy, "1234")]);
    assert_eq!(r.screen(), "forward");
    // The number of a rule that is off is changed, and kept, and not
    // forwarded to until the rule is switched on.
    r.tap(row(1));
    assert_eq!(r.screen(), "forward_edit");
    for _ in 0..4 {
        r.tap(opendeskphone_ui::osk::Cap::Backspace.hit());
    }
    r.type_keys("2000");
    r.tap(opendeskphone_ui::osk::Cap::Done.hit());
    assert!(taken(&r.calls).is_empty());
    r.tap(row(0));
    assert_eq!(taken(&r.calls), vec![fwd(ForwardKind::Always, "2000")]);
    // That of a rule that is on is forwarded to at once.
    r.store.update::<Features, _>(|f| f.forward.always = "2000".into());
    r.ui.pump();
    r.tap(row(1));
    r.type_keys("1");
    r.tap(opendeskphone_ui::osk::Cap::Done.hit());
    assert_eq!(taken(&r.calls), vec![fwd(ForwardKind::Always, "20001")]);
}

#[test]
fn display_settings_are_switched_and_chosen() {
    let mut r = Rig::new();
    r.show("menu");
    r.tap(row(MENU_SETTINGS));
    r.tap(row(SETTINGS_DISPLAY));
    // Dim when idle: a switch, which a tap turns.
    r.tap(row(1));
    assert_eq!(setting(taken(&r.config)), vec![pair("dim_backlight", "0")]);
    assert_eq!(r.screen(), "display");
    // The backlight's timeout: a choice of five, a tap choosing.
    r.tap(row(2));
    assert_eq!(r.screen(), "backlight_timeout");
    all_large(&r);
    assert_eq!(r.rows(), vec![0, 1, 2, 3, 4]);
    r.tap(row(3));
    assert_eq!(setting(taken(&r.config)), vec![pair("backlight_timeout", "300")]);
    r.tap(row(4));
    assert_eq!(setting(taken(&r.config)), vec![pair("backlight_timeout", "0")]);
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "display");
}

#[test]
fn addressing_and_vlan_show_their_settings_and_turn_them() {
    use opendeskphone_ui::osk::Cap;
    let mut r = Rig::new();
    r.show("network");
    // DHCP, and in use: a tap asks for a static address.
    r.tap(row(0));
    assert_eq!(setting(taken(&r.config)), vec![pair("network_enable_dhcp", "0")]);
    // The configuration says so (the network has yet to be applied): the
    // next tap is back to DHCP.
    let cfg = UiConfig { network_dhcp: false, vlan_discovery: "LLDP".into(), backlight_timeout_secs: 0, ..Default::default() };
    r.ui.handle().set_config(cfg);
    r.ui.pump();
    r.tap(row(0));
    assert_eq!(setting(taken(&r.config)), vec![pair("network_enable_dhcp", "1")]);
    // VLAN discovery by LLDP: next, manual.
    r.tap(row(6));
    assert_eq!(setting(taken(&r.config)), vec![pair("network_vlan_discovery_mode", "MANUAL")]);
    // The VLAN ID is a number, not a secret: the pad of numbers, not of
    // a PIN.
    r.tap(row(7));
    assert_eq!(r.screen(), "setting_edit");
    assert!(r.ui.canvas().hit_rect(Cap::Char('.').hit()).is_some());
}

#[test]
fn factory_reset_is_last_apart_and_red() {
    let mut r = Rig::new();
    r.show("menu");
    r.tap(row(MENU_SETTINGS));
    r.tap(row(SETTINGS_RESET));
    assert_eq!(r.screen(), "reset");
    // Restart, Reconfigure, Forget server, a gap, Factory reset.
    assert_eq!(r.rows(), vec![0, 1, 2, 3, 4]);
    r.tap(row(3));
    assert_eq!(r.screen(), "reset");
    let at = r.region(row(4));
    let red = (at.x..at.right()).any(|x| (at.y..at.bottom()).any(|y| r.ui.canvas().pixel(x, y) == shown_th().error));
    assert!(red, "Factory reset is not red");
    r.tap(row(4));
    assert_eq!(r.screen(), "confirm");
    r.tap(Hit::Soft(1));
    assert_eq!(taken(&r.system), vec![Command::FactoryReset]);
}

#[test]
fn the_call_log_is_today_s_times_a_wide_clear_and_a_green_call() {
    let mut r = Rig::new();
    // The fixed clock is 2026-11-13 21:58 UTC.
    r.store.update::<CallLog, _>(|l| {
        for (i, start) in [1_794_600_000u64, 1_794_500_000].into_iter().enumerate() {
            l.entries.push_back(CallLogEntry {
                id: i as u64,
                kind: CallLogKind::Received,
                party: Party { name: format!("Party {i}"), number: format!("20{i}"), uri: String::new() },
                account: AccountId(0),
                start_unix: start,
                duration_secs: 75,
            });
        }
    });
    r.show("call_log");
    all_large(&r);
    let th = shown_th();
    let clear = r.region(Hit::Soft(3));
    assert!(clear.w >= th.width * 9 / 10, "{clear:?}");
    // Today's entry has its time only: it begins farther right than
    // yesterday's, which has the date before it.
    let leftmost = |r: &Rig, i: u32| {
        let at = r.region(row(i));
        let details = r.region(Hit::Custom(0x1_0000 + i));
        (at.x + at.w / 2..details.x).find(|x| (at.y..at.bottom()).any(|y| r.ui.canvas().pixel(*x, y) == th.secondary)).expect("a time")
    };
    assert!(leftmost(&r, 0) > leftmost(&r, 1) + th.px(40), "{} {}", leftmost(&r, 0), leftmost(&r, 1));
    // The details: a green button across the bottom calls the number.
    r.tap(Hit::Custom(0x1_0000));
    assert_eq!(r.screen(), "call_log_entry");
    let call = r.region(Hit::Soft(1));
    assert!(call.w >= th.width * 9 / 10);
    assert_eq!(r.ui.canvas().pixel(call.x + th.pad, call.y + call.h / 2), th.sk_go, "Call is not green");
    r.tap(Hit::Soft(1));
    assert_eq!(taken(&r.calls), vec![Command::Dial { target: "200".into(), account: Some(AccountId(0)), path: None }]);
    // Empty: a sentence, and no Clear.
    r.store.update::<CallLog, _>(|l| l.entries.clear());
    r.show("call_log");
    assert!(r.ui.canvas().hit_rect(Hit::Soft(3)).is_none());
    assert!(r.rows().is_empty());
}

#[test]
fn every_menu_is_operated_by_touch_alone() {
    let mut r = Rig::new();
    r.store.update::<CallLog, _>(|l| {
        l.entries.push_back(CallLogEntry { id: 1, kind: CallLogKind::Missed, party: Party::default(), account: AccountId(0), start_unix: 0, duration_secs: 0 });
    });
    // Each screen of the menus, reached by touch from the menu (rows that
    // do something at once left out): what it shows is large enough to
    // touch, and the back button (a question's No) goes back.
    let walks: &[&[u32]] = &[
        &[0],
        &[1],
        &[2],
        &[2, 0],
        &[4],
        &[MENU_SETTINGS],
        &[MENU_SETTINGS, 0],
        &[MENU_SETTINGS, 0, 7],
        &[MENU_SETTINGS, 0, 9],
        &[MENU_SETTINGS, 1],
        &[MENU_SETTINGS, 2],
        &[MENU_SETTINGS, 2, 2],
        &[MENU_SETTINGS, 3],
        &[MENU_SETTINGS, 4],
        &[MENU_SETTINGS, 4, 0],
        &[MENU_SETTINGS, 4, 2],
        &[MENU_SETTINGS, 4, 4],
        &[7],
        &[7, 0],
        &[7, 1],
        &[7, 2],
        &[7, 3],
    ];
    for walk in walks {
        r.show("menu");
        let mut names = vec![r.screen()];
        for i in *walk {
            r.tap(row(*i));
            names.push(r.screen());
        }
        let name = r.screen();
        assert_ne!(names[names.len() - 2], name, "{walk:?}: {names:?}");
        assert!(!r.regions().is_empty(), "{name}");
        for (rect, hit) in r.regions() {
            // (The keyboard's letters are as wide as ten in a row allow.)
            let w = if matches!(hit, Hit::Custom(id) if id >= opendeskphone_ui::osk::HIT_BASE) { 70 } else { th().touch_min };
            assert!(rect.w >= w && rect.h >= th().touch_min, "{name}: {hit:?} is {rect:?}");
            assert!(rect.bottom() <= th().height && rect.right() <= th().width);
        }
        r.tap(if name == "confirm" { Hit::Soft(4) } else { Hit::Key(Key::Cancel) });
        assert_eq!(r.screen(), names[names.len() - 2], "{walk:?}: {names:?}");
    }
    // Nothing was done along the way.
    for rx in [&r.calls, &r.config, &r.audio, &r.apps] {
        let cmds: Vec<Command> = taken(rx).into_iter().filter(|c| *c != Command::MarkMissedSeen).collect();
        assert!(cmds.is_empty(), "{cmds:?}");
    }
}
