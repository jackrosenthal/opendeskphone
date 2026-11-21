//! The touch skin (D80), through the HAL simulator's touchscreen: taps
//! reach the regions the screens register as they draw, the softkeys are
//! buttons, menus are operated by touching their items, a finger scrolls
//! a list, and what is touched is large enough to touch.

use opendeskphone_hal::Hal;
use opendeskphone_state::*;
use opendeskphone_ui::gfx::IRect;
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::{Hit, Ui, UiConfig};

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    calls: CommandReceiver,
}

impl Rig {
    fn new(model: Model) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        store.update::<Accounts, _>(|a| {
            let mut acct = Account::new(AccountId(0));
            acct.label = "100".into();
            a.accounts.push(acct);
        });
        let calls = store.handle(Domain::Calls);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.set_fixed_time(Some((5_000_000, 1_794_607_087)));
        ui.pump();
        Rig { store, ui, sim, calls }
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
    /// A finger down and up in the middle of `r`.
    fn tap_in(&mut self, r: IRect) {
        self.sim.touch_tap((r.x + r.w / 2) as u16, (r.y + r.h / 2) as u16);
        self.ui.pump();
    }
    fn tap(&mut self, hit: Hit) {
        self.tap_in(self.region(hit));
    }
    fn softkeys(&self) -> Vec<(IRect, u32)> {
        self.ui.canvas().hits().iter().filter_map(|(r, h)| if let Hit::Soft(id) = h { Some((*r, *id)) } else { None }).collect()
    }
    fn call_cmds(&self) -> Vec<Command> {
        let mut v = Vec::new();
        while let Some(env) = self.calls.try_recv() {
            v.push(env.command.clone());
            env.done();
        }
        v
    }
    fn fill_call_log(&mut self, n: usize) {
        self.store.update::<CallLog, _>(|l| {
            for i in 0..n {
                l.entries.push_back(CallLogEntry {
                    id: i as u64,
                    kind: CallLogKind::Placed,
                    party: Party { name: String::new(), number: format!("2{i:02}"), uri: String::new() },
                    account: AccountId(0),
                    start_unix: 1_794_600_000,
                    duration_secs: 10,
                });
            }
        });
        self.ui.pump();
    }
}

fn row(row: u32) -> Hit {
    Hit::Row { list: 0, row }
}

#[test]
fn a_menu_is_operated_by_touching_its_items() {
    let mut r = Rig::new(Model::D80);
    r.show("menu");
    // No softkeys ("Select" and "Back" are a cursor's), and no row drawn
    // as the selected one: the first row's fill is the background's.
    assert!(r.softkeys().is_empty());
    let th = Theme::for_model(Model::D80);
    let first = r.region(row(0));
    assert_eq!(first.h, th.row_h);
    assert!(first.h >= th.touch_min);
    // Rows are told apart by where the finger is: the first item is the
    // call log, the second the apps.
    r.tap(row(1));
    assert_eq!(r.screen(), "apps");
    // The back button of the title bar is the X key.
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "menu");
    r.tap(row(0));
    assert_eq!(r.screen(), "call_log");
    r.tap(Hit::Key(Key::Cancel));
    r.tap(Hit::Key(Key::Cancel));
    assert_eq!(r.screen(), "idle");
}

#[test]
fn a_touch_outside_every_region_does_nothing() {
    let mut r = Rig::new(Model::D80);
    r.show("menu");
    // Below the last item.
    assert_eq!(r.ui.canvas().hit_at(400, 1200), None);
    r.sim.touch_tap(400, 1200);
    r.ui.pump();
    assert_eq!(r.screen(), "menu");
}

