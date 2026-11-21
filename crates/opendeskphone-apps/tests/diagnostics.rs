//! The Diagnostics app's hardware tests, drawn by the phone UI on a
//! simulated D60 (color), D40, and D70 (gray), with the simulator's keys,
//! hookswitch, and lamps.
//!
//! The pictures are written as PNG files under `diagnostics/` in cargo's
//! test scratch directory (`CARGO_TARGET_TMPDIR`), or in the directory
//! `DIAGNOSTICS_PREVIEWS` names.  The menu, the status pages, and the tone
//! are tested in `apps.rs`.

mod common;

use common::{staged, temp_dir};
use opendeskphone_apps::engine::Target;
use opendeskphone_apps::testutil::{FakeConfig, FakePbx, services};
use opendeskphone_apps::uishell::UiShell;
use opendeskphone_apps::{AppManager, ManagerConfig};
use opendeskphone_hal::{Hal, Lamp, Screen};
use opendeskphone_state::{Domain, Key, Keys, LampColor, LampState, Model, Phase, Store, System};
use opendeskphone_ui::{Ui, UiConfig};
use std::path::PathBuf;

const APP: &str = "diagnostics";
const WHITE: u32 = 0xFF_FFFF;
const BLACK: u32 = 0;

/// A simulated phone with the app on screen.
struct Phone {
    model: Model,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    mgr: AppManager,
    audio: opendeskphone_state::CommandReceiver,
    calls: opendeskphone_state::CommandReceiver,
    _dir: common::TempDir,
}

