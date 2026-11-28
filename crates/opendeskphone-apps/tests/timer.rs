//! The Timer app: a countdown timer and a stopwatch, under the real
//! runtime, on the phone's clock.
//!
//! The last test draws the app's screens with the phone UI on a simulated
//! D60 (color), D70, D40 (gray), and D80 (touch), and writes them as PNG
//! files under `timer/` in cargo's test scratch directory
//! (`CARGO_TARGET_TMPDIR`), or in the directory `TIMER_PREVIEWS` names.

mod common;

use common::{Rig, staged, temp_dir};
use opendeskphone_apps::engine::Target;
use opendeskphone_apps::testutil::{FakeConfig, FakePbx, services};
use opendeskphone_apps::uishell::UiShell;
use opendeskphone_apps::{AppManager, DrawOp, ManagerConfig};
use opendeskphone_hal::{Hal, Screen};
use opendeskphone_state::{
    AccountId, Alarms, Call, CallId, CallState, Calls, Command, Direction, Domain, Key, Model,
    Party, Phase, Reply, Store, System,
};
use opendeskphone_ui::{Ui, UiConfig};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const APP: &str = "timer";

/// A phone with two alarm tones, whose commands to the audio side are
/// kept.
fn rig(model: Model) -> (Rig, Arc<Mutex<Vec<Command>>>) {
    let r = Rig::with_root(staged(), FakeConfig::default(), model);
    r.store
        .update::<Alarms, _>(|a| a.tones = ["Beep", "Buzzer"].map(String::from).to_vec());
    let audio = Arc::new(Mutex::new(Vec::new()));
    common::answer_commands(&r.store, Domain::Audio, audio.clone(), |_| Ok(Reply::Done));
    (r, audio)
}

/// The time on screen, read from its characters' canvases (each drawn as
/// `timer_face` draws the character in one of its faces at the canvas's
/// size), and the part of it that is highlighted.
fn time(r: &Rig) -> (String, String) {
    let (mut text, mut lit) = (String::new(), String::new());
    for (at, ops) in r.ui.drawings(APP) {
        let bits: Vec<Vec<u8>> = ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Bitmap(b) => Some(b.bits.clone()),
                _ => None,
            })
            .collect();
        if bits.is_empty() {
            continue;
        }
        let drawn = |c: char| {
            [&timer_face::ROBOTO, &timer_face::HELVETICA]
                .iter()
                .any(|face| {
                    face.cell(c, at.height).is_some_and(|cell| {
                        cell.height == at.height
                            && cell.width == at.width
                            && timer_face::layers(&cell)
                                .into_iter()
                                .map(|(_, b)| b)
                                .eq(bits.iter().cloned())
                    })
                })
        };
        let c = "0123456789:.".chars().find(|c| drawn(*c)).unwrap_or('?');
        text.push(c);
        if matches!(ops.first(), Some(DrawOp::Fill(_))) {
            lit.push(c);
        }
    }
    (text, lit)
}

fn shown(time: &str, lit: &str) -> (String, String) {
    (time.to_string(), lit.to_string())
}

fn type_digits(r: &mut Rig, digits: &str) {
    for d in digits.bytes() {
        r.key(APP, Key::Digit(d - b'0'));
    }
}

/// Runs the phone until `done`, for up to five seconds.
fn until(r: &mut Rig, what: &str, done: impl Fn(&Rig) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(r) {
        assert!(Instant::now() < deadline, "{what}");
        r.settle();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn on_screen(r: &Rig) -> bool {
    r.ui.top().as_deref() == Some(APP)
}

fn tones(audio: &Mutex<Vec<Command>>) -> usize {
    audio
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, Command::PlayAlarmTone { .. }))
        .count()
}

