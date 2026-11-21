//! Desk Sparrow, the game: the app's life under the manager with the fake
//! screen, and the real thing, drawn by the phone UI on a simulated D60
//! (color) and D40 (gray), with a player that reads the picture.
//!
//! The pictures are written as PNG files under `desk-sparrow/` in cargo's
//! test scratch directory (`CARGO_TARGET_TMPDIR`), or in the directory
//! `DESK_SPARROW_PREVIEWS` names.  The rules of the game itself are tested
//! where they are, in `apps/desksparrow/src/game.rs`, and the rows of the
//! leaderboard in `apps/desksparrow/src/board.rs`.
//!
//! The leaderboard is tested with phones of one PBX, whose store of app
//! data is `FakeStore`.

mod common;

use common::{Rig, staged, temp_dir};
use opendeskphone_apps::engine::Target;
use opendeskphone_apps::testutil::{FakeConfig, FakePbx, FakeStore, local_value, services_with_store};
use opendeskphone_apps::uishell::UiShell;
use opendeskphone_apps::{
    AppManager, AppWindow, Color, ColorRole, DrawOp, ManagerConfig, Rect, Shell, StoreOp,
    StoreReply, StoreScope, WindowEvent, WindowKind,
};
use opendeskphone_hal::{Hal, Screen};
use opendeskphone_state::{
    AccountId, Call, CallState, Calls, Direction, Key, Model, Party, Phase, Store, System,
};
use opendeskphone_ui::{Ui, UiConfig};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const APP: &str = "desksparrow";

/// The three lines of the start screen.
const START: [&str; 3] = [
    "Desk Sparrow",
    "Any key flaps.  Cancel leaves.",
    "Down shows the leaderboard.",
];
const BOARD_HINT: &str = "Down shows the leaderboard.";

/// What the tests need to know of the game on a model: the width of a
/// pipe's flange and its height, the sparrow's size and place, the height
/// of an opening and of the field, and the length of a tick.
#[derive(Clone, Copy)]
struct Measures {
    pipe_w: u16,
    flange_h: u16,
    bird_w: i16,
    bird_h: i16,
    bird_x: i16,
    gap: i16,
    field_h: i16,
    tick_ms: u64,
}

fn measures(model: Model) -> Measures {
    match model {
        Model::D40 => Measures {
            pipe_w: 20,
            flange_h: 5,
            bird_w: 16,
            bird_h: 12,
            bird_x: 44,
            gap: 50,
            field_h: 116,
            tick_ms: 100,
        },
        Model::D80 => Measures {
            pipe_w: 88,
            flange_h: 20,
            bird_w: 64,
            bird_h: 48,
            bird_x: 160,
            gap: 192,
            field_h: 1248,
            tick_ms: 50,
        },
        Model::D70 => Measures {
            pipe_w: 24,
            flange_h: 5,
            bird_w: 16,
            bird_h: 12,
            bird_x: 60,
            gap: 64,
            field_h: 156,
            tick_ms: 100,
        },
        _ => Measures {
            pipe_w: 44,
            flange_h: 10,
            bird_w: 32,
            bird_h: 24,
            bird_x: 96,
            gap: 96,
            field_h: 264,
            tick_ms: 50,
        },
    }
}

/// The sparrow's top edge: its body is the first picture drawn.
fn bird_y(ops: &[DrawOp]) -> i16 {
    ops.iter()
        .find_map(|op| match op {
            DrawOp::Bitmap(b) => Some(b.y),
            _ => None,
        })
        .expect("a sparrow")
}

fn bird_x(ops: &[DrawOp]) -> i16 {
    ops.iter()
        .find_map(|op| match op {
            DrawOp::Bitmap(b) => Some(b.x),
            _ => None,
        })
        .expect("a sparrow")
}

/// The openings on screen, left to right: left edge, top, and bottom.  A
/// pipe's flange is the one rectangle as wide as the pipe and as high as
/// a flange; the upper pipe's is above the opening, the lower one's below.
fn openings(ops: &[DrawOp], m: Measures) -> Vec<(i16, i16, i16)> {
    let mut flanges: Vec<(i16, i16)> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Fill((r, _)) if r.width == m.pipe_w && r.height == m.flange_h => {
                Some((r.x, r.y))
            }
            _ => None,
        })
        .collect();
    flanges.sort();
    flanges.dedup();
    flanges
        .chunks(2)
        .map(|pair| {
            assert_eq!(pair.len(), 2, "a pipe without its partner");
            assert_eq!(pair[0].0, pair[1].0);
            (pair[0].0, pair[0].1 + m.flange_h as i16, pair[1].1)
        })
        .collect()
}

/// A player: flaps when the sparrow, falling as it fell in the last tick,
/// would sink too low in the next opening.
struct Player {
    m: Measures,
    last_y: Option<i16>,
}

impl Player {
    fn new(m: Measures) -> Player {
        Player { m, last_y: None }
    }
    fn flaps(&mut self, ops: &[DrawOp]) -> bool {
        let m = self.m;
        let y = bird_y(ops);
        let fall = y - self.last_y.replace(y).unwrap_or(y);
        let bottom = openings(ops, m)
            .into_iter()
            .find(|(x, _, _)| x + m.pipe_w as i16 > m.bird_x)
            .map(|(_, _, bottom)| bottom)
            // Nothing ahead yet: stay in the middle.
            .unwrap_or((m.field_h + m.gap) / 2);
        y + fall + m.bird_h + m.gap / 6 > bottom
    }
}

// ------------------------------------------------- with the fake screen

/// The game's canvas on the fake screen.
fn drawing(r: &Rig) -> Vec<DrawOp> {
    let mut all = r.ui.drawings(APP);
    assert_eq!(all.len(), 1, "one canvas");
    all.remove(0).1
}

/// Runs the manager until the game has drawn again (a tick has passed).
fn next_tick(r: &mut Rig) {
    let before = r.ui.draw_count(APP);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < until, "the game stopped drawing");
        let wait = r.mgr.pump();
        if r.ui.draw_count(APP) != before {
            break;
        }
        std::thread::sleep(
            wait.expect("the game's timer is running")
                .min(Duration::from_millis(20)),
        );
    }
}

fn game_over(r: &Rig) -> bool {
    r.ui.shows(APP, "Game over")
}