impl Phone {
    fn new(model: Model) -> Phone {
        let dir = temp_dir("diagnostics-phone");
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        let ui = Ui::from_hal(&mut hal, store.clone(), UiConfig::default());
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        // The phone's own lamp, to tell it from the test's.
        store.update::<Keys, _>(|k| k.message_lamp = LampState::solid(LampColor::Red));
        let (audio, calls) = (store.handle(Domain::Audio), store.handle(Domain::Calls));
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
            audio,
            calls,
            _dir: dir,
        };
        phone.settle();
        phone.mgr.launch(APP).unwrap();
        phone.settle();
        assert_eq!(phone.ui.screen_name(), "app:diagnostics");
        phone
    }

    /// The UI and the manager take turns until both are done.
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

    fn hook(&mut self, off_hook: bool) {
        self.sim.set_hook(off_hook);
        self.settle();
    }

    /// Waits for the app's next timer.
    fn timer(&mut self) {
        let wait = self.mgr.pump().expect("a timer is pending");
        std::thread::sleep(wait + std::time::Duration::from_millis(20));
        self.settle();
    }

    /// How many commands the phone's audio and call sides were sent.
    fn commands(&self) -> usize {
        let mut n = 0;
        for rx in [&self.audio, &self.calls] {
            while let Some(env) = rx.try_recv() {
                env.done();
                n += 1;
            }
        }
        n
    }

    /// The pixels of the frame as 0xRRGGBB, and the frame's width.
    fn pixels(&self) -> (Vec<u32>, usize) {
        let f = self.sim.frame(Screen::Main).expect("main frame");
        let bpp = f.info.format.bytes_per_pixel();
        let mut out = Vec::new();
        for y in 0..f.info.height as usize {
            for x in 0..f.info.width as usize {
                let i = y * f.info.stride + x * bpp;
                out.push(if bpp == 4 {
                    u32::from_le_bytes([f.pixels[i], f.pixels[i + 1], f.pixels[i + 2], 0])
                } else {
                    u32::from(f.pixels[i]) * 0x01_0101
                });
            }
        }
        (out, f.info.width as usize)
    }

    /// The colors on the display, each once, darkest first.
    fn colors(&self) -> Vec<u32> {
        let mut colors = self.pixels().0;
        colors.sort_unstable();
        colors.dedup();
        colors
    }

    /// The color every lamp shows that the phone has: the lamp table of
    /// a D60 lists line lamps that are not fitted, which no app is told
    /// of and none sets.
    fn lamps(&self) -> LampColor {
        let all = self.sim.lamps();
        assert_eq!(all.len(), Lamp::for_model(self.model).len());
        let fitted = opendeskphone_apps::Lamp::for_model(self.model).len();
        let unfitted = all.len() - fitted;
        let lamps: Vec<_> =
            all.iter().filter(|(l, _)| !matches!(l, Lamp::Line(i) if *i >= self.model.line_keys())).collect();
        assert_eq!((lamps.len(), unfitted), (fitted, (self.model.line_lamps() - self.model.line_keys()) as usize));
        assert!(lamps.iter().all(|(_, c)| *c == lamps[0].1), "{lamps:?}");
        lamps[0].1
    }

    /// Writes the frame as a PNG (the gray ones doubled in size).
    fn shot(&self, name: &str) {
        let dir = match std::env::var_os("DIAGNOSTICS_PREVIEWS") {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("diagnostics"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        let (pixels, w) = self.pixels();
        let h = pixels.len() / w;
        let scale = if self.model.is_color() { 1 } else { 2 };
        let mut rgb = Vec::with_capacity(w * h * 3 * scale * scale);
        for y in 0..h * scale {
            for x in 0..w * scale {
                let p = pixels[(y / scale) * w + x / scale];
                rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
        let path = dir.join(format!("{}-{name}.png", self.model.name()));
        let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut enc = png::Encoder::new(file, (w * scale) as u32, (h * scale) as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    }
}

fn display_test(model: Model) {
    let mut p = Phone::new(model);
    let size = model.main_screen();
    let (w, h) = (size.width as usize, size.height as usize);
    p.shot("menu");
    p.tap(&[Key::Digit(1)]);
    // The whole display, the softkey bar's part too.
    p.shot("display-1-white");
    assert_eq!(p.colors(), [WHITE]);
    // Any key shows the next pattern: the phone's own keys too.
    p.tap(&[Key::VolumeUp]);
    p.shot("display-2-black");
    assert_eq!(p.colors(), [BLACK]);
    p.tap(&[Key::Line(1)]);
    p.shot("display-3-bars");
    let (px, _) = p.pixels();
    let bars: Vec<u32> = if model.is_color() {
        vec![0xFF_0000, 0x00_FF00, 0x00_00FF, 0xFF_FF00, 0x00_FFFF, 0xFF_00FF, WHITE, BLACK]
    } else {
        vec![BLACK, 0x55_5555, 0xAA_AAAA, WHITE]
    };
    let bar = w / bars.len();
    for (i, color) in bars.iter().enumerate() {
        for x in [i * bar, i * bar + bar - 1] {
            for y in [0, h - 1] {
                assert_eq!(px[y * w + x], *color, "bar {i} at {x},{y}");
            }
        }
    }
    p.tap(&[Key::Speaker]);
    p.shot("display-4-pixels");
    let (px, _) = p.pixels();
    for y in 0..h {
        for x in 0..w {
            let want = if x % 2 == 0 && y % 2 == 0 { BLACK } else { WHITE };
            assert_eq!(px[y * w + x], want, "{x},{y}");
        }
    }
    p.tap(&[Key::Soft(2)]);
    p.shot("display-5-frame");
    let (px, _) = p.pixels();
    let at = |x: usize, y: usize| px[y * w + x];
    assert_eq!([at(0, 0), at(w - 1, h - 1), at(0, h - 1), at(w - 1, 0)], [BLACK; 4]);
    assert_eq!([at(1, 1), at(w - 2, h - 2)], [WHITE; 2]);
    assert_eq!([at(4, 4), at(w - 5, h - 5), at(4, h - 5)], [0x55_5555; 3]);
    assert_eq!([at(w / 2, 2), at(2, h / 2)], [BLACK; 2]);
    p.tap(&[Key::Mute]);
    p.shot("display-6-fonts");
    assert!(p.colors().len() > 1);
    // The hookswitch is not a key, and the phone does nothing with it.
    p.hook(true);
    p.hook(false);
    assert_eq!(p.ui.screen_name(), "app:diagnostics");
    assert_eq!(p.commands(), 0, "the phone did nothing with the keys");
    // Any key after the last pattern: the menu.
    let fonts = p.pixels().0;
    p.tap(&[Key::Hold]);
    assert_ne!(p.pixels().0, fonts);
    // Cancel leaves at once, and the keys are the phone's again.
    p.tap(&[Key::Digit(1)]);
    assert_eq!(p.colors(), [WHITE]);
    p.tap(&[Key::Cancel]);
    assert!(p.colors().len() > 1);
    p.tap(&[Key::VolumeUp]);
    assert_eq!(p.commands(), 1, "the volume key is the phone's");
    p.tap(&[Key::Line(1)]);
    assert_eq!(p.ui.screen_name(), "idle");
    p.settle();
    assert!(!p.mgr.is_running(APP));
}

fn key_test(model: Model) {
    let mut p = Phone::new(model);
    p.tap(&[Key::Digit(2)]);
    let empty = p.pixels().0;
    p.tap(&[Key::Line(1), Key::VolumeUp, Key::VolumeDown, Key::Speaker, Key::Soft(1), Key::Digit(5), Key::Cancel, Key::Hold]);
    p.hook(true);
    p.hook(false);
    p.shot("keys");
    assert_ne!(p.pixels().0, empty);
    assert_eq!(p.ui.screen_name(), "app:diagnostics");
    assert_eq!(p.commands(), 0, "the phone did nothing with the keys");
    // Cancel twice in a row: the menu, and the keys are the phone's again.
    p.tap(&[Key::Cancel, Key::Cancel]);
    assert_eq!(p.ui.screen_name(), "app:diagnostics");
    p.tap(&[Key::VolumeUp]);
    assert_eq!(p.commands(), 1, "the volume key is the phone's");
    p.tap(&[Key::Line(1)]);
    assert_eq!(p.ui.screen_name(), "idle");
}

fn lamp_test(model: Model) {
    let mut p = Phone::new(model);
    assert_eq!(p.sim.lamp(Lamp::Message), LampColor::Red);
    assert_eq!(p.sim.lamp(Lamp::Line(0)), LampColor::Off);
    p.tap(&[Key::Digit(3)]);
    p.shot("lamps");
    assert_eq!(p.lamps(), LampColor::Red);
    for color in [LampColor::Green, LampColor::Amber, LampColor::Off, LampColor::Red, LampColor::Green] {
        p.timer();
        assert_eq!(p.lamps(), color);
    }
    // Left with Select: the phone's lamps.
    p.tap(&[Key::Select]);
    assert_eq!(p.ui.screen_name(), "app:diagnostics");
    assert_eq!((p.sim.lamp(Lamp::Message), p.sim.lamp(Lamp::Line(0))), (LampColor::Red, LampColor::Off));
    // The app left in the middle of the test (line key 1): the same.
    p.tap(&[Key::Digit(3)]);
    p.timer();
    assert_eq!(p.lamps(), LampColor::Green);
    p.tap(&[Key::Line(1)]);
    assert_eq!(p.ui.screen_name(), "idle");
    assert_eq!((p.sim.lamp(Lamp::Message), p.sim.lamp(Lamp::Line(0))), (LampColor::Red, LampColor::Off));
    p.settle();
    assert!(!p.mgr.is_running(APP));
}

#[test]
fn the_display_test_on_every_skin() {
    for model in [Model::D60, Model::D40, Model::D70] {
        display_test(model);
    }
}

#[test]
fn the_key_test_on_every_skin() {
    for model in [Model::D60, Model::D40, Model::D70] {
        key_test(model);
    }
}

#[test]
fn the_lamp_test_on_every_skin() {
    for model in [Model::D60, Model::D40, Model::D70] {
        lamp_test(model);
    }
}

// ---------------------------------------------------------- the touchscreen

impl Phone {
    /// A finger down and up at (`x`, `y`) of the display.
    fn touch(&mut self, x: u16, y: u16) {
        self.sim.touch_tap(x, y);
        self.settle();
    }
}

/// The D80's title and back button: 96 pixels each way, at the top-left.
const CORNER: usize = 96;

#[test]
fn the_display_test_on_the_touchscreen() {
    let model = Model::D80;
    let mut p = Phone::new(model);
    let size = model.main_screen();
    let (w, h) = (size.width as usize, size.height as usize);
    // The pixels of the display without the corner the back button is
    // drawn over, each with its place.
    let rest = |p: &Phone| -> Vec<(usize, usize, u32)> {
        let (px, _) = p.pixels();
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|(x, y)| *x >= CORNER || *y >= CORNER).map(|(x, y)| (x, y, px[y * w + x])).collect()
    };
    p.shot("menu");
    let menu = p.pixels().0;
    // The first row of the menu, under the title.
    p.touch(400, 96 + 48);
    p.shot("display-1-white");
    assert!(rest(&p).iter().all(|(_, _, c)| *c == WHITE), "the whole display, the softkey buttons' part too");
    // The back button is over the pattern's corner, on a round button of
    // its own: it shows on white, with dark and light of its own, as it
    // does on black.
    let corner = |p: &Phone| -> (bool, bool) {
        let (px, _) = p.pixels();
        let green = |c: &u32| (c >> 8) & 0xFF;
        let corner: Vec<u32> = (0..CORNER).flat_map(|y| px[y * w..y * w + CORNER].to_vec()).collect();
        (corner.iter().any(|c| green(c) < 0x60), corner.iter().any(|c| green(c) > 0xA0))
    };
    assert_eq!(corner(&p), (true, true), "the button and its arrow on white");
    // A touch anywhere shows the next pattern.
    p.touch(700, 1200);
    p.shot("display-2-black");
    assert!(rest(&p).iter().all(|(_, _, c)| *c == BLACK));
    assert_eq!(corner(&p), (true, true), "the button and its arrow on black");
    p.touch(10, 600);
    p.shot("display-3-bars");
    let bars = [0xFF_0000, 0x00_FF00, 0x00_00FF, 0xFF_FF00, 0x00_FFFF, 0xFF_00FF, WHITE, BLACK];
    let bar = w / bars.len();
    let (px, _) = p.pixels();
    for (i, color) in bars.iter().enumerate() {
        for x in [i * bar, i * bar + bar - 1] {
            assert_eq!(px[(h - 1) * w + x], *color, "bar {i} at {x}");
        }
    }
    // The display has more pixels than a drawing may have of bitmap:
    // squares of two pixels, every other one of every other row.
    p.touch(400, 400);
    p.shot("display-4-pixels");
    for (x, y, c) in rest(&p) {
        let want = if (x / 2) % 2 == 0 && (y / 2) % 2 == 0 { BLACK } else { WHITE };
        assert_eq!(c, want, "{x},{y}");
    }
    p.touch(400, 400);
    p.shot("display-5-frame");
    let (px, _) = p.pixels();
    let at = |x: usize, y: usize| px[y * w + x];
    assert_eq!([at(w - 1, h - 1), at(0, h - 1), at(w - 1, 0)], [BLACK; 3]);
    assert_eq!([at(w - 5, h - 5), at(4, h - 5)], [0x55_5555; 2]);
    assert_eq!([at(w / 2, 2), at(2, h / 2)], [BLACK; 2]);
    p.touch(400, 400);
    p.shot("display-6-fonts");
    let fonts = p.pixels().0;
    // The sample lines begin under the back button, not behind it.
    let background = fonts[w - 1];
    assert!((0..CORNER).all(|y| fonts[y * w + CORNER..(y + 1) * w].iter().all(|c| *c == background)));
    // A touch after the last pattern: the menu.
    p.touch(400, 400);
    assert_eq!(p.pixels().0, menu);
    assert_eq!(p.commands(), 0);
    // The back button leaves the test at once.
    p.touch(400, 96 + 48);
    p.touch(400, 400);
    assert!(rest(&p).iter().all(|(_, _, c)| *c == BLACK));
    p.touch(48, 48);
    assert_eq!(p.pixels().0, menu);
    // And from the menu, the app.
    p.touch(48, 48);
    assert_ne!(p.ui.screen_name(), "app:diagnostics");
}

#[test]
fn the_menu_on_the_touchscreen_has_no_key_test() {
    let mut r = common::Rig::with_root(staged(), FakeConfig::default(), Model::D80);
    r.launch(APP);
    let items: Vec<String> = r.ui.items(APP).into_iter().map(|i| i.0).collect();
    assert_eq!(items, ["Display test", "Lamp test", "Tone test", "Subsystems", "Log"]);
    // A row is touched, and the back button goes back: no softkeys.
    assert_eq!(r.ui.softkeys(APP), [""; 0]);
    // The second row is the lamp test, not the key test: nothing takes
    // the keys.
    r.ui.tap_row(APP, 1);
    r.settle();
    assert!(r.ui.shows(APP, "Lamp test"));
    assert!(!r.ui.captures(APP));
    assert_eq!(r.ui.softkeys(APP), [""; 0]);
    r.key(APP, Key::Cancel);
    // The fourth: the subsystems, a page to read.
    r.ui.tap_row(APP, 3);
    r.settle();
    assert_eq!(r.ui.texts(APP)[0], "Subsystems");
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP), ["Diagnostics"]);
    // The display test takes touches, and gives them back.
    r.ui.tap_row(APP, 0);
    r.settle();
    assert!(r.ui.full_screen(APP) && r.ui.takes_touches(APP, 0));
    for _ in 0..6 {
        r.ui.tap(APP, 400, 640);
        r.settle();
    }
    assert!(!r.ui.full_screen(APP) && !r.ui.takes_touches(APP, 0));
    assert_eq!(r.ui.texts(APP), ["Diagnostics"]);
    r.key(APP, Key::Cancel);
    assert_ne!(r.ui.top().as_deref(), Some(APP));
}