#[test]
fn a_timer_comes_to_the_front_and_rings_when_its_time_is_up() {
    let (mut r, audio) = rig(Model::D60);
    r.launch(APP);
    assert_eq!(r.ui.texts(APP)[0], "Timer");
    // The minutes are chosen.
    assert_eq!(time(&r), shown("00:00:00", "00"));
    assert_eq!(r.ui.softkeys(APP), ["Start", "Stopwatch", "Exit"]);
    // Nothing set: Start does nothing.
    r.softkey(APP, "Start");
    assert_eq!(r.ui.softkeys(APP)[0], "Start");

    // Up and down change the chosen field, around its range; left and
    // right choose another.
    r.key(APP, Key::Up);
    assert_eq!(time(&r), shown("00:01:00", "01"));
    r.key(APP, Key::Down);
    r.key(APP, Key::Down);
    assert_eq!(time(&r), shown("00:59:00", "59"));
    r.key(APP, Key::Left);
    r.key(APP, Key::Left);
    assert_eq!(time(&r), shown("00:59:00", "00"), "the hours");
    r.key(APP, Key::Right);
    r.key(APP, Key::Right);
    r.key(APP, Key::Right);
    assert_eq!(time(&r).1, "00", "the seconds");
    // Two digits type a field and go on to the next.
    r.key(APP, Key::Left);
    type_digits(&mut r, "13");
    assert_eq!(time(&r), shown("00:13:00", "00"));
    r.key(APP, Key::Left);
    type_digits(&mut r, "002");
    assert_eq!(time(&r), shown("00:00:02", "02"));

    r.key(APP, Key::Select);
    assert_eq!(time(&r), shown("0:02", ""));
    assert_eq!(r.ui.softkeys(APP), ["Pause", "Cancel", "Stopwatch", "Exit"]);
    // A pause keeps the time left.
    r.softkey(APP, "Pause");
    assert!(r.ui.shows(APP, "Paused"));
    assert_eq!(r.ui.softkeys(APP)[0], "Resume");
    std::thread::sleep(Duration::from_millis(1200));
    r.settle();
    assert_eq!(time(&r).0, "0:02");
    r.softkey(APP, "Resume");

    // Off the screen, the timer keeps running, and comes back when the
    // time is up.
    r.softkey(APP, "Exit");
    assert!(!on_screen(&r) && r.mgr.is_running(APP));
    until(&mut r, "the timer came to the front", on_screen);
    r.settle();
    assert!(r.ui.shows(APP, "Time's up"), "{:?}", r.ui.texts(APP));
    assert_eq!(time(&r).0, "0:00");
    assert_eq!(r.ui.softkeys(APP), ["Dismiss"]);
    assert_eq!(
        common::recorded(&audio, 1),
        [Command::PlayAlarmTone {
            name: "Beep".into()
        }]
    );
    // The tone starts again until the timer is dismissed.
    until(&mut r, "the tone started again", |_| tones(&audio) == 2);

    r.softkey(APP, "Dismiss");
    assert_eq!(common::recorded(&audio, 3)[2], Command::StopSound);
    // The duration stays, for another timer.
    assert_eq!(time(&r), shown("00:00:02", "02"));
    // With nothing running, the app stops when it leaves the screen.
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
}

#[test]
fn a_timer_does_not_ring_into_a_call() {
    let (mut r, audio) = rig(Model::D60);
    r.launch(APP);
    r.key(APP, Key::Right);
    type_digits(&mut r, "1");
    r.softkey(APP, "Start");
    r.softkey(APP, "Exit");
    let mut call = Call::new(
        CallId(3),
        AccountId(0),
        Direction::Incoming,
        Party::default(),
    );
    call.state = CallState::Active;
    r.store.update::<Calls, _>(|c| c.calls.push(call));
    std::thread::sleep(Duration::from_millis(1500));
    r.settle();
    assert!(!on_screen(&r));
    assert_eq!(tones(&audio), 0);
    // The call is over: the timer rings.
    r.store.update::<Calls, _>(|c| c.calls.clear());
    until(&mut r, "the timer came to the front", on_screen);
    assert_eq!(
        common::recorded(&audio, 1),
        [Command::PlayAlarmTone {
            name: "Beep".into()
        }]
    );
}

#[test]
fn the_stopwatch_keeps_laps_and_runs_off_the_screen() {
    let (mut r, _) = rig(Model::D60);
    r.launch(APP);
    r.softkey(APP, "Stopwatch");
    assert_eq!(r.ui.texts(APP), ["Stopwatch"]);
    assert_eq!(time(&r), shown("0:00.0", ""));
    assert_eq!(r.ui.softkeys(APP), ["Start", "Timer", "Exit"]);

    r.key(APP, Key::Select);
    assert_eq!(r.ui.softkeys(APP), ["Stop", "Lap", "Timer", "Exit"]);
    std::thread::sleep(Duration::from_millis(300));
    r.softkey(APP, "Lap");
    std::thread::sleep(Duration::from_millis(300));
    r.softkey(APP, "Lap");
    let laps = r.ui.items(APP);
    assert_eq!(
        laps.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(),
        ["Lap 2", "Lap 1"]
    );
    assert!(laps.iter().all(|l| l.1.starts_with("0:00.")), "{laps:?}");
    // The time on screen counts.
    let before = time(&r).0;
    until(&mut r, "the time on screen changed", |r| {
        time(r).0 != before
    });

    // Off the screen and back, it runs on, as it was.
    r.softkey(APP, "Exit");
    assert!(r.mgr.is_running(APP));
    std::thread::sleep(Duration::from_millis(1000));
    r.launch(APP);
    assert_eq!(r.ui.texts(APP)[0], "Stopwatch");
    assert!(time(&r).0.as_str() >= "0:01.6", "{:?}", time(&r));
    assert_eq!(r.ui.items(APP).len(), 2);

    r.key(APP, Key::Select);
    assert_eq!(r.ui.softkeys(APP), ["Start", "Reset", "Timer", "Exit"]);
    let stopped = time(&r);
    std::thread::sleep(Duration::from_millis(300));
    r.settle();
    assert_eq!(time(&r), stopped);
    r.softkey(APP, "Reset");
    assert_eq!(time(&r), shown("0:00.0", ""));
    assert!(r.ui.items(APP).is_empty());
    r.softkey(APP, "Exit");
    assert!(!r.mgr.is_running(APP));
}

