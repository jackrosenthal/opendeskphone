//! The Alarms app: the screens for the phone's alarm clock, under the real
//! runtime.  The alarm clock is the main role's (its tests are there, and
//! the ringing alarm's are in `opendeskphone-ui`); here a stand-in answers the
//! `Alarms` commands and publishes the state, which is all the app sees
//! of it.
//!
//! The last test draws the app's screens with the phone UI on a simulated
//! D60 (color), D70, and D40 (gray), and writes them as PNG files under
//! `alarms/` in cargo's test scratch directory (`CARGO_TARGET_TMPDIR`), or
//! in the directory `ALARMS_PREVIEWS` names.

mod common;

use common::{Rig, staged, temp_dir};
use opendeskphone_apps::engine::Target;
use opendeskphone_apps::testutil::{FakeConfig, FakePbx, services};
use opendeskphone_apps::uishell::UiShell;
use opendeskphone_apps::{AppManager, ManagerConfig};
use opendeskphone_hal::{Hal, Screen};
use opendeskphone_state::{Alarm, Alarms, Command, CommandError, Domain, Key, Model, Phase, Reply, Store, System};
use opendeskphone_ui::{Ui, UiConfig};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const APP: &str = "alarms";
const WEEKDAYS: u8 = 0b0111110;

/// Stands in for the phone's alarm clock: keeps the alarms in the state
/// and answers the commands, as the main role's does.  Nothing rings.
fn alarm_clock(store: &Store) -> Arc<Mutex<Vec<Command>>> {
    store.update::<Alarms, _>(|a| {
        a.tones = ["Beep", "Buzzer", "Osmium"].map(String::from).to_vec();
        a.snooze_secs = 600;
    });
    let log = Arc::new(Mutex::new(Vec::new()));
    let (rx, store, seen) = (store.handle(Domain::Alarms), store.clone(), log.clone());
    std::thread::spawn(move || {
        let mut last = 0;
        while let Some(env) = rx.recv() {
            let (cmd, responder) = env.split();
            let mut reply = Ok(Reply::Done);
            store.update::<Alarms, _>(|a| match &cmd {
                Command::SetAlarm { alarm } if alarm.id == 0 && a.alarms.len() >= Alarms::MAX => {
                    reply = Err(CommandError::Invalid(format!("the phone keeps at most {} alarms", Alarms::MAX)));
                }
                Command::SetAlarm { alarm } if alarm.id == 0 => {
                    last += 1;
                    a.alarms.push(Alarm { id: last, ..alarm.clone() });
                    reply = Ok(Reply::Id(last.into()));
                }
                Command::SetAlarm { alarm } => match a.alarms.iter_mut().find(|x| x.id == alarm.id) {
                    Some(x) => *x = alarm.clone(),
                    None => reply = Err(CommandError::Invalid("no such alarm".into())),
                },
                Command::RemoveAlarm { id } => a.alarms.retain(|x| x.id != *id),
                Command::DismissAlarm => a.snoozed = None,
                _ => {}
            });
            seen.lock().unwrap().push(cmd);
            responder.reply(reply);
        }
    });
    log
}

fn rig() -> (Rig, Arc<Mutex<Vec<Command>>>) {
    let r = Rig::new(FakeConfig::default());
    let log = alarm_clock(&r.store);
    (r, log)
}

fn alarm(hour: u8, minute: u8, days: u8, tone: &str) -> Alarm {
    Alarm { id: 0, hour, minute, days, tone: tone.into(), enabled: true }
}

fn rows(r: &Rig) -> Vec<(String, String)> {
    r.ui.items(APP)
}

fn row(text: &str, detail: &str) -> (String, String) {
    (text.to_string(), detail.to_string())
}

fn type_digits(r: &mut Rig, digits: &str) {
    for d in digits.bytes() {
        r.key(APP, Key::Digit(d - b'0'));
    }
}