#[test]
fn start_screen_flight_and_game_over() {
    let mut r = Rig::new(FakeConfig::default());
    let m = measures(Model::D60);

    r.launch(APP);
    assert_eq!(r.ui.texts(APP), START);
    // The whole display, and no softkeys.
    assert!(r.ui.full_screen(APP));
    assert!(r.ui.softkeys(APP).is_empty());
    // One canvas over the whole window, drawn once; nothing ticks on the
    // start screen.
    let (bounds, ops) = r.ui.drawings(APP).remove(0);
    assert_eq!(
        (bounds.x, bounds.y, bounds.width, bounds.height),
        (0, 0, 480, 272)
    );
    assert!(openings(&ops, m).is_empty());
    assert_eq!(r.ui.draw_count(APP), 1);
    assert_eq!(r.mgr.pump(), None, "no timer on the start screen");

    // Select starts a game with a flap: the sparrow is where it flies, in
    // the middle of the field, and there is a score.
    r.key(APP, Key::Select);
    assert!(r.ui.texts(APP).is_empty());
    assert!(r.ui.softkeys(APP).is_empty());
    let ops = drawing(&r);
    assert_eq!(bird_x(&ops), m.bird_x);
    let y0 = bird_y(&ops);
    assert_eq!(y0, (m.field_h - m.bird_h) / 2);
    let tick = r.mgr.pump().expect("the game's timer");
    assert!(tick <= Duration::from_millis(m.tick_ms));

    // It rises from the flap, more slowly each tick, and then falls
    // faster and faster.
    let mut ys = vec![y0];
    for _ in 0..12 {
        next_tick(&mut r);
        assert!(!game_over(&r));
        ys.push(bird_y(&drawing(&r)));
    }
    let top = ys
        .iter()
        .position(|y| y == ys.iter().min().unwrap())
        .unwrap();
    assert!((5..=8).contains(&top), "{ys:?}");
    assert!(ys[..=top].windows(2).all(|w| w[1] <= w[0]), "{ys:?}");
    assert!(ys[top..].windows(2).all(|w| w[1] >= w[0]), "{ys:?}");
    assert!(
        ys[0] - ys[top] > m.bird_h && ys[12] > ys[top] + m.bird_h / 2,
        "{ys:?}"
    );
    let falls: Vec<i16> = ys[top..].windows(2).map(|w| w[1] - w[0]).collect();
    assert!(falls.windows(2).all(|w| w[1] >= w[0]), "{falls:?}");

    // Another flap, by a digit this time, sends it up again.
    let before = *ys.last().unwrap();
    r.key(APP, Key::Digit(5));
    next_tick(&mut r);
    assert!(bird_y(&drawing(&r)) < before);

    // Left alone it hits the floor: the game is over and nothing ticks.
    let until = Instant::now() + Duration::from_secs(10);
    while !game_over(&r) {
        assert!(Instant::now() < until);
        next_tick(&mut r);
    }
    assert_eq!(
        r.ui.texts(APP),
        ["Game over", "Score 0   Best 0", BOARD_HINT]
    );
    assert!(r.ui.softkeys(APP).is_empty());
    assert_eq!(bird_y(&drawing(&r)), m.field_h - m.bird_h);
    assert_eq!(r.mgr.pump(), None, "no timer after the game");

    // A key right after the crash is the last flap of the lost game, not
    // a new game; a moment later it is.
    r.key(APP, Key::Select);
    assert!(game_over(&r));
    std::thread::sleep(Duration::from_millis(750));
    r.key(APP, Key::Select);
    assert!(!game_over(&r));
    assert_eq!(bird_y(&drawing(&r)), y0);
    assert!(r.mgr.pump().is_some());

    // Cancel, which the game leaves to the phone, closes it, and with
    // that the app stops.
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
    assert_eq!(r.ui.top(), None);
    assert_eq!(r.mgr.pump(), None);
}

#[test]
fn a_covered_game_stops() {
    // A call's screen (or any other) over the game: the app is told it
    // left the screen, and, as an app that is only its window, stops.
    let mut r = Rig::new(FakeConfig::default());
    r.launch(APP);
    r.key(APP, Key::Select);
    next_tick(&mut r);
    assert!(r.mgr.pump().is_some());
    r.ui.cover(APP, true);
    r.settle();
    assert!(!r.mgr.is_running(APP));
    assert_eq!(r.mgr.pump(), None, "its timer is gone");
}

#[test]
fn every_keypad_key_plays_and_cancel_leaves() {
    let mut r = Rig::new(FakeConfig::default());
    // Where the sparrow is three ticks after a game started with one flap.
    let after = |r: &mut Rig, start: Key, again: Option<Key>| {
        r.launch(APP);
        assert!(r.ui.shows(APP, "Desk Sparrow"));
        r.key(APP, start);
        assert!(r.ui.texts(APP).is_empty(), "{start:?} starts a game");
        next_tick(r);
        if let Some(key) = again {
            r.key(APP, key);
        }
        next_tick(r);
        next_tick(r);
        let y = bird_y(&drawing(r));
        // Cancel is not the game's: it closes the app.
        r.key(APP, Key::Cancel);
        assert!(!r.mgr.is_running(APP));
        y
    };
    let once = after(&mut r, Key::Select, None);
    let mut keys: Vec<Key> = (0..=9).map(Key::Digit).collect();
    keys.extend([Key::Star, Key::Pound, Key::Select]);
    for key in keys {
        // The key starts a game as Select does, and flaps in it: after a
        // second flap the sparrow is higher than after one.
        let twice = after(&mut r, key, Some(key));
        assert!(twice < once, "{key:?}: {twice} against {once}");
    }
    // The other keys are not the game's: nothing starts.  (Down and Up
    // show the leaderboard and take it away.)
    r.launch(APP);
    for key in [Key::Left, Key::Right, Key::Soft(1), Key::Soft(4)] {
        r.key(APP, key);
        assert!(r.ui.shows(APP, "Desk Sparrow"), "{key:?}");
        assert_eq!(r.mgr.pump(), None, "{key:?}");
    }
}

#[test]
fn points_the_best_score_and_the_seed() {
    let m = measures(Model::D60);
    // The same seed: the same pipes.
    let first_openings = |seed: &str| {
        let mut r = Rig::new(FakeConfig::default());
        let dir = r.nvdata.join("appdata").join(APP);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("seed"), seed).unwrap();
        r.launch(APP);
        r.key(APP, Key::Select);
        let mut player = Player::new(m);
        let mut seen = Vec::new();
        while seen.len() < 2 {
            next_tick(&mut r);
            let ops = drawing(&r);
            if player.flaps(&ops) {
                r.key(APP, Key::Select);
            }
            seen = openings(&ops, m).iter().map(|o| o.1).collect();
        }
        seen
    };
    let a = first_openings("12345\n");
    assert_eq!(a, first_openings("12345\n"));
    assert_ne!(a, first_openings("54321\n"));

    // A game played through two openings, then dropped: two points, and
    // the best score is in the app's storage.
    let mut r = Rig::new(FakeConfig::default());
    let best = r.nvdata.join("appdata").join(APP).join("best");
    r.launch(APP);
    r.key(APP, Key::Select);
    let mut player = Player::new(m);
    let mut passed = 0;
    let mut ahead_before = None;
    let until = Instant::now() + Duration::from_secs(40);
    while passed < 2 {
        assert!(Instant::now() < until);
        next_tick(&mut r);
        assert!(!game_over(&r), "the player crashed");
        let ops = drawing(&r);
        if player.flaps(&ops) {
            r.key(APP, Key::Select);
        }
        // An opening is passed when its pipe's right edge is no longer to
        // the right of the sparrow's left edge.
        let ahead = openings(&ops, m)
            .iter()
            .filter(|(x, _, _)| x + m.pipe_w as i16 > m.bird_x)
            .map(|(x, _, _)| *x)
            .next();
        if let (Some(before), Some(now)) = (ahead_before, ahead)
            && now > before
        {
            passed += 1;
        }
        ahead_before = ahead;
    }
    assert!(!best.exists(), "nothing is written during a game");
    while !game_over(&r) {
        next_tick(&mut r);
    }
    assert_eq!(
        r.ui.texts(APP),
        ["Game over", "Score 2   Best 2", BOARD_HINT]
    );
    assert_eq!(file(&r, "best").as_deref(), Some("2"));

    // A worse game leaves the best score alone, in a new run of the app
    // too.
    r.key(APP, Key::Cancel);
    std::fs::write(&best, "17").unwrap();
    r.launch(APP);
    r.key(APP, Key::Select);
    while !game_over(&r) {
        next_tick(&mut r);
    }
    assert_eq!(
        r.ui.texts(APP),
        ["Game over", "Score 0   Best 17", BOARD_HINT]
    );
    assert_eq!(file(&r, "best").as_deref(), Some("17"));
}

