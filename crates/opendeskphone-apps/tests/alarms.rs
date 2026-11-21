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
    assert_eq!(*audio.lock().unwrap(), [Command::PlayAlarmTone { name: "Buzzer".into() }, Command::StopSound]);
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
        for a in [
            alarm(6, 30, WEEKDAYS, "Osmium"),
            alarm(8, 0, 0b0000010, "Beep"),
            Alarm { enabled: false, ..alarm(9, 30, 0b1000001, "Buzzer") },
            alarm(14, 45, 0, "Beep"),
        ] {
            store.send(Command::SetAlarm { alarm: a }).wait().unwrap();
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
        let played = || -> Vec<String> {
            let log = self.audio.lock().unwrap();
            log.iter()
                .filter_map(|c| if let Command::PlayAlarmTone { name } = c { Some(name.clone()) } else { None })
                .collect()
        };
        for _ in 0..200 {
            if played().len() >= n {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        played()
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
    r.ui.tap(APP, 400, TITLE + row * ROW + ROW / 2);
    r.settle();
}

/// Touches row `row` of the list that has the focus.
fn touch_row(r: &mut Rig, row: u32) {
    r.ui.tap_row(APP, row);
    r.settle();
}

#[test]
fn on_the_touchscreen_an_alarm_is_set_by_touch() {
    let (mut r, log) = touch_rig();
    let audio = Arc::new(Mutex::new(Vec::new()));
    common::answer_commands(&r.store, Domain::Audio, audio.clone(), |_| Ok(Reply::Done));
    r.launch(APP);
    // The back button leaves: no "Exit".
    assert_eq!(r.ui.softkeys(APP), ["New"]);
    r.softkey(APP, "New");
    // (The back button is Cancel.)
    assert_eq!(r.ui.softkeys(APP), ["Save"]);

    // The time: touching the field opens the dialog, in which the hours
    // and the minutes are two lists; nothing is typed, and nothing says
    // to type.
    touch_field(&mut r, 0);
    let texts = r.ui.texts(APP);
    assert_eq!(texts[..2], ["New Alarm", "Time"]);
    assert!(!texts.iter().any(|t| t.contains("Type")), "{texts:?}");
    assert_eq!(r.ui.softkeys(APP), ["OK", "AM/PM"]);
    assert_eq!(rows(&r).len(), 12, "the hours of half a day");
    assert_eq!(rows(&r)[0], row("12", ""));
    // What is under the dialog is not touched while it is open.
    touch_field(&mut r, 1);
    assert_eq!(r.ui.texts(APP)[..2], ["New Alarm", "Time"]);
    // Seven o'clock; then the minutes, the list at the right (the dialog
    // is 600 pixels wide in the middle of the window, and its lists are
    // under a head of 88 pixels).
    touch_row(&mut r, 7);
    r.ui.tap(APP, 550, 342 + 90 + ROW / 2);
    r.settle();
    assert_eq!(rows(&r).len(), 60);
    touch_row(&mut r, 30);
    if !r.ui.shows(APP, "7:30 AM") {
        r.softkey(APP, "AM/PM");
    }
    assert!(r.ui.shows(APP, "7:30 AM"), "{:?}", r.ui.texts(APP));
    r.softkey(APP, "AM/PM");
    assert!(r.ui.shows(APP, "7:30 PM"), "{:?}", r.ui.texts(APP));
    r.softkey(APP, "OK");
    assert_eq!(r.ui.texts(APP), ["New Alarm"], "the dialog is gone");
    assert_eq!(rows(&r)[0], row("Time", "7:30 PM"));

    // The days: a touch marks a day, and another takes the mark off.
    touch_field(&mut r, 1);
    assert_eq!(r.ui.texts(APP), ["New Alarm", "Repeat"]);
    touch_row(&mut r, 1);
    touch_row(&mut r, 3);
    touch_row(&mut r, 5);
    touch_row(&mut r, 3);
    r.softkey(APP, "OK");
    assert_eq!(rows(&r)[1], row("Repeat", "Mon Fri"));

    // The tone: a touch sounds one, and the button takes it.
    touch_field(&mut r, 2);
    assert_eq!(r.ui.texts(APP), ["New Alarm", "Tone"]);
    touch_row(&mut r, 1);
    touch_row(&mut r, 2);
    assert_eq!(r.ui.texts(APP), ["New Alarm", "Tone"], "the dialog stays open");
    assert_eq!(r.ui.softkeys(APP), ["OK"]);
    r.softkey(APP, "OK");
    assert_eq!(rows(&r)[2], row("Tone", "Osmium"));
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(
        *audio.lock().unwrap(),
        [
            Command::PlayAlarmTone { name: "Buzzer".into() },
            Command::PlayAlarmTone { name: "Osmium".into() },
            Command::StopSound
        ]
    );

    r.softkey(APP, "Save");
    assert_eq!(*log.lock().unwrap(), [Command::SetAlarm { alarm: alarm(19, 30, 0b0100010, "Osmium") }]);
    assert_eq!(rows(&r), [row("7:30 PM", "Mon Fri")]);
    // The list's softkeys are not for an alarm: none is highlighted.
    assert_eq!(r.ui.softkeys(APP), ["New"]);
}

#[test]
fn on_the_touchscreen_a_touched_alarm_is_opened_and_its_buttons_are_with_it() {
    let (mut r, log) = touch_rig();
    for a in [alarm(8, 0, 0b0000010, "Beep"), alarm(13, 15, 0, "Buzzer")] {
        r.store.send(Command::SetAlarm { alarm: a }).wait().unwrap();
    }
    log.lock().unwrap().clear();
    r.launch(APP);
    assert_eq!(rows(&r), [row("8:00 AM", "Mon"), row("1:15 PM", "Once")]);
    assert_eq!(r.ui.softkeys(APP), ["New"]);
    // Touching the second alarm opens it, with the buttons that switch
    // and delete it.
    touch_field(&mut r, 1);
    assert_eq!(r.ui.texts(APP), ["Edit Alarm"]);
    assert_eq!(rows(&r)[0], row("Time", "1:15 PM"));
    assert_eq!(r.ui.softkeys(APP), ["Save", "Disable", "Delete"]);
    r.softkey(APP, "Disable");
    assert_eq!(r.ui.texts(APP), ["Alarms"]);
    assert_eq!(rows(&r), [row("8:00 AM", "Mon"), row("1:15 PM", "Disabled")]);
    // It is switched on again as its fields then are: with another tone.
    touch_field(&mut r, 1);
    assert_eq!(r.ui.softkeys(APP), ["Save", "Enable", "Delete"]);
    touch_field(&mut r, 2);
    touch_row(&mut r, 2);
    r.softkey(APP, "OK");
    log.lock().unwrap().clear();
    r.softkey(APP, "Enable");
    let set = log.lock().unwrap().clone();
    assert!(
        matches!(&set[..], [Command::SetAlarm { alarm }] if alarm.enabled && alarm.tone == "Osmium" && (alarm.hour, alarm.minute) == (13, 15)),
        "{set:?}"
    );
    assert_eq!(rows(&r), [row("8:00 AM", "Mon"), row("1:15 PM", "Once")]);
    // The time's dialog opens on the alarm's time; the back button (the
    // key `cancel`) closes the dialog, then the alarm, then the app.
    touch_field(&mut r, 1);
    touch_field(&mut r, 0);
    assert!(r.ui.shows(APP, "1:15 PM"));
    assert_eq!(r.ui.softkeys(APP)[0], "OK");
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP), ["Edit Alarm"]);
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP), ["Alarms"]);
    // The first alarm is deleted from its own screen.
    touch_field(&mut r, 0);
    assert_eq!(rows(&r)[0], row("Time", "8:00 AM"));
    r.softkey(APP, "Delete");
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
    // The first alarm, touched, is opened.
    touch(&mut p, 400, 96 + 48);
    let edit = p.shot("2-edit");
    assert!(edit != list);
    let width = p.sim.frame(Screen::Main).unwrap().info.stride;
    // The time: the lists of the hours and the minutes.
    touch(&mut p, 400, 96 + 48);
    let time = p.shot("3-time");
    assert!(time != edit);
    assert!(time[..width * 96] == edit[..width * 96], "the title stays");
    // An hour and a minute, by touch, and a drag through the minutes.
    touch(&mut p, 250, 342 + 90 + 96 + 48);
    p.sim.touch_drag((550, 342 + 90 + 400), (550, 342 + 90 + 100), 4);
    p.settle();
    touch(&mut p, 550, 342 + 90 + 48);
    let picked = p.shot("4-time-picked");
    assert!(picked != time);
    // "OK", the first button.
    touch(&mut p, 12 + 90, 1172 + 48);
    let edited = p.shot("5-edited");
    assert!(edited != edit, "the new time shows");
    assert!(edited[..width * 96] == edit[..width * 96]);
    touch(&mut p, 400, 96 + 96 + 48);
    let days = p.shot("6-repeat");
    assert!(days != edited);
    // The back button closes the dialog.
    touch(&mut p, 48, 48);
    assert!(p.shot("6-closed") != days);
    touch(&mut p, 400, 96 + 2 * 96 + 48);
    let tones = p.shot("7-tone");
    assert!(tones != days && tones != edited);
    touch(&mut p, 48, 48);
    touch(&mut p, 48, 48);
    assert!(p.shot("8-list")[..width * 96] == list[..width * 96]);
    touch(&mut p, 48, 48);
    assert_ne!(p.ui.screen_name(), "app:alarms");
}