#[test]
fn an_alarm_is_added_with_a_time_days_and_a_tone() {
    let (mut r, log) = rig();
    r.launch(APP);
    // No alarms: an empty list.
    assert_eq!(rows(&r), []);
    assert_eq!(r.ui.softkeys(APP), ["New", "Exit"]);

    r.softkey(APP, "New");
    assert_eq!(r.ui.texts(APP), ["New Alarm"]);
    let fields = rows(&r);
    assert_eq!(fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["Time", "Repeat", "Tone"]);
    assert_eq!(fields[1..], [row("Repeat", "Once"), row("Tone", "Beep")]);
    assert_eq!(r.ui.softkeys(APP), ["Save", "Cancel"]);

    // The time: Select opens a dialog over the fields, in which the time
    // is typed as hour and minutes, with AM or PM.
    r.key(APP, Key::Select);
    assert_eq!(r.ui.texts(APP)[..2], ["New Alarm", "Time"]);
    assert_eq!(rows(&r).len(), 3, "the fields are still there");
    assert_eq!(r.ui.softkeys(APP), ["OK", "AM/PM", "Delete", "Cancel"]);
    type_digits(&mut r, "063");
    assert!(r.ui.texts(APP).iter().any(|t| t.starts_with("06:3_")), "{:?}", r.ui.texts(APP));
    // Not a time yet: OK does nothing.
    r.softkey(APP, "OK");
    assert_eq!(r.ui.softkeys(APP)[0], "OK");
    type_digits(&mut r, "5");
    r.softkey(APP, "Delete");
    type_digits(&mut r, "0");
    if !r.ui.shows(APP, "06:30 AM") {
        r.softkey(APP, "AM/PM");
    }
    assert!(r.ui.shows(APP, "06:30 AM"), "{:?}", r.ui.texts(APP));
    r.softkey(APP, "OK");
    assert_eq!(r.ui.texts(APP), ["New Alarm"], "the dialog is gone");
    assert_eq!(rows(&r)[0], row("Time", "6:30 AM"));
    assert_eq!(r.ui.softkeys(APP), ["Save", "Cancel"]);

    // The days: Select marks a day; Cancel leaves the alarm's days alone,
    // and OK takes the marked ones.
    r.key(APP, Key::Down);
    r.key(APP, Key::Select);
    assert_eq!(r.ui.texts(APP), ["New Alarm", "Repeat"]);
    assert_eq!(rows(&r).len(), 7);
    assert_eq!(r.ui.softkeys(APP), ["OK", "Cancel"]);
    r.key(APP, Key::Select);
    r.softkey(APP, "Cancel");
    assert_eq!(rows(&r)[1], row("Repeat", "Once"));
    r.key(APP, Key::Select);
    for _ in 0..5 {
        r.key(APP, Key::Down);
        r.key(APP, Key::Select);
    }
    r.softkey(APP, "OK");
    assert_eq!(rows(&r)[1], row("Repeat", "Weekdays"));

    // The tone: Play sounds the highlighted one, Select takes it.
    r.key(APP, Key::Down);
    r.key(APP, Key::Select);
    assert_eq!(r.ui.texts(APP), ["New Alarm", "Tone"]);
    assert_eq!(rows(&r), [row("Beep", ""), row("Buzzer", ""), row("Osmium", "")]);
    assert_eq!(r.ui.softkeys(APP), ["Select", "Play", "Cancel"]);
    r.key(APP, Key::Down);
    r.key(APP, Key::Down);
    r.softkey(APP, "Select");
    assert_eq!(rows(&r)[2], row("Tone", "Osmium"));

    r.softkey(APP, "Save");
    assert_eq!(*log.lock().unwrap(), [Command::SetAlarm { alarm: alarm(6, 30, WEEKDAYS, "Osmium") }]);
    assert_eq!(rows(&r), [row("6:30 AM", "Weekdays")]);
    assert_eq!(r.ui.softkeys(APP), ["New", "Edit", "Disable", "Delete", "Exit"]);
}