#[test]
fn it_plays_on_the_gray_screens_in_gray() {
    for model in [Model::D40, Model::D70] {
        let m = measures(model);
        let mut r = Rig::with_root(staged(), FakeConfig::default(), model);
        r.launch(APP);
        assert_eq!(r.ui.texts(APP), START);
        let area = model.main_screen();
        r.key(APP, Key::Select);
        let started = Instant::now();
        let mut player = Player::new(m);
        // Until the first pipe is at the sparrow.
        loop {
            next_tick(&mut r);
            assert!(!game_over(&r), "the player crashed on {model:?}");
            let ops = drawing(&r);
            if player.flaps(&ops) {
                r.key(APP, Key::Select);
            }
            for op in &ops {
                let (c, right, bottom) = match op {
                    DrawOp::Fill((r, c)) => (c, r.x + r.width as i16, r.y + r.height as i16),
                    DrawOp::Bitmap(b) => (&b.color, b.x, b.y),
                    DrawOp::Text(t) => panic!("no text in this game: {t:?}"),
                };
                assert!(c.r == c.g && c.g == c.b, "a color on a gray screen: {c:?}");
                assert!(bottom <= area.height as i16, "{op:?}");
                assert!(right <= area.width as i16 + m.pipe_w as i16, "{op:?}");
            }
            if openings(&ops, m)
                .iter()
                .any(|(x, _, _)| *x <= m.bird_x + m.bird_w)
            {
                break;
            }
        }
        // Ten ticks a second, not twenty: the gray panels are slow.
        let ticks = r.ui.draw_count(APP) - 2;
        let per_tick = started.elapsed().as_millis() as u64 / u64::from(ticks);
        assert!(per_tick >= m.tick_ms, "{per_tick} ms a tick on {model:?}");
        r.mgr.close(APP);
    }
}

// --------------------------------------------------- with the phone UI

/// The canvas as the app last drew it, how many times it has drawn, and
/// its text widgets.
#[derive(Default)]
struct Seen {
    draws: u32,
    ops: Vec<DrawOp>,
    labels: std::collections::BTreeMap<u32, Label>,
}

/// A text widget: its width, its font, its text, and whether it is shown.
struct Label {
    width: u16,
    font: opendeskphone_apps::Font,
    text: String,
    visible: bool,
}

/// The phone UI's shell, with a look at what the app draws.
struct Watch {
    inner: UiShell,
    seen: Arc<Mutex<Seen>>,
}

struct WatchedWindow {
    inner: Box<dyn AppWindow>,
    seen: Arc<Mutex<Seen>>,
}

impl Shell for Watch {
    fn open(
        &mut self,
        app: &str,
        kind: WindowKind,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Box<dyn AppWindow> {
        Box::new(WatchedWindow {
            inner: self.inner.open(app, kind, wake),
            seen: self.seen.clone(),
        })
    }
    fn show(&mut self, app: &str) {
        self.inner.show(app)
    }
    fn hide(&mut self, app: &str) {
        self.inner.hide(app)
    }
    fn close(&mut self, app: &str, kind: WindowKind) {
        self.inner.close(app, kind)
    }
    fn idle_size(&self) -> opendeskphone_apps::view::Size {
        self.inner.idle_size()
    }
}

impl AppWindow for WatchedWindow {
    fn metrics(&self) -> opendeskphone_apps::Metrics {
        self.inner.metrics()
    }
    fn create(&mut self, kind: opendeskphone_apps::WidgetKind, parent: Option<u32>, bounds: Rect) -> u32 {
        let w = self.inner.create(kind, parent, bounds);
        if kind == opendeskphone_apps::WidgetKind::Text {
            let label = Label {
                width: bounds.width,
                font: opendeskphone_apps::Font::Normal,
                text: String::new(),
                visible: true,
            };
            self.seen.lock().unwrap().labels.insert(w, label);
        }
        w
    }
    fn destroy(&mut self, w: u32) {
        self.seen.lock().unwrap().labels.remove(&w);
        self.inner.destroy(w)
    }
    fn clear(&mut self, w: Option<u32>) {
        // The game clears only its window, of which every widget is a
        // child.
        self.seen.lock().unwrap().labels.clear();
        self.inner.clear(w)
    }
    fn set_bounds(&mut self, w: u32, bounds: Rect) {
        if let Some(l) = self.seen.lock().unwrap().labels.get_mut(&w) {
            l.width = bounds.width;
        }
        self.inner.set_bounds(w, bounds)
    }
    fn set_visible(&mut self, w: u32, visible: bool) {
        if let Some(l) = self.seen.lock().unwrap().labels.get_mut(&w) {
            l.visible = visible;
        }
        self.inner.set_visible(w, visible)
    }
    fn set_focus(&mut self, w: Option<u32>) {
        self.inner.set_focus(w)
    }
    fn set_text(&mut self, w: u32, text: &str) {
        if let Some(l) = self.seen.lock().unwrap().labels.get_mut(&w) {
            l.text = text.to_string();
        }
        self.inner.set_text(w, text)
    }
    fn set_align(&mut self, w: u32, align: opendeskphone_apps::Align) {
        self.inner.set_align(w, align)
    }
    fn set_font(&mut self, w: u32, font: opendeskphone_apps::Font) {
        if let Some(l) = self.seen.lock().unwrap().labels.get_mut(&w) {
            l.font = font;
        }
        self.inner.set_font(w, font)
    }
    fn set_wrap(&mut self, w: u32, wrap: bool) {
        self.inner.set_wrap(w, wrap)
    }
    fn set_highlight(&mut self, w: u32, on: bool) {
        self.inner.set_highlight(w, on)
    }
    fn set_items(&mut self, w: u32, items: &[opendeskphone_apps::ListItem]) {
        self.inner.set_items(w, items)
    }
    fn set_selected(&mut self, w: u32, index: u32) {
        self.inner.set_selected(w, index)
    }
    fn selected(&self, w: u32) -> Option<u32> {
        self.inner.selected(w)
    }
    fn hold_changes(&mut self, on: bool) {
        self.inner.hold_changes(on)
    }
    fn set_value(&mut self, w: u32, value: &str) {
        self.inner.set_value(w, value)
    }
    fn value(&self, w: u32) -> String {
        self.inner.value(w)
    }
    fn set_input_mode(&mut self, w: u32, mode: opendeskphone_apps::InputMode) {
        self.inner.set_input_mode(w, mode)
    }
    fn set_max_length(&mut self, w: u32, max: u32) {
        self.inner.set_max_length(w, max)
    }
    fn set_image(&mut self, w: u32, image: &opendeskphone_apps::view::Image) {
        self.inner.set_image(w, image)
    }
    fn set_full_screen(&mut self, on: bool) {
        self.inner.set_full_screen(on)
    }
    fn theme_color(&self, role: ColorRole) -> Color {
        self.inner.theme_color(role)
    }
    fn text_size(&self, font: opendeskphone_apps::Font, text: &str) -> (u16, u16) {
        self.inner.text_size(font, text)
    }
    fn create_canvas(&mut self, parent: Option<u32>, bounds: Rect) -> u32 {
        self.inner.create_canvas(parent, bounds)
    }
    fn draw(&mut self, w: u32, ops: Vec<DrawOp>) -> bool {
        {
            let mut seen = self.seen.lock().unwrap();
            seen.draws += 1;
            seen.ops = ops.clone();
        }
        self.inner.draw(w, ops)
    }
    fn set_softkeys(&mut self, w: Option<u32>, keys: &[opendeskphone_apps::Softkey]) {
        self.inner.set_softkeys(w, keys)
    }
    fn claim_keys(&mut self, w: Option<u32>, keys: &[Key]) {
        self.inner.claim_keys(w, keys)
    }
    fn claim_touch(&mut self, w: Option<u32>, on: bool) {
        self.inner.claim_touch(w, on)
    }
    fn poll_event(&mut self) -> Option<WindowEvent> {
        self.inner.poll_event()
    }
}

/// A simulated phone: the phone UI on the simulator's display and keys,
/// and the app manager with the real apps.
struct Phone {
    model: Model,
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    mgr: AppManager,
    seen: Arc<Mutex<Seen>>,
    _dir: common::TempDir,
}

impl Phone {
    fn new(model: Model) -> Phone {
        Phone::on_pbx(model, &FakeStore::default())
    }