#[test]
fn softkeys_are_buttons_all_shown() {
    let mut r = Rig::new(Model::D80);
    assert_eq!(r.screen(), "idle");
    let th = Theme::for_model(Model::D80);
    // The idle screen has seven softkeys by default: all of them are on
    // the screen, in two rows, with no "More".
    let keys = r.softkeys();
    assert_eq!(keys.len(), 7);
    assert!(keys.iter().all(|(_, id)| *id != u32::MAX));
    let rows: std::collections::BTreeSet<i32> = keys.iter().map(|(rect, _)| rect.y).collect();
    assert_eq!(rows.len(), 2);
    for (i, (rect, _)) in keys.iter().enumerate() {
        assert!(rect.w >= th.touch_min && rect.h >= th.touch_min, "{rect:?}");
        assert!(rect.bottom() <= th.height && rect.right() <= th.width);
        for (other, _) in &keys[i + 1..] {
            assert!(rect.intersect(other).is_empty());
        }
    }
    // One of them is the menu's.
    let mut opened = Vec::new();
    for (rect, _) in keys {
        r.tap_in(rect);
        opened.push(r.screen());
        r.show("idle");
    }
    assert!(opened.contains(&"menu".to_string()), "{opened:?}");
    assert!(opened.contains(&"call_log".to_string()), "{opened:?}");
}

#[test]
fn the_lines_are_on_the_screen_and_are_the_line_keys() {
    let r = Rig::new(Model::D80);
    // The D80 has no line keys, and shows its lines.
    assert_eq!(Model::D80.line_keys(), 0);
    assert!(Model::D80.lines() > 0);
    let th = Theme::for_model(Model::D80);
    // (A free line is the idle screen's own region: it opens the dialer
    // for the line's account.)
    let line = r.region(Hit::Custom(101));
    assert!(line.h >= th.touch_min && line.w >= th.touch_min);
    // A model with keys registers nothing.
    let d60 = Rig::new(Model::D60);
    assert!(d60.ui.canvas().hits().is_empty());
}

#[test]
fn a_tap_on_a_row_of_a_list_selects_and_activates_it() {
    let mut r = Rig::new(Model::D80);
    r.fill_call_log(5);
    r.show("call_log");
    r.tap(row(3));
    assert_eq!(r.call_cmds(), vec![Command::Dial { target: "203".into(), account: Some(AccountId(0)), path: None }]);
}

#[test]
fn a_finger_scrolls_a_list() {
    let mut r = Rig::new(Model::D80);
    r.fill_call_log(40);
    r.show("call_log");
    let th = Theme::for_model(Model::D80);
    let top = r.region(row(0));
    // Up by three rows and a bit: the fourth row is where the first was.
    let from = (400u16, (top.y + 5 * th.row_h) as u16);
    let to = (400u16, from.1 - (3 * th.row_h + 10) as u16);
    r.sim.touch_drag(from, to, 8);
    r.ui.pump();
    assert_eq!(r.screen(), "call_log");
    assert_eq!(r.region(row(3)), top);
    assert!(r.ui.canvas().hit_rect(row(0)).is_none());
    // The drag dialed nothing.
    assert!(r.call_cmds().is_empty());
    // Down again by more than there is: the list stops at its first row.
    r.sim.touch_drag(to, (400, 1200), 8);
    r.ui.pump();
    assert_eq!(r.region(row(0)), top);
    // And up by more than there is: the last row is the last one shown.
    for _ in 0..8 {
        r.sim.touch_drag((400, 1100), (400, 200), 8);
        r.ui.pump();
    }
    let last = r.region(row(39));
    assert!(last.bottom() <= th.height);
    assert!(r.ui.canvas().hit_rect(row(0)).is_none());
}

#[test]
fn a_touch_takes_a_notice_down_and_does_nothing_else() {
    let mut r = Rig::new(Model::D80);
    r.show("menu");
    let item = r.region(row(1));
    let p = r.store.send(Command::Notify { level: NoticeLevel::Info, title: "Notice".into(), text: "Text".into(), timeout_ms: 0 });
    r.ui.pump();
    assert!(matches!(p.wait(), Ok(Reply::Id(_))));
    assert_eq!(r.store.get::<opendeskphone_state::Ui>().notices.len(), 1);
    // On the item under the notice's surroundings: the notice goes, the
    // menu stays.
    r.tap_in(item);
    assert!(r.store.get::<opendeskphone_state::Ui>().notices.is_empty());
    assert_eq!(r.screen(), "menu");
    // Now the item is touched.
    r.tap_in(item);
    assert_eq!(r.screen(), "apps");
}