#[test]
fn the_tone_dialog_plays_a_tone_and_stops_it() {
    let (mut r, _log) = rig();
    let audio = Arc::new(Mutex::new(Vec::new()));
    common::answer_commands(&r.store, Domain::Audio, audio.clone(), |_| Ok(Reply::Done));
    r.launch(APP);
    r.softkey(APP, "New");
    r.key(APP, Key::Down);
    r.key(APP, Key::Down);
    r.key(APP, Key::Select);
    r.key(APP, Key::Down);
    r.softkey(APP, "Play");
    r.softkey(APP, "Cancel");
    // Both are posted, not waited for.
    assert_eq!(common::recorded(&audio, 2), [Command::PlayAlarmTone { name: "Buzzer".into() }, Command::StopSound]);
    // Cancel did not take the tone.
    assert_eq!(rows(&r)[2], row("Tone", "Beep"));
}

#[test]
fn alarms_are_changed_switched_and_deleted() {
    let (mut r, log) = rig();
    for a in [alarm(8, 0, 0b0000010, "Beep"), alarm(13, 15, 0, "Buzzer")] {
        r.store.send(Command::SetAlarm { alarm: a }).wait().unwrap();
    }
    log.lock().unwrap().clear();
    r.launch(APP);
    assert_eq!(rows(&r), [row("8:00 AM", "Mon"), row("1:15 PM", "Once")]);

    // Disabled, and enabled again.
    r.softkey(APP, "Disable");
    assert_eq!(rows(&r)[0], row("8:00 AM", "Disabled"));
    assert_eq!(r.ui.softkeys(APP)[2], "Enable");
    r.softkey(APP, "Enable");
    assert_eq!(rows(&r)[0], row("8:00 AM", "Mon"));

    // Select edits the highlighted alarm; Cancel leaves it as it was.
    r.key(APP, Key::Down);
    assert_eq!(r.ui.softkeys(APP), ["New", "Edit", "Disable", "Delete", "Exit"]);
    r.key(APP, Key::Select);
    assert_eq!(r.ui.texts(APP), ["Edit Alarm"]);
    assert_eq!(rows(&r), [row("Time", "1:15 PM"), row("Repeat", "Once"), row("Tone", "Buzzer")]);
    r.key(APP, Key::Select);
    assert!(r.ui.shows(APP, "01:15 PM"));
    // Cancel closes the dialog, not the alarm's fields.
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP), ["Edit Alarm"]);
    r.key(APP, Key::Select);
    type_digits(&mut r, "2145");
    r.key(APP, Key::Select);
    assert_eq!(rows(&r)[0], row("Time", "9:45 PM"));
    r.softkey(APP, "Cancel");
    assert_eq!(rows(&r)[1], row("1:15 PM", "Once"));
    // Changed and saved: an alarm that is edited stays enabled or
    // disabled as it was.
    r.softkey(APP, "Disable");
    r.softkey(APP, "Edit");
    r.key(APP, Key::Select);
    type_digits(&mut r, "1320");
    r.softkey(APP, "OK");
    r.softkey(APP, "Save");
    assert_eq!(rows(&r)[1], row("1:20 PM", "Disabled"));

    // Deleted.
    r.softkey(APP, "Delete");
    assert_eq!(rows(&r), [row("8:00 AM", "Mon")]);
    assert_eq!(r.ui.softkeys(APP), ["New", "Edit", "Disable", "Delete", "Exit"]);

    let sent = log.lock().unwrap().clone();
    let on = Alarm { id: 1, ..alarm(8, 0, 0b0000010, "Beep") };
    assert_eq!(
        sent,
        [
            Command::SetAlarm { alarm: Alarm { enabled: false, ..on.clone() } },
            Command::SetAlarm { alarm: on },
            Command::SetAlarm { alarm: Alarm { id: 2, enabled: false, ..alarm(13, 15, 0, "Buzzer") } },
            Command::SetAlarm { alarm: Alarm { id: 2, enabled: false, ..alarm(13, 20, 0, "Buzzer") } },
            Command::RemoveAlarm { id: 2 },
        ]
    );
    // Cancel on the list leaves the app.
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.top(), None);
}