    /// A phone whose PBX store of app data is `app_data`.
    fn on_pbx(model: Model, app_data: &FakeStore) -> Phone {
        let dir = temp_dir("sparrow-phone");
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        // The first theme, whose palette is the color skin's own: the
        // colors this test names are that palette's.
        let ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { theme: "harbor".into(), ..UiConfig::default() });
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        let seen = Arc::new(Mutex::new(Seen::default()));
        let shell = Watch {
            inner: UiShell::new(ui.handle(), model),
            seen: seen.clone(),
        };
        let mut cfg = ManagerConfig::rooted(staged(), &dir.0.join("nvdata"));
        cfg.target = Target::Pulley64;
        let mut mgr = AppManager::new(
            cfg,
            store.clone(),
            Box::new(shell),
            services_with_store(&FakePbx::default(), FakeConfig::default(), app_data),
        )
        .expect("engine");
        mgr.scan();
        let mut phone = Phone {
            model,
            store,
            ui,
            sim,
            mgr,
            seen,
            _dir: dir,
        };
        phone.settle();
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

    fn tap(&mut self, key: Key) {
        self.sim.tap(key);
        self.settle();
    }

    fn draws(&self) -> u32 {
        self.seen.lock().unwrap().draws
    }

    fn ops(&self) -> Vec<DrawOp> {
        self.seen.lock().unwrap().ops.clone()
    }