#[test]
fn the_volume_popup_has_its_keys_and_goes_when_touched_beside() {
    let mut r = Rig::new(Model::D80);
    let audio = r.store.handle(Domain::Audio);
    // (A volume key from elsewhere, e.g., the web mirror, raises it.)
    r.sim.tap(Key::VolumeUp);
    r.ui.pump();
    while let Some(env) = audio.try_recv() {
        env.done();
    }
    let plus = r.region(Hit::Key(Key::VolumeUp));
    let th = Theme::for_model(Model::D80);
    assert!(plus.w >= th.touch_min && plus.h >= th.touch_min);
    r.tap_in(plus);
    let cmds: Vec<Command> = std::iter::from_fn(|| audio.try_recv()).map(|env| env.command.clone()).collect();
    assert_eq!(cmds, vec![Command::AdjustVolume { target: VolumeTarget::Current, delta: 1 }]);
    // Still up; a touch beside it takes it down and opens nothing.
    assert!(r.ui.canvas().hit_rect(Hit::Key(Key::VolumeDown)).is_some());
    r.sim.touch_tap(400, 100);
    r.ui.pump();
    assert!(r.ui.canvas().hit_rect(Hit::Key(Key::VolumeDown)).is_none());
    assert_eq!(r.screen(), "idle");
}

#[test]
fn touches_are_injected_by_command_on_the_d80_only() {
    let mut r = Rig::new(Model::D80);
    r.show("menu");
    let item = r.region(row(0));
    let (x, y) = ((item.x + 40) as u16, (item.y + 40) as u16);
    for phase in [TouchPhase::Down, TouchPhase::Up] {
        let p = r.store.send(Command::InjectTouch { phase, x, y });
        r.ui.pump();
        assert_eq!(p.wait(), Ok(Reply::Done));
    }
    assert_eq!(r.screen(), "call_log");

    let mut d60 = Rig::new(Model::D60);
    let p = d60.store.send(Command::InjectTouch { phase: TouchPhase::Down, x: 10, y: 10 });
    d60.ui.pump();
    assert!(matches!(p.wait(), Err(CommandError::Invalid(_))));
}

#[test]
fn a_long_press_of_a_button_presses_it() {
    let mut r = Rig::new(Model::D80);
    r.show("menu");
    let back = r.region(Hit::Key(Key::Cancel));
    let (x, y) = ((back.x + back.w / 2) as u16, (back.y + back.h / 2) as u16);
    r.sim.touch(TouchPhase::Down, x, y);
    r.ui.pump();
    r.ui.set_fixed_time(Some((5_000_000 + opendeskphone_ui::touch::LONG_PRESS_MS, 1_794_607_087)));
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
    // Lifting the finger does nothing more.
    r.sim.touch(TouchPhase::Up, x, y);
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
}

#[test]
fn what_is_touched_is_large_enough() {
    let th = Theme::for_model(Model::D80);
    let mut r = Rig::new(Model::D80);
    r.fill_call_log(3);
    for screen in ["idle", "menu", "settings", "call_log", "forward", "about", "network", "status"] {
        r.show(screen);
        let hits = r.ui.canvas().hits().to_vec();
        assert!(!hits.is_empty(), "{screen}");
        for (rect, hit) in hits {
            assert!(rect.w >= th.touch_min && rect.h >= th.touch_min, "{screen}: {hit:?} is {rect:?}");
        }
    }
}

#[test]
fn the_wallpaper_fills_the_screen() {
    let mut r = Rig::new(Model::D80);
    r.show("idle");
    // The four corners are the wallpaper's, not one flat background: the
    // picture differs from top to bottom.
    let c = r.ui.canvas();
    assert_eq!((c.width(), c.height()), (800, 1280));
    let column: std::collections::BTreeSet<u32> = (0..1000).step_by(50).map(|y| c.pixel(700, y)).collect();
    assert!(column.len() > 1, "{column:?}");
}