// ---------------------------------------------------------- the touchscreen

fn pad(r: &mut Rig, digits: &str) {
    for d in digits.chars() {
        common::press(r, APP, &d.to_string());
    }
}

#[test]
fn on_the_touchscreen_the_timer_and_stopwatch_are_set_by_touch() {
    let (mut r, audio) = rig(Model::D80);
    r.launch(APP);
    assert_eq!(r.ui.softkeys(APP), [] as [&str; 0]);
    // The two screens are two buttons, the one shown chosen; under them,
    // the duration and a number pad.  (The backspace key has no label.)
    assert_eq!(
        common::labels(&r, APP),
        [
            "Timer",
            "Stopwatch",
            "1",
            "2",
            "3",
            "4",
            "5",
            "6",
            "7",
            "8",
            "9",
            "0",
            "Start"
        ]
    );
    assert_eq!(common::button(&r, APP, "Timer").fill, common::highlight());
    assert_ne!(
        common::button(&r, APP, "Stopwatch").fill,
        common::highlight()
    );
    assert_ne!(
        common::button(&r, APP, "Start").fill,
        common::highlight(),
        "nothing to start"
    );

    // The digits fill the seconds first, as on a microwave oven.
    pad(&mut r, "130");
    assert_eq!(time(&r), shown("00:01:30", ""));
    common::press(&mut r, APP, "Start");
    assert_eq!(
        common::labels(&r, APP),
        ["Timer", "Stopwatch", "Cancel", "Pause"]
    );
    common::press(&mut r, APP, "Pause");
    assert!(r.ui.shows(APP, "Paused"));
    assert_eq!(common::labels(&r, APP)[3], "Resume");
    common::press(&mut r, APP, "Cancel");
    assert_eq!(time(&r).0, "00:01:30");

    // A second's timer, on screen: "Dismiss" alone, and the back button
    // dismisses too.
    pad(&mut r, "1");
    common::press(&mut r, APP, "Start");
    until(&mut r, "the time was up", |r| r.ui.shows(APP, "Time's up"));
    assert_eq!(common::labels(&r, APP), ["Dismiss"]);
    assert_eq!(
        common::recorded(&audio, 1),
        [Command::PlayAlarmTone {
            name: "Beep".into()
        }]
    );
    r.key(APP, Key::Cancel);
    assert!(on_screen(&r));
    assert_eq!(
        common::labels(&r, APP).last().map(String::as_str),
        Some("Start")
    );

    common::press(&mut r, APP, "Stopwatch");
    assert_eq!(r.ui.texts(APP)[0], "Stopwatch");
    assert_eq!(
        common::labels(&r, APP),
        ["Timer", "Stopwatch", "Lap", "Start"]
    );
    assert_ne!(common::button(&r, APP, "Lap").fill, common::highlight());
    common::press(&mut r, APP, "Start");
    assert_eq!(common::labels(&r, APP)[2..], ["Lap", "Stop"]);
    common::press(&mut r, APP, "Lap");
    assert_eq!(r.ui.items(APP).len(), 1);
    common::press(&mut r, APP, "Stop");
    assert_eq!(common::labels(&r, APP)[2..], ["Reset", "Start"]);
    common::press(&mut r, APP, "Reset");
    assert!(r.ui.items(APP).is_empty());
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
}

// -------------------------------------------------------- the phone's screens

/// A simulated phone with the app on screen, drawn by the phone UI.
struct Phone {
    model: Model,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    mgr: AppManager,
    _dir: common::TempDir,
}