    /// Waits for the game's next tick and lets the UI paint it.  Returns
    /// how many times the display was sent something.
    fn next_tick(&mut self) -> u64 {
        let before = self.draws();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < until, "the game stopped drawing");
            let wait = self.mgr.pump();
            if self.draws() != before {
                break;
            }
            std::thread::sleep(
                wait.expect("the game's timer")
                    .min(Duration::from_millis(20)),
            );
        }
        let flushes = self.sim.frame_generation();
        self.ui.pump();
        self.sim.frame_generation() - flushes
    }

    fn frame(&self) -> opendeskphone_hal::Frame {
        self.sim.frame(Screen::Main).expect("main frame")
    }

    /// The pixels of the frame as 0xRRGGBB.
    fn pixels(&self) -> Vec<u32> {
        let f = self.frame();
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
        out
    }

    fn count(&self, color: u32) -> usize {
        self.pixels().iter().filter(|p| **p == color).count()
    }

    /// The texts on screen that the phone cuts short, as they are wider
    /// in their fonts than their widgets.
    fn cut(&self) -> Vec<String> {
        use opendeskphone_apps::Font as App;
        use opendeskphone_ui::appview::Font as Ui;
        let theme = opendeskphone_ui::skin::Theme::for_model(self.model);
        let font = |f: App| match f {
            App::Small => Ui::Small,
            App::Normal => Ui::Normal,
            App::Large => Ui::Large,
            App::Bold => Ui::Bold,
            App::Title => Ui::Title,
        };
        self.seen
            .lock()
            .unwrap()
            .labels
            .values()
            .filter(|l| l.visible && theme.text_width(font(l.font), &l.text) > l.width)
            .map(|l| l.text.clone())
            .collect()
    }

    /// Writes the frame as a PNG (the gray ones doubled in size).
    fn shot(&self, name: &str) {
        let dir = match std::env::var_os("DESK_SPARROW_PREVIEWS") {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("desk-sparrow"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        let f = self.frame();
        let (w, h) = (f.info.width as usize, f.info.height as usize);
        let scale = if self.model.is_color() { 1 } else { 2 };
        let pixels = self.pixels();
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

/// Plays a game on the simulated phone: the start screen, a flight to the
/// second pipe, and the end, with a picture of each.
fn play_on(model: Model) -> Phone {
    let m = measures(model);
    let mut p = Phone::new(model);
    assert_eq!(p.ui.screen_name(), "idle");
    p.mgr.launch(APP).unwrap();
    p.settle();
    assert_eq!(p.ui.screen_name(), "app:desksparrow");
    p.shot("1-start");
    let start = p.pixels();
    // The game has the whole display: where the softkey bar would be is
    // the floor, one color from side to side.
    let size = model.main_screen();
    let (w, h) = (size.width as usize, size.height as usize);
    let floor = start[(h - 1) * w];
    assert!(start[(h - 2) * w..].iter().all(|p| *p == floor));
    assert!(
        start[(m.field_h as usize + 3) * w..][..w]
            .iter()
            .all(|p| *p == floor)
    );

    // The softkey keys do nothing now.  A digit reaches the game through
    // the phone's keys, and starts it; it is no number to dial.
    p.tap(Key::Soft(1));
    assert_eq!(p.mgr.pump(), None);
    p.tap(Key::Digit(5));
    assert!(p.mgr.pump().is_some(), "the game's timer runs");
    assert_eq!(p.ui.screen_name(), "app:desksparrow");
    assert_eq!(p.store.get::<opendeskphone_state::Ui>().dial_buffer, "");
    let mut player = Player::new(m);
    let mut shot = false;
    let mut score = 0;
    let mut ahead_before = None;
    while score < 2 {
        // One tick is one repaint, and one transfer to the display at
        // most (none when the picture is the same: the sparrow at the top
        // of its rise before the first pipe shows).
        assert!(p.next_tick() <= 1);
        let ops = p.ops();
        let open = openings(&ops, m);
        if player.flaps(&ops) {
            p.tap(Key::Select);
        }
        let ahead = open
            .iter()
            .map(|o| o.0)
            .find(|x| x + m.pipe_w as i16 > m.bird_x);
        if let (Some(before), Some(now)) = (ahead_before, ahead)
            && now > before
        {
            score += 1;
        }
        ahead_before = ahead;
        // The picture: one point made, the sparrow in the second opening.
        if !shot && score == 1 && ahead.is_some_and(|x| x <= m.bird_x + m.bird_w / 2) {
            p.shot("2-flying");
            shot = true;
        }
    }
    assert!(shot);
    assert_eq!(p.ui.screen_name(), "app:desksparrow");
    while p.mgr.pump().is_some() {
        p.next_tick();
    }
    p.settle();
    p.shot("3-game-over");
    assert_ne!(p.pixels(), start);
    p
}

#[test]
fn drawn_on_a_color_phone() {
    let mut p = play_on(Model::D60);
    // The pipes are the skin's blue (its selection color), and the
    // sparrow is brown with a pale belly.
    assert!(p.count(0x01_579B) > 2000, "pipes in the skin's blue");
    assert!(p.count(0xBC_864E) > 100, "a brown sparrow");
    assert!(p.count(0xF3_E6CC) > 40, "with a pale belly");

    // Cancel on the phone's keypad leaves the game: the idle screen is
    // back, with its softkeys, and the app has stopped.
    std::thread::sleep(Duration::from_millis(750));
    p.tap(Key::Pound);
    p.next_tick();
    p.tap(Key::Cancel);
    assert_eq!(p.ui.screen_name(), "idle");
    assert!(!p.mgr.is_running(APP));
    assert_eq!(p.store.get::<opendeskphone_state::Ui>().dial_buffer, "");

    // A call comes in: its screen takes the display and the game stops.
    p.mgr.launch(APP).unwrap();
    p.settle();
    p.tap(Key::Star);
    p.next_tick();
    assert!(p.mgr.is_running(APP));
    let id = p.store.next_call_id();
    p.store.update::<Calls, _>(|c| {
        let from = Party {
            number: "1002".into(),
            ..Default::default()
        };
        let mut call = Call::new(id, AccountId(0), Direction::Incoming, from);
        call.state = CallState::Ringing;
        c.calls.push(call);
        c.focused = Some(id);
    });
    p.settle();
    assert_eq!(p.ui.screen_name(), "call");
    assert!(!p.mgr.is_running(APP));
    assert_eq!(p.mgr.pump(), None, "nothing of the game still ticks");
}

#[test]
fn drawn_on_a_gray_phone() {
    for model in [Model::D40, Model::D70] {
        drawn_in_gray(model);
    }
}

fn drawn_in_gray(model: Model) {
    let p = play_on(model);
    // Only the panel's four levels, and mostly black and white: black
    // pipes on a white field.
    let pixels = p.pixels();
    assert!(
        pixels
            .iter()
            .all(|c| [0x00_0000, 0x55_5555, 0xAA_AAAA, 0xFF_FFFF].contains(c))
    );
    let share = |c: u32| pixels.iter().filter(|p| **p == c).count() * 100 / pixels.len();
    assert!(share(0xFF_FFFF) > 50, "a white field");
    assert!(share(0x00_0000) > 4, "black pipes");
    assert!(share(0xAA_AAAA) + share(0x55_5555) < 12, "few grays");
}

// ------------------------------------------------------ the touchscreen

/// The touchscreen's start screen: no key is named, and the leaderboard
/// has a button.
const TOUCH_START: [&str; 4] = ["Desk Sparrow", "Touch to flap.", "", "Leaderboard"];

/// The button under the plate: the last plate drawn, whose outer
/// rectangle is the last fill but one.
fn button(ops: &[DrawOp]) -> Rect {
    let fills: Vec<Rect> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Fill((r, _)) => Some(*r),
            _ => None,
        })
        .collect();
    fills[fills.len() - 2]
}

#[test]
fn it_is_played_by_touch_where_there_are_no_keys() {
    let mut r = Rig::with_root(staged(), FakeConfig::default(), Model::D80);
    let m = measures(Model::D80);
    r.launch(APP);
    assert_eq!(r.ui.texts(APP), TOUCH_START);
    // The whole upright display, and the window takes the touches.
    assert!(r.ui.full_screen(APP));
    assert!(r.ui.takes_touches(APP, 0));
    let (bounds, ops) = r.ui.drawings(APP).remove(0);
    assert_eq!((bounds.x, bounds.y, bounds.width, bounds.height), (0, 0, 800, 1280));
    assert_eq!(r.mgr.pump(), None, "no timer on the start screen");
    // The button is large enough to touch, under the plate, and clear of
    // the corner where the phone has its back button.
    let b = button(&ops);
    assert!(b.width >= 88 && b.height >= 88, "{b:?}");
    assert!(b.x > 96 && b.y > 96 && b.x as u16 + b.width <= 800 && b.y as u16 + b.height <= 1248, "{b:?}");
    let on_button = (i32::from(b.x) + 10, i32::from(b.y) + 10);

    // The button shows the leaderboard, and takes it away again.
    r.ui.tap(APP, on_button.0, on_button.1);
    r.settle();
    assert_eq!(r.ui.texts(APP), ["Leaderboard", "No scores yet.", "Back"]);
    assert_eq!(r.mgr.pump(), None, "nothing ticks");
    let back = button(&drawing(&r));
    assert!(back.y > b.y, "under the bigger plate");
    r.ui.tap(APP, i32::from(back.x) + 10, i32::from(back.y) + 10);
    r.settle();
    assert_eq!(r.ui.texts(APP), TOUCH_START);

    // A touch anywhere else starts a game with a flap; the sparrow is as
    // large as a finger, in the middle of the high field.
    r.ui.tap(APP, 700, 1200);
    r.settle();
    assert!(r.ui.texts(APP).is_empty());
    let ops = drawing(&r);
    assert_eq!(bird_x(&ops), m.bird_x);
    let y0 = bird_y(&ops);
    assert_eq!(y0, (m.field_h - m.bird_h) / 2);
    assert!(r.mgr.pump().expect("the game's timer") <= Duration::from_millis(m.tick_ms));
    // It rises, and falls; a touch where the button was is a flap now,
    // and so is a finger that slips as it touches, once.
    let rises = |r: &mut Rig| {
        for _ in 0..12 {
            next_tick(r);
        }
        let before = bird_y(&drawing(r));
        next_tick(r);
        assert!(bird_y(&drawing(r)) > before, "falling");
        next_tick(r);
        bird_y(&drawing(r))
    };
    let falling = rises(&mut r);
    r.ui.tap(APP, on_button.0, on_button.1);
    r.settle();
    next_tick(&mut r);
    assert!(bird_y(&drawing(&r)) < falling, "a flap");
    assert!(r.ui.texts(APP).is_empty(), "no leaderboard in a game");
    let falling = rises(&mut r);
    r.ui.drag(APP, (400, 600), (430, 640));
    r.settle();
    next_tick(&mut r);
    assert!(bird_y(&drawing(&r)) < falling, "a flap");

    // Left alone, it falls to the floor: the game is over, and the
    // button is back.
    while !game_over(&r) {
        next_tick(&mut r);
    }
    assert_eq!(r.ui.texts(APP), ["Game over", "Score 0   Best 0", "Touch to play again.", "Leaderboard"]);
    assert_eq!(button(&drawing(&r)), b);
    // A touch after a moment is the next game.
    std::thread::sleep(Duration::from_millis(750));
    r.ui.tap(APP, 400, 300);
    r.settle();
    assert!(r.ui.texts(APP).is_empty());
    assert!(r.mgr.pump().is_some(), "the game's timer runs");
}

#[test]
fn drawn_on_the_touchscreen() {
    let model = Model::D80;
    let m = measures(model);
    let mut p = Phone::new(model);
    p.mgr.launch(APP).unwrap();
    p.settle();
    assert_eq!(p.ui.screen_name(), "app:desksparrow");
    p.shot("1-start");
    assert!(p.cut().is_empty(), "{:?}", p.cut());
    let start = p.pixels();
    // The whole display: the bottom rows are the floor, side to side.
    let floor = start[1279 * 800];
    assert!(start[1278 * 800..].iter().all(|c| *c == floor));
    // The phone's back button is in its corner, over the game.
    let back = p.ui.canvas().hit_rect(opendeskphone_ui::Hit::Key(Key::Cancel)).expect("a back button");
    assert_eq!((back.x, back.y, back.w, back.h), (0, 0, 96, 96));

    // The button under the plate: the leaderboard, and back.
    let b = button(&p.ops());
    let touch = |p: &mut Phone, x: i16, y: i16| {
        p.sim.touch_tap(x as u16, y as u16);
        p.settle();
    };
    touch(&mut p, b.x + 20, b.y + 20);
    p.shot("4-leaderboard");
    assert_ne!(p.pixels(), start);
    assert!(p.cut().is_empty(), "{:?}", p.cut());
    let back_button = button(&p.ops());
    touch(&mut p, back_button.x + 20, back_button.y + 20);
    assert_eq!(p.pixels(), start);

    // A finger on the display starts the game and flies the sparrow
    // through two openings.
    touch(&mut p, 400, 1000);
    assert!(p.mgr.pump().is_some(), "the game's timer runs");
    let mut player = Player::new(m);
    let (mut shot, mut score, mut ahead_before) = (false, 0, None);
    while score < 2 {
        assert!(p.next_tick() <= 1);
        let ops = p.ops();
        if player.flaps(&ops) {
            touch(&mut p, 400, 1000);
        }
        let ahead = openings(&ops, m).iter().map(|o| o.0).find(|x| x + m.pipe_w as i16 > m.bird_x);
        if let (Some(before), Some(now)) = (ahead_before, ahead)
            && now > before
        {
            score += 1;
        }
        ahead_before = ahead;
        if !shot && score == 1 && ahead.is_some_and(|x| x <= m.bird_x + m.bird_w / 2) {
            p.shot("2-flying");
            shot = true;
        }
    }
    assert!(shot);
    while p.mgr.pump().is_some() {
        p.next_tick();
    }
    p.settle();
    p.shot("3-game-over");
    assert!(p.cut().is_empty(), "{:?}", p.cut());
    assert!(p.count(0xBC_864E) > 400, "a brown sparrow, twice the size");

    // The back button leaves the game: the app has stopped.
    touch(&mut p, 48, 48);
    assert_ne!(p.ui.screen_name(), "app:desksparrow");
    assert!(!p.mgr.is_running(APP));
}

// ------------------------------------------------------ the leaderboard

/// Times at which the tests' games end, in seconds since the Unix epoch,
/// and as the phone writes them (on UTC, with AM and PM).
const T0: (u64, &str) = (1_794_865_440, "Nov 16, 2026 9:44 PM");
const T1: (u64, &str) = (1_794_865_500, "Nov 16, 2026 9:45 PM");
const T2: (u64, &str) = (1_794_865_920, "Nov 16, 2026 9:52 PM");
const T3: (u64, &str) = (1_794_866_580, "Nov 16, 2026 10:03 PM");

/// A phone's wall clock, which a test sets.
type Clock = Arc<std::sync::atomic::AtomicU64>;

fn clock(unix: u64) -> Clock {
    Arc::new(unix.into())
}

fn set_clock(clock: &Clock, unix: u64) {
    clock.store(unix, std::sync::atomic::Ordering::Relaxed);
}

/// A phone's configuration with `clock` for its wall clock.
fn on_clock(clock: &Clock) -> FakeConfig {
    FakeConfig {
        wall_time: Some(clock.clone()),
        ..FakeConfig::default()
    }
}

/// A phone whose PBX store of app data is `app_data`, as `Rig::on_pbx`
/// makes one, with `clock` for its wall clock.
fn on_pbx_with(app_data: FakeStore, clock: &Clock) -> Rig {
    Rig::with_app_data(staged(), on_clock(clock), Model::D60, common::phone_budget(), app_data)
}

/// The leaderboard's value on the PBX, as text.
fn timed_on_pbx(store: &FakeStore) -> Option<String> {
    store
        .value(APP, StoreScope::Shared, "leaderboard")
        .map(|v| String::from_utf8(v).unwrap())
}

/// The rows of a leaderboard's value without the dates and times that
/// tell them apart: a line for each, of the score, the user's id, and the
/// name.
fn untimed(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.splitn(4, '\t').take(3).collect();
            format!("{}\n", fields.join("\t"))
        })
        .collect()
}