#[test]
fn the_list_follows_changes_made_elsewhere() {
    let (mut r, _log) = rig();
    r.launch(APP);
    assert_eq!(rows(&r), []);
    // From the web interface.
    r.store.send(Command::SetAlarm { alarm: alarm(7, 5, 0x7f, "Osmium") }).wait().unwrap();
    r.settle();
    assert_eq!(rows(&r), [row("7:05 AM", "Every day")]);
    // It rang and was snoozed: the list says so, and can end the snooze.
    r.store.update::<Alarms, _>(|a| a.snoozed = Some((1, 1_800_000_000)));
    r.settle();
    assert_eq!(rows(&r)[0], row("7:05 AM", "Snoozed"));
    assert_eq!(r.ui.softkeys(APP), ["New", "Edit", "Disable", "Dismiss", "Delete", "Exit"]);
    r.softkey(APP, "Dismiss");
    assert_eq!(rows(&r)[0], row("7:05 AM", "Every day"));
    // An alarm that is being edited is not swept away by a change.
    r.softkey(APP, "Edit");
    r.store.send(Command::SetAlarm { alarm: alarm(9, 0, 0, "Osmium") }).wait().unwrap();
    r.settle();
    assert_eq!(r.ui.texts(APP), ["Edit Alarm"]);
    r.softkey(APP, "Cancel");
    assert_eq!(rows(&r).len(), 2);
}

#[test]
fn a_refusal_of_the_alarm_clock_is_shown() {
    let (mut r, _log) = rig();
    for _ in 0..Alarms::MAX {
        r.store.send(Command::SetAlarm { alarm: alarm(8, 0, 0, "Beep") }).wait().unwrap();
    }
    r.launch(APP);
    r.softkey(APP, "New");
    r.softkey(APP, "Save");
    assert!(r.ui.shows(APP, "The phone keeps at most 16 alarms."), "{:?}", r.ui.texts(APP));
    r.softkey(APP, "OK");
    assert_eq!(rows(&r).len(), Alarms::MAX);
}

/// The phone's alarm tones, in its order.
const TONES: [&str; 39] = [
    "Beep-Beep-Beep", "Argon", "Argon JP", "Awaken", "Barium", "BeeBeep", "Bounce", "Buzzer", "Carbon", "Carbon JP",
    "Cesium", "Copernicium", "Curium", "Drip", "Fermium", "Gallop", "Hassium", "Helium", "Helium JP", "Krypton",
    "Neon", "Neptunium", "Nobelium", "Nudge", "Orbit", "Osmium", "Osmium Classic", "Oxygen", "Piezo", "Platinum",
    "Plutonium", "Promethium", "Rise", "Rooster 1", "Rooster 2", "Scandium", "Sway", "Timer", "Wag",
];

/// A simulated phone with the app on screen, drawn by the phone UI.
struct Phone {
    model: Model,
    ui: Ui,
    /// The commands to the audio side.
    audio: Arc<Mutex<Vec<Command>>>,
    sim: opendeskphone_hal::sim::SimHandle,
    mgr: AppManager,
    _dir: common::TempDir,
}

impl Phone {
    fn new(model: Model) -> Phone {
        Phone::with_alarms(
            model,
            &[
                alarm(6, 30, WEEKDAYS, "Osmium"),
                alarm(8, 0, 0b0000010, "Beep"),
                Alarm { enabled: false, ..alarm(9, 30, 0b1000001, "Buzzer") },
                alarm(14, 45, 0, "Beep"),
            ],
        )
    }