impl Phone {
    fn new(model: Model) -> Phone {
        let dir = temp_dir("timer-phone");
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        let ui = Ui::from_hal(&mut hal, store.clone(), UiConfig::default());
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        store.update::<Alarms, _>(|a| a.tones = vec!["Beep".into()]);
        common::answer_commands(&store, Domain::Audio, Arc::default(), |_| Ok(Reply::Done));
        let mut cfg = ManagerConfig::rooted(staged(), &dir.0.join("nvdata"));
        cfg.target = Target::Pulley64;
        let mut mgr = AppManager::new(
            cfg,
            store,
            Box::new(UiShell::new(ui.handle(), model)),
            services(&FakePbx::default(), FakeConfig::default()),
        )
        .expect("engine");
        mgr.scan();
        let mut phone = Phone {
            model,
            ui,
            sim,
            mgr,
            _dir: dir,
        };
        phone.settle();
        phone.mgr.launch(APP).unwrap();
        phone.settle();
        assert_eq!(phone.ui.screen_name(), "app:timer");
        phone
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            self.ui.pump();
            self.mgr.pump();
        }
        self.ui.pump();
    }

    fn tap(&mut self, keys: &[Key]) {
        for key in keys {
            self.sim.tap(*key);
            self.settle();
        }
    }

    fn touch(&mut self, x: u16, y: u16) {
        self.sim.touch_tap(x, y);
        self.settle();
    }

    /// Runs the phone for `ms`.
    fn wait(&mut self, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            self.settle();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.settle();
    }

    /// Writes the frame as a PNG (the gray ones doubled in size), and
    /// returns its pixels.
    fn shot(&self, name: &str) -> Vec<u8> {
        let dir = match std::env::var_os("TIMER_PREVIEWS") {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("timer"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        let f = self.sim.frame(Screen::Main).expect("main frame");
        let (w, h) = (f.info.width as usize, f.info.height as usize);
        let bpp = f.info.format.bytes_per_pixel();
        let scale = if self.model.is_color() { 1 } else { 2 };
        let mut rgb = Vec::with_capacity(w * h * 3 * scale * scale);
        for y in 0..h * scale {
            for x in 0..w * scale {
                let i = (y / scale) * f.info.stride + (x / scale) * bpp;
                if bpp == 4 {
                    rgb.extend_from_slice(&[f.pixels[i + 2], f.pixels[i + 1], f.pixels[i]]);
                } else {
                    rgb.extend_from_slice(&[f.pixels[i]; 3]);
                }
            }
        }
        let path = dir.join(format!("{}-{name}.png", self.model.name()));
        let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut enc = png::Encoder::new(file, (w * scale) as u32, (h * scale) as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        f.pixels.to_vec()
    }
}

#[test]
fn the_screens_on_every_kind_of_display() {
    for model in [Model::D60, Model::D70, Model::D40] {
        let mut p = Phone::new(model);
        let set = p.shot("1-timer");
        p.tap(&[Key::Digit(1), Key::Digit(3), Key::Digit(3)]);
        let typed = p.shot("2-typed");
        assert!(typed != set, "{model:?}");
        p.tap(&[Key::Select, Key::Soft(1)]);
        let paused = p.shot("3-paused");
        assert!(paused != typed, "{model:?}");
        // Cancel, and a second's timer.
        p.tap(&[
            Key::Soft(2),
            Key::Left,
            Key::Digit(0),
            Key::Digit(0),
            Key::Digit(1),
            Key::Select,
        ]);
        p.wait(1200);
        let up = p.shot("4-up");
        assert!(up != paused, "{model:?}");
        p.tap(&[Key::Soft(1), Key::Soft(2), Key::Select]);
        p.wait(300);
        p.tap(&[Key::Soft(2)]);
        p.wait(300);
        p.tap(&[Key::Soft(2), Key::Select]);
        let laps = p.shot("5-stopwatch");
        assert!(laps != up, "{model:?}");
        assert_eq!(p.ui.screen_name(), "app:timer");
    }
}

#[test]
fn the_screens_on_the_touchscreen() {
    let mut p = Phone::new(Model::D80);
    let set = p.shot("1-timer");
    // 1, 3, and 0 on the number pad (four rows of keys 112 high, from
    // 536), and "Start" across the bottom.
    for (x, y) in [(138, 536 + 56), (662, 536 + 56), (400, 920 + 56)] {
        p.touch(x, y);
    }
    let typed = p.shot("2-typed");
    assert!(typed != set);
    p.touch(400, 1048 + 48);
    // "Pause", at the right of the bottom.
    p.touch(600, 1048 + 48);
    let paused = p.shot("3-paused");
    assert!(paused != typed);
    // "Cancel", then a second's timer.
    p.touch(200, 1048 + 48);
    p.touch(138, 536 + 56);
    p.touch(400, 1048 + 48);
    p.wait(1200);
    let up = p.shot("4-up");
    assert!(up != paused);
    // "Dismiss"; "Stopwatch", under the title at the right; "Start", two
    // laps, and "Stop".
    p.touch(400, 1048 + 48);
    p.touch(600, 112 + 48);
    p.touch(600, 1048 + 48);
    p.wait(300);
    p.touch(200, 1048 + 48);
    p.wait(300);
    p.touch(200, 1048 + 48);
    p.touch(600, 1048 + 48);
    let laps = p.shot("5-stopwatch");
    assert!(laps != up);
    assert_eq!(p.ui.screen_name(), "app:timer");
}