/// The leaderboard's value on the PBX, as text, without the dates and
/// times.
fn on_pbx(store: &FakeStore) -> Option<String> {
    timed_on_pbx(store).map(|v| untimed(&v))
}

/// Writes the leaderboard as the phone of `store` would.
fn write_board(store: &FakeStore, value: &str, expect: Option<u64>) -> StoreReply {
    store.run(StoreOp::Set {
        app: APP.into(),
        scope: StoreScope::Shared,
        key: "leaderboard".into(),
        value: value.as_bytes().to_vec(),
        expect,
    })
}

/// The value of one of the game's keys on the phone.
fn file(r: &Rig, name: &str) -> Option<String> {
    String::from_utf8(local_value(&r.nvdata.join("appdata").join(APP), name)?).ok()
}

/// The scores that wait on the phone to be written to the PBX, without
/// their dates and times.
fn waiting(r: &Rig) -> Option<String> {
    file(r, "waiting").map(|v| untimed(&v))
}

/// Plays a game from the start screen, the leaderboard, or the end of a
/// game: through `points` openings, and then to the floor or a pipe.
/// Returns the score.
fn play(r: &mut Rig, points: u32) -> u32 {
    let m = measures(Model::D60);
    // A key just after a game is that game's.
    if game_over(r) || !r.ui.shows(APP, "Desk Sparrow") {
        std::thread::sleep(Duration::from_millis(750));
    }
    r.key(APP, Key::Select);
    assert!(r.ui.texts(APP).is_empty(), "a game");
    let mut player = Player::new(m);
    let mut passed = 0;
    let mut ahead_before = None;
    let until = Instant::now() + Duration::from_secs(60);
    while passed < points {
        assert!(Instant::now() < until);
        next_tick(r);
        assert!(!game_over(r), "the player crashed");
        let ops = drawing(r);
        if player.flaps(&ops) {
            r.key(APP, Key::Select);
        }
        let ahead = openings(&ops, m)
            .iter()
            .filter(|(x, _, _)| x + m.pipe_w as i16 > m.bird_x)
            .map(|(x, _, _)| *x)
            .next();
        if let (Some(before), Some(now)) = (ahead_before, ahead)
            && now > before
        {
            passed += 1;
        }
        ahead_before = ahead;
    }
    while !game_over(r) {
        next_tick(r);
    }
    r.settle();
    let line = r.ui.texts(APP)[1].clone();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[test]
fn down_shows_the_leaderboard_between_games() {
    let mut r = Rig::new(FakeConfig::default());
    r.launch(APP);
    let picture = |r: &Rig| format!("{:?}", drawing(r));
    let start = picture(&r);
    r.key(APP, Key::Down);
    assert_eq!(r.ui.texts(APP), ["Leaderboard", "No scores yet."]);
    assert!(r.ui.softkeys(APP).is_empty());
    assert!(r.ui.full_screen(APP));
    assert_eq!(r.mgr.pump(), None, "nothing ticks");
    // A bigger plate, with the sparrow on it.
    assert_ne!(picture(&r), start);
    // Down or Up: the start screen again.
    r.key(APP, Key::Down);
    assert_eq!(r.ui.texts(APP), START);
    assert_eq!(picture(&r), start);
    r.key(APP, Key::Up);
    assert!(r.ui.shows(APP, "Leaderboard"));
    r.key(APP, Key::Up);
    assert_eq!(r.ui.texts(APP), START);

    // Any key of the game starts one from the leaderboard too, and in a
    // game Down and Up do nothing.
    r.key(APP, Key::Down);
    r.key(APP, Key::Digit(8));
    assert!(r.ui.texts(APP).is_empty());
    assert!(r.mgr.pump().is_some());
    next_tick(&mut r);
    let draws = r.ui.draw_count(APP);
    r.key(APP, Key::Down);
    r.key(APP, Key::Up);
    assert!(r.ui.texts(APP).is_empty());
    assert_eq!(r.ui.draw_count(APP), draws);
    while !game_over(&r) {
        next_tick(&mut r);
    }
    // After the game: the leaderboard over the end of the game, and back.
    // A score of 0 is not on it.
    r.key(APP, Key::Down);
    assert_eq!(r.ui.texts(APP), ["Leaderboard", "No scores yet."]);
    r.key(APP, Key::Down);
    assert_eq!(
        r.ui.texts(APP),
        ["Game over", "Score 0   Best 0", BOARD_HINT]
    );
    assert_eq!(on_pbx(&r.app_data), None);
    // Cancel leaves from the leaderboard as from anywhere.
    r.key(APP, Key::Up);
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
}

#[test]
fn a_score_is_on_every_phones_leaderboard_with_who_set_it() {
    let pbx = FakeStore::default();
    let now = clock(T1.0);
    let mut alice = on_pbx_with(pbx.phone("101", "Alice"), &now);
    let mut bob = on_pbx_with(pbx.phone("102", "Bob"), &now);
    // Bob looks at the leaderboard while Alice plays.
    bob.launch(APP);
    bob.key(APP, Key::Down);
    assert_eq!(bob.ui.texts(APP), ["Leaderboard", "No scores yet."]);
    alice.launch(APP);
    assert_eq!(play(&mut alice, 1), 1);
    assert_eq!(on_pbx(&pbx).as_deref(), Some("1\t101\tAlice\n"));
    assert_eq!(waiting(&alice).as_deref(), Some(""), "nothing waits");
    // Bob's phone is told, and shows it with the date and time of her
    // game; the row is not his.
    bob.settle();
    assert_eq!(bob.ui.texts(APP), ["Leaderboard", "1. Alice", T1.1, "1"]);
    assert!(bob.ui.highlighted(APP).is_empty());
    // Alice's shows the same, with her row marked.
    alice.key(APP, Key::Down);
    assert_eq!(alice.ui.texts(APP), ["Leaderboard", "1. Alice", T1.1, "1"]);
    assert_eq!(alice.ui.highlighted(APP), ["1. Alice", T1.1, "1"]);

    // Bob does as well, later: he is below her, who was there first.
    set_clock(&now, T2.0);
    assert_eq!(play(&mut bob, 1), 1);
    assert_eq!(
        on_pbx(&pbx).as_deref(),
        Some("1\t101\tAlice\n1\t102\tBob\n")
    );
    alice.settle();
    assert_eq!(
        alice.ui.texts(APP),
        ["Leaderboard", "1. Alice", T1.1, "1", "2. Bob", T2.1, "1"]
    );

    // Then he does better: every score is a row of its own, so the better
    // one is above her and his first stays, on both phones.
    set_clock(&now, T3.0);
    assert_eq!(play(&mut bob, 2), 2);
    assert_eq!(
        on_pbx(&pbx).as_deref(),
        Some("2\t102\tBob\n1\t101\tAlice\n1\t102\tBob\n")
    );
    alice.settle();
    let all = [
        "Leaderboard",
        "1. Bob",
        T3.1,
        "2",
        "2. Alice",
        T1.1,
        "1",
        "3. Bob",
        T2.1,
        "1",
    ];
    assert_eq!(alice.ui.texts(APP), all);
    assert_eq!(alice.ui.highlighted(APP), ["2. Alice", T1.1, "1"]);
    bob.key(APP, Key::Up);
    assert_eq!(bob.ui.texts(APP), all);
    assert_eq!(
        bob.ui.highlighted(APP),
        ["1. Bob", T3.1, "2", "3. Bob", T2.1, "1"]
    );

    // Each row has the date and time of its game, in seconds since the
    // Unix epoch, and each of Bob's games was written once.
    assert_eq!(
        timed_on_pbx(&pbx),
        Some(format!(
            "2\t102\tBob\t{}\n1\t101\tAlice\t{}\n1\t102\tBob\t{}\n",
            T3.0, T1.0, T2.0
        ))
    );
    let writes = bob
        .app_data
        .ops()
        .iter()
        .filter(|op| matches!(op, StoreOp::Set { .. }))
        .count();
    assert_eq!(writes, 2);

    // A phone that starts the game later reads it.
    let mut carol = Rig::on_pbx(pbx.phone("103", "Carol"));
    carol.launch(APP);
    carol.key(APP, Key::Down);
    assert_eq!(carol.ui.texts(APP), all);
    assert!(carol.ui.highlighted(APP).is_empty());
}

#[test]
fn two_who_finish_at_once_are_both_on_the_leaderboard() {
    // Alice's phone reads the leaderboard; then, before its write
    // arrives, Bob's phone writes.  A store that slips that write in:
    let pbx = FakeStore::default();
    let alice_side = pbx.phone("101", "Alice");
    struct Racing {
        inner: FakeStore,
        other: FakeStore,
        raced: std::sync::atomic::AtomicBool,
    }
    impl opendeskphone_apps::PbxStore for Racing {
        fn request(&self, op: StoreOp, done: opendeskphone_apps::Completion<StoreReply>) {
            let first_write = matches!(op, StoreOp::Set { .. })
                && !self.raced.swap(true, std::sync::atomic::Ordering::SeqCst);
            if first_write {
                let bob = format!("3\t102\tBob\t{}\n", T0.0);
                let r = write_board(&self.other, &bob, Some(0));
                assert!(matches!(r, StoreReply::Stored(_)));
            }
            self.inner.request(op, done);
        }
        fn user(&self) -> Option<opendeskphone_apps::StoreUser> {
            self.inner.user()
        }
        fn online(&self) -> bool {
            self.inner.online()
        }
        fn watch(&self, apps: &[String]) {
            self.inner.watch(apps)
        }
        fn subscribe(&self, f: Box<dyn Fn(opendeskphone_apps::StoreSignal) + Send + Sync>) {
            self.inner.subscribe(f)
        }
    }
    let racing = Racing {
        inner: alice_side.clone(),
        other: pbx.phone("102", "Bob"),
        raced: false.into(),
    };
    let mut alice = Rig::with_services(|mut services| {
        services.store = Arc::new(racing);
        services.config = Arc::new(on_clock(&clock(T1.0)));
        services
    });
    alice.launch(APP);
    assert_eq!(play(&mut alice, 1), 1);
    // Both are there: Alice's phone was shown Bob's entry and wrote over
    // that one.
    assert_eq!(
        on_pbx(&pbx).as_deref(),
        Some("3\t102\tBob\n1\t101\tAlice\n")
    );
    let writes: Vec<Option<u64>> = alice_side
        .ops()
        .into_iter()
        .filter_map(|op| match op {
            StoreOp::Set { expect, .. } => Some(expect),
            _ => None,
        })
        .collect();
    assert_eq!(
        writes.len(),
        2,
        "one refused write and one over the newer entry"
    );
    assert_eq!(writes[0], Some(0));
    assert!(writes[1].is_some_and(|v| v > 0));
    alice.key(APP, Key::Down);
    assert_eq!(
        alice.ui.texts(APP),
        ["Leaderboard", "1. Bob", T0.1, "3", "2. Alice", T1.1, "1"]
    );
    assert_eq!(waiting(&alice).as_deref(), Some(""));
}

#[test]
fn a_score_set_out_of_the_pbxs_reach_is_written_when_it_is_in_reach() {
    let pbx = FakeStore::default();
    // Bob's row was written before rows had a date and time: it is shown
    // without one.
    write_board(&pbx.phone("102", "Bob"), "3\t102\tBob\n", None);
    let now = clock(T1.0);
    let mut alice = on_pbx_with(pbx.phone("101", "Alice"), &now);
    // She has seen the leaderboard; then the PBX is gone.
    alice.launch(APP);
    alice.app_data.set_online(false);
    assert_eq!(play(&mut alice, 1), 1);
    // The score waits on the phone, and her leaderboard has it.
    assert_eq!(waiting(&alice).as_deref(), Some("1\t101\tAlice\n"));
    alice.key(APP, Key::Down);
    let both = ["1. Bob", "", "3", "2. Alice", T1.1, "1"];
    assert_eq!(alice.ui.texts(APP)[0], "Leaderboard (offline)");
    assert_eq!(alice.ui.texts(APP)[1..], both);
    // Meanwhile Carol's phone, which can reach the PBX, writes.
    write_board(
        &pbx.phone("103", "Carol"),
        &format!("3\t102\tBob\n2\t103\tCarol\t{}\n", T2.0),
        None,
    );
    // The PBX is back while the game is on screen: the score is written
    // into the leaderboard as it is now.
    alice.app_data.set_online(true);
    alice.settle();
    assert_eq!(
        on_pbx(&pbx).as_deref(),
        Some("3\t102\tBob\n2\t103\tCarol\n1\t101\tAlice\n")
    );
    assert_eq!(
        alice.ui.texts(APP),
        [
            "Leaderboard",
            "1. Bob",
            "",
            "3",
            "2. Carol",
            T2.1,
            "2",
            "3. Alice",
            T1.1,
            "1"
        ]
    );
    assert_eq!(waiting(&alice).as_deref(), Some(""));

    // Dan's phone has never reached the PBX since it started, and he
    // leaves the game before it can: the score is written the next time
    // the game starts, with the date and time of the game.
    set_clock(&now, T3.0);
    let mut dan = on_pbx_with(pbx.phone("104", "Dan"), &now);
    dan.app_data.set_online(false);
    dan.launch(APP);
    dan.key(APP, Key::Down);
    assert_eq!(
        dan.ui.texts(APP),
        ["Leaderboard (offline)", "No scores yet."]
    );
    assert_eq!(play(&mut dan, 2), 2);
    dan.key(APP, Key::Cancel);
    assert!(!dan.mgr.is_running(APP));
    dan.app_data.set_online(true);
    dan.settle();
    assert_eq!(waiting(&dan).as_deref(), Some("2\t104\tDan\n"));
    set_clock(&now, T3.0 + 3600);
    dan.launch(APP);
    assert_eq!(
        timed_on_pbx(&pbx),
        Some(format!(
            "3\t102\tBob\t\n2\t103\tCarol\t{}\n2\t104\tDan\t{}\n1\t101\tAlice\t{}\n",
            T2.0, T3.0, T1.0
        ))
    );
    assert_eq!(waiting(&dan).as_deref(), Some(""));
}

#[test]
fn a_phone_without_a_pbx_store_plays_and_keeps_its_scores() {
    let now = clock(T1.0);
    let mut r = Rig::with_services(|mut services| {
        services.store = Arc::new(opendeskphone_apps::NoStore);
        services.config = Arc::new(on_clock(&now));
        services
    });
    r.launch(APP);
    assert_eq!(r.ui.texts(APP), START);
    assert_eq!(play(&mut r, 1), 1);
    assert_eq!(
        r.ui.texts(APP),
        ["Game over", "Score 1   Best 1", BOARD_HINT]
    );
    assert_eq!(file(&r, "best").as_deref(), Some("1"));
    // The phone does not know its user: the score waits without one.
    assert_eq!(waiting(&r).as_deref(), Some("1\t\t\n"));
    r.key(APP, Key::Down);
    assert_eq!(
        r.ui.texts(APP),
        ["Leaderboard (offline)", "1. This phone", T1.1, "1"]
    );
    assert_eq!(r.ui.highlighted(APP), ["1. This phone", T1.1, "1"]);
    // The next game is as playable, its score is a row of its own, and a
    // new run of the game still has both.
    set_clock(&now, T2.0);
    assert_eq!(play(&mut r, 1), 1);
    assert_eq!(waiting(&r).as_deref(), Some("1\t\t\n1\t\t\n"));
    r.key(APP, Key::Cancel);
    r.launch(APP);
    r.key(APP, Key::Down);
    let both = ["1. This phone", T1.1, "1", "2. This phone", T2.1, "1"];
    assert_eq!(r.ui.texts(APP)[0], "Leaderboard (offline)");
    assert_eq!(r.ui.texts(APP)[1..], both);
    assert_eq!(r.ui.highlighted(APP), both);

    // A phone with such scores that then has a PBX with a store: the
    // scores are its user's, with their dates and times.  (The first waits
    // as it was written before rows had them.)
    let pbx = FakeStore::default();
    let mut alice = Rig::on_pbx(pbx.phone("101", "Alice"));
    let dir = alice.nvdata.join("appdata").join(APP);
    std::fs::create_dir_all(&dir).unwrap();
    let waits = format!("4\t\t\n2\t\t\t{}\n", T1.0);
    std::fs::write(dir.join("waiting"), waits).unwrap();
    alice.launch(APP);
    assert_eq!(
        timed_on_pbx(&pbx),
        Some(format!("4\t101\tAlice\t\n2\t101\tAlice\t{}\n", T1.0))
    );
    assert_eq!(waiting(&alice).as_deref(), Some(""));
}

#[test]
fn a_score_written_once_is_not_written_again() {
    // The PBX does the phone's first write, but its answer is lost: the
    // phone is told that the PBX was out of reach.  A store that does
    // that:
    struct Lossy {
        inner: FakeStore,
        lost: std::sync::atomic::AtomicBool,
    }
    impl opendeskphone_apps::PbxStore for Lossy {
        fn request(&self, op: StoreOp, done: opendeskphone_apps::Completion<StoreReply>) {
            let first_write = matches!(op, StoreOp::Set { .. })
                && !self.lost.swap(true, std::sync::atomic::Ordering::SeqCst);
            if first_write {
                self.inner.run(op);
                done.complete(StoreReply::Unreachable("no answer".into()));
            } else {
                self.inner.request(op, done);
            }
        }
        fn user(&self) -> Option<opendeskphone_apps::StoreUser> {
            self.inner.user()
        }
        fn online(&self) -> bool {
            self.inner.online()
        }
        fn watch(&self, apps: &[String]) {
            self.inner.watch(apps)
        }
        fn subscribe(&self, f: Box<dyn Fn(opendeskphone_apps::StoreSignal) + Send + Sync>) {
            self.inner.subscribe(f)
        }
    }
    let pbx = FakeStore::default();
    let alice_side = pbx.phone("101", "Alice");
    let lossy = Lossy {
        inner: alice_side.clone(),
        lost: false.into(),
    };
    let now = clock(T1.0);
    let mut alice = Rig::with_services(|mut services| {
        services.store = Arc::new(lossy);
        services.config = Arc::new(on_clock(&now));
        services
    });
    alice.launch(APP);
    assert_eq!(play(&mut alice, 1), 1);
    let once = format!("1\t101\tAlice\t{}\n", T1.0);
    assert_eq!(timed_on_pbx(&pbx).as_ref(), Some(&once));
    // The phone keeps the score to write, with its date and time.
    assert_eq!(file(&alice, "waiting").as_ref(), Some(&once));
    // The next time the game starts, later, the phone reads the
    // leaderboard, finds the score there, and does not write it again.
    alice.key(APP, Key::Cancel);
    set_clock(&now, T2.0);
    alice.launch(APP);
    assert_eq!(timed_on_pbx(&pbx).as_ref(), Some(&once));
    assert_eq!(waiting(&alice).as_deref(), Some(""));
    let writes = |store: &FakeStore| {
        store
            .ops()
            .iter()
            .filter(|op| matches!(op, StoreOp::Set { .. }))
            .count()
    };
    assert_eq!(writes(&alice_side), 1);
    // Another game of the same score is a row of its own.
    assert_eq!(play(&mut alice, 1), 1);
    assert_eq!(
        timed_on_pbx(&pbx),
        Some(format!("{once}1\t101\tAlice\t{}\n", T2.0))
    );
    assert_eq!(writes(&alice_side), 2);
}

/// The leaderboard on the simulated phone, with a picture of it.
fn leaderboard_on(model: Model) -> Phone {
    let pbx = FakeStore::default();
    // Among the dates and times, the widest there are in the small font
    // of the gray screens (Nov 10, 2026 12:40 PM) and in that of the
    // color ones (May 20, 2026 10:48 AM).
    write_board(
        &pbx,
        "23\t105\tPriya Raman\t1794314400\n\
         17\t102\tBob Castellanos-Whitfield\t1779274080\n\
         9\t101\tAlice Moreau\t1794863520\n\
         4\t108\tSam Okafor\t1793181900\n\
         2\t110\tWei Chen\t1788424200\n",
        None,
    );
    let mut p = Phone::on_pbx(model, &pbx.phone("101", "Alice Moreau"));
    p.mgr.launch(APP).unwrap();
    p.settle();
    let start = p.pixels();
    // Down reaches the game through the phone's keys.
    p.tap(Key::Down);
    assert_eq!(p.ui.screen_name(), "app:desksparrow");
    assert_ne!(p.pixels(), start);
    p.shot("4-leaderboard");
    // Every date and time and every score fits its column; a long name is
    // cut short on the gray screens.
    let cut: &[&str] = if model.is_color() {
        &[]
    } else {
        &["2. Bob Castellanos-Whitfield"]
    };
    assert_eq!(p.cut(), cut);
    // The whole display still: the floor is where the softkeys would be.
    let size = model.main_screen();
    let (w, h) = (size.width as usize, size.height as usize);
    let pixels = p.pixels();
    let floor = pixels[(h - 1) * w];
    assert!(pixels[(h - 2) * w..].iter().all(|p| *p == floor));
    p.tap(Key::Up);
    assert_eq!(p.pixels(), start);
    // With no scores, and out of the PBX's reach.
    let empty = FakeStore::default();
    empty.set_online(false);
    let mut q = Phone::on_pbx(model, &empty);
    q.mgr.launch(APP).unwrap();
    q.settle();
    q.tap(Key::Down);
    q.shot("5-leaderboard-empty-offline");
    p
}

#[test]
fn the_leaderboard_drawn_on_a_color_phone() {
    let mut p = leaderboard_on(Model::D60);
    // The user's row is in the skin's selection color, which nothing
    // else on the start screen has.
    assert_eq!(p.count(0x01_579B), 0);
    p.tap(Key::Down);
    assert!(p.count(0x01_579B) > 2000);
}

#[test]
fn the_leaderboard_drawn_on_the_gray_phones() {
    for model in [Model::D40, Model::D70] {
        let mut p = leaderboard_on(model);
        p.tap(Key::Down);
        let pixels = p.pixels();
        assert!(
            pixels
                .iter()
                .all(|c| [0x00_0000, 0x55_5555, 0xAA_AAAA, 0xFF_FFFF].contains(c))
        );
    }
}