    fn with_alarms(model: Model, alarms: &[Alarm]) -> Phone {
        let dir = temp_dir("alarms-phone");
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        let ui = Ui::from_hal(&mut hal, store.clone(), UiConfig::default());
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        alarm_clock(&store);
        // The tones the phone has (`alarmtones::ALL` in the main role).
        store.update::<Alarms, _>(|a| a.tones = TONES.map(String::from).to_vec());
        let audio = Arc::new(Mutex::new(Vec::new()));
        common::answer_commands(&store, Domain::Audio, audio.clone(), |_| Ok(Reply::Done));
        for a in alarms {
            store.send(Command::SetAlarm { alarm: a.clone() }).wait().unwrap();
        }
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
        let mut phone = Phone { model, ui, audio, sim, mgr, _dir: dir };
        phone.settle();
        phone.mgr.launch(APP).unwrap();
        phone.settle();
        assert_eq!(phone.ui.screen_name(), "app:alarms");
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

    /// The tones played so far, once `n` have been.
    fn previews(&self, n: usize) -> Vec<String> {
        common::recorded_of(&self.audio, n, |c| matches!(c, Command::PlayAlarmTone { .. }))
            .into_iter()
            .filter_map(|c| if let Command::PlayAlarmTone { name } = c { Some(name) } else { None })
            .collect()
    }

    /// Writes the frame as a PNG (the gray ones doubled in size), and
    /// returns its pixels.
    fn shot(&self, name: &str) -> Vec<u8> {
        let dir = match std::env::var_os("ALARMS_PREVIEWS") {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("alarms"),
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
        let list = p.shot("1-list");
        // Edit the first alarm.
        p.tap(&[Key::Select]);
        let edit = p.shot("2-edit");
        assert!(edit != list, "{model:?}");
        // Each dialog is drawn over the fields: the top of the screen,
        // with the title, stays as it was.
        let width = p.sim.frame(Screen::Main).unwrap().info.stride;
        p.tap(&[Key::Select]);
        let time = p.shot("3-time");
        assert!(time != edit, "{model:?}");
        assert_eq!(time[..width * 12], edit[..width * 12], "{model:?}");
        p.tap(&[Key::Digit(0), Key::Digit(7), Key::Digit(1)]);
        let typed = p.shot("4-time-typed");
        assert!(typed != time, "{model:?}");
        p.tap(&[Key::Digit(5), Key::Select]);
        let edited = p.shot("5-edited");
        assert!(edited != edit, "{model:?}: the new time shows");
        p.tap(&[Key::Down, Key::Select]);
        let days = p.shot("6-repeat");
        assert!(days != edited, "{model:?}");
        assert_eq!(days[..width * 12], edit[..width * 12], "{model:?}");
        p.tap(&[Key::Cancel, Key::Down, Key::Select]);
        // The tones, a list longer than the dialog: it opens on the
        // alarm's, far down the list and marked, and scrolls both ways.
        // Play (the second softkey) sounds the highlighted tone.
        let tones = p.shot("7-tone");
        assert!(tones != days, "{model:?}");
        assert_eq!(tones[..width * 12], edit[..width * 12], "{model:?}");
        p.tap(&[Key::Soft(2)]);
        p.tap(&[Key::Down; 13]);
        let last = p.shot("7-tone-last");
        p.tap(&[Key::Soft(2)]);
        p.tap(&[Key::Up; 38]);
        let first = p.shot("7-tone-first");
        p.tap(&[Key::Soft(2)]);
        assert!(last != tones && first != tones && first != last, "{model:?}");
        assert_eq!(p.previews(3), ["Osmium", "Wag", "Beep-Beep-Beep"], "{model:?}");
        p.tap(&[Key::Cancel]);
        assert_eq!(p.shot("8-closed")[..width * 12], edit[..width * 12], "{model:?}");
        assert_eq!(p.ui.screen_name(), "app:alarms");
    }
}

// ---------------------------------------------------------- the touchscreen

/// The app on a D80, which has no keys.
fn touch_rig() -> (Rig, Arc<Mutex<Vec<Command>>>) {
    let r = Rig::with_root(staged(), FakeConfig::default(), Model::D80);
    let log = alarm_clock(&r.store);
    (r, log)
}

/// The D80's window: the title's height, and a row's.
const TITLE: i32 = 96;
const ROW: i32 = 96;

/// Touches row `row` of the list under the title.
fn touch_field(r: &mut Rig, row: i32) {
    r.ui.tap(APP, 300, TITLE + row * ROW + ROW / 2);
    r.settle();
}

/// Touches row `row` of the list that has the focus.
fn touch_row(r: &mut Rig, row: u32) {
    r.ui.tap_row(APP, row);
    r.settle();
}

/// Touches the switch of the alarm of row `row` of the list.
fn touch_switch(r: &mut Rig, row: i32) {
    r.ui.tap(APP, 720, TITLE + row * ROW + ROW / 2);
    r.settle();
}

/// Touches the `n`-th of the buttons labeled `label` (the days have two
/// of "S" and two of "T").
fn press_nth(r: &mut Rig, label: &str, n: usize) {
    let at = common::buttons(r, APP).into_iter().filter(|b| b.label == label).nth(n).expect(label).at;
    r.ui.tap(APP, at.x as i32 + at.width as i32 / 2, at.y as i32 + at.height as i32 / 2);
    r.settle();
}

/// Types a time on the number pad.
fn pad(r: &mut Rig, digits: &str) {
    for d in digits.chars() {
        common::press(r, APP, &d.to_string());
    }
}

#[test]
fn on_the_touchscreen_an_alarm_is_set_by_touch() {
    let (mut r, log) = touch_rig();
    let audio = Arc::new(Mutex::new(Vec::new()));
    common::answer_commands(&r.store, Domain::Audio, audio.clone(), |_| Ok(Reply::Done));
    r.launch(APP);
    // No alarms: a sentence that says so, and the button that sets one,
    // across the bottom.  The back button leaves: there are no softkeys.
    assert!(r.ui.shows(APP, "There are no alarms."), "{:?}", r.ui.texts(APP));
    assert_eq!(common::labels(&r, APP), ["+ New alarm"]);
    assert_eq!(common::button(&r, APP, "+ New alarm").fill, common::highlight());
    assert_eq!(r.ui.softkeys(APP), [] as [&str; 0]);
    common::press(&mut r, APP, "+ New alarm");
    assert_eq!(r.ui.texts(APP)[0], "New Alarm");
    let fields = rows(&r);
    assert_eq!(fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["Time", "Tone"]);
    assert_eq!(fields[1], row("Tone", "Beep"));
    // The days are buttons, all off: the alarm rings once.  A new alarm
    // has nothing to delete.
    assert_eq!(common::labels(&r, APP), ["S", "M", "T", "W", "T", "F", "S", "Save"]);
    assert!(r.ui.shows(APP, "Once"));

    // The time: touching its row opens a number pad, on which it is typed
    // as hour and minutes, with AM and PM beside it.
    touch_field(&mut r, 0);
    assert_eq!(r.ui.texts(APP)[0], "Time");
    assert_eq!(
        common::labels(&r, APP),
        ["Set time", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "AM", "PM"]
    );
    pad(&mut r, "073");
    assert!(r.ui.shows(APP, "07:3_"), "{:?}", r.ui.texts(APP));
    // Not a time yet: "Set time" is dim, and does nothing.
    assert_ne!(common::button(&r, APP, "Set time").fill, common::highlight());
    common::press(&mut r, APP, "Set time");
    assert_eq!(r.ui.texts(APP)[0], "Time");
    pad(&mut r, "0");
    assert!(r.ui.shows(APP, "07:30"), "{:?}", r.ui.texts(APP));
    common::press(&mut r, APP, "PM");
    assert_eq!(common::button(&r, APP, "PM").fill, common::highlight());
    assert_ne!(common::button(&r, APP, "AM").fill, common::highlight());
    common::press(&mut r, APP, "Set time");
    assert_eq!(r.ui.texts(APP)[0], "New Alarm");
    assert_eq!(rows(&r)[0], row("Time", "7:30 PM"));

    // The days: a touch puts a day on, and another takes it off.
    common::press(&mut r, APP, "M");
    common::press(&mut r, APP, "W");
    common::press(&mut r, APP, "F");
    common::press(&mut r, APP, "W");
    assert!(r.ui.shows(APP, "Mon Fri"), "{:?}", r.ui.texts(APP));
    assert_eq!(common::button(&r, APP, "M").fill, common::highlight());
    assert_ne!(common::button(&r, APP, "W").fill, common::highlight());
    press_nth(&mut r, "S", 1);
    assert!(r.ui.shows(APP, "Mon Fri Sat"), "{:?}", r.ui.texts(APP));
    press_nth(&mut r, "S", 1);

    // The tone: a touched tone is heard and is the alarm's; the back
    // button keeps it.
    touch_field(&mut r, 1);
    assert_eq!(r.ui.texts(APP), ["Tone"]);
    touch_row(&mut r, 1);
    touch_row(&mut r, 2);
    assert_eq!(r.ui.texts(APP), ["Tone"], "the list stays");
    r.key(APP, Key::Cancel);
    assert_eq!(rows(&r)[1], row("Tone", "Osmium"));
    assert_eq!(
        common::recorded(&audio, 3),
        [
            Command::PlayAlarmTone { name: "Buzzer".into() },
            Command::PlayAlarmTone { name: "Osmium".into() },
            Command::StopSound
        ]
    );

    common::press(&mut r, APP, "Save");
    assert_eq!(*log.lock().unwrap(), [Command::SetAlarm { alarm: alarm(19, 30, 0b0100010, "Osmium") }]);
    assert_eq!(rows(&r), [row("7:30 PM", "Mon Fri")]);
    assert_eq!(common::labels(&r, APP), ["+ New alarm"]);
    assert_eq!(r.ui.softkeys(APP), [] as [&str; 0]);
}

#[test]
fn on_the_touchscreen_an_alarm_is_switched_on_its_row_and_deleted_on_its_screen() {
    let (mut r, log) = touch_rig();
    for a in [alarm(8, 0, 0b0000010, "Beep"), alarm(13, 15, 0, "Buzzer")] {
        r.store.send(Command::SetAlarm { alarm: a }).wait().unwrap();
    }
    log.lock().unwrap().clear();
    r.launch(APP);
    assert_eq!(rows(&r), [row("8:00 AM", "Mon"), row("1:15 PM", "Once")]);
    // The switch at the end of the second row turns it off, and on.
    touch_switch(&mut r, 1);
    let set = common::recorded(&log, 1);
    assert!(matches!(&set[..], [Command::SetAlarm { alarm }] if !alarm.enabled && alarm.hour == 13), "{set:?}");
    assert_eq!(rows(&r), [row("8:00 AM", "Mon"), row("1:15 PM", "Once")], "the row stays as it was");
    touch_switch(&mut r, 1);
    let set = common::recorded(&log, 2);
    assert!(matches!(&set[1], Command::SetAlarm { alarm } if alarm.enabled && alarm.hour == 13), "{set:?}");
    assert_eq!(r.ui.texts(APP)[0], "Alarms", "the switch opens nothing");

    // Touching the row opens the alarm, with "Delete alarm" in red, apart
    // from "Save" at the bottom.
    touch_field(&mut r, 1);
    assert_eq!(r.ui.texts(APP)[0], "Edit Alarm");
    assert_eq!(rows(&r)[0], row("Time", "1:15 PM"));
    let delete = common::button(&r, APP, "Delete alarm");
    let save = common::button(&r, APP, "Save");
    assert_eq!((delete.fill, save.fill), (common::red(), common::highlight()));
    assert!(save.at.y - (delete.at.y + delete.at.height as i16) > 200, "{delete:?} {save:?}");
    // The time's screen opens on the alarm's time; the back button
    // leaves it without taking what was typed, then the alarm without
    // saving it.
    touch_field(&mut r, 0);
    assert!(r.ui.shows(APP, "01:15"), "{:?}", r.ui.texts(APP));
    assert_eq!(common::button(&r, APP, "PM").fill, common::highlight());
    pad(&mut r, "0900");
    r.key(APP, Key::Cancel);
    assert_eq!(rows(&r)[0], row("Time", "1:15 PM"));
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP)[0], "Alarms");
    // The first alarm is deleted from its own screen.
    touch_field(&mut r, 0);
    assert_eq!(rows(&r)[0], row("Time", "8:00 AM"));
    common::press(&mut r, APP, "Delete alarm");
    assert_eq!(rows(&r), [row("1:15 PM", "Once")]);
    r.key(APP, Key::Cancel);
    assert_ne!(r.ui.top().as_deref(), Some(APP));
}

#[test]
fn the_screens_on_the_touchscreen() {
    let mut p = Phone::new(Model::D80);
    let touch = |p: &mut Phone, x: u16, y: u16| {
        p.sim.touch_tap(x, y);
        p.settle();
    };
    let list = p.shot("1-list");
    // The third alarm's switch, which is off, turned on.
    touch(&mut p, 720, 96 + 2 * 96 + 48);
    let switched = p.shot("1-list-switched");
    assert!(switched != list);
    // The first alarm, touched, is opened.
    touch(&mut p, 300, 96 + 48);
    let edit = p.shot("2-edit");
    assert!(edit != list);
    let width = p.sim.frame(Screen::Main).unwrap().info.stride;
    // The time: a number pad (four rows of keys 112 high, from 536), and
    // AM and PM over it.
    touch(&mut p, 300, 96 + 48);
    let time = p.shot("3-time");
    assert!(time != edit);
    // 0, 7, 4, and 5.
    for (x, y) in [(400, 920 + 56), (138, 792 + 56), (138, 664 + 56), (400, 664 + 56)] {
        touch(&mut p, x, y);
    }
    // "PM".
    touch(&mut p, 488, 320 + 48);
    let typed = p.shot("4-time-typed");
    assert!(typed != time);
    // "Set time", across the bottom.
    touch(&mut p, 400, 1048 + 48);
    let edited = p.shot("5-edited");
    assert!(edited != edit, "the new time shows");
    assert!(edited[..width * 96] == edit[..width * 96], "the title is as it was");
    // Saturday on, and Monday off.
    touch(&mut p, 688 + 48, 356 + 48);
    touch(&mut p, 128 + 48, 356 + 48);
    let days = p.shot("6-repeat");
    assert!(days != edited);
    // The tones, opened on the alarm's.
    touch(&mut p, 300, 96 + 96 + 48);
    let tones = p.shot("7-tone");
    assert!(tones != days);
    touch(&mut p, 48, 48);
    // "Save": the list, with the alarm as it now is.
    touch(&mut p, 400, 1048 + 48);
    assert!(p.shot("8-list") != switched);
    touch(&mut p, 48, 48);
    assert_ne!(p.ui.screen_name(), "app:alarms");
}

#[test]
fn the_empty_list_on_the_touchscreen() {
    let p = Phone::with_alarms(Model::D80, &[]);
    p.shot("0-empty");
    let c = p.ui.canvas();
    // The button across the bottom, and the sentence a quarter of the way
    // down the space above the button.
    let ground = c.pixel(400, 700);
    assert_ne!(c.pixel(400, 1048 + 10), ground, "the button");
    assert!((0..800).any(|x| c.pixel(x, 96 + 952 / 4) != ground), "the sentence");
}
