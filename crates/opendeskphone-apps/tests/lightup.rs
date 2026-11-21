//! Light Up, the puzzle: the app's life under the manager with the fake
//! screen (the keys, the screens, generation in turns, what is kept, the
//! time spent, and the tables of the best times with two phones of one
//! PBX, with a PBX out of reach, and with no store of app data on the
//! PBX).  The rules, the solver, the generator, the pictures, and the
//! tables themselves are tested in the crate `lightup-game`
//! (`apps/lightup/game`).

mod common;

use common::{Rig, staged};
use opendeskphone_apps::testutil::{FakeConfig, FakeStore, local_value};
use opendeskphone_apps::{DrawOp, StoreOp, StoreScope};
use opendeskphone_state::{Key, Model};
use lightup_game::generate::{Difficulty, Params, SIZES, generate};
use lightup_game::scores::{Kind, Table, Win};
use lightup_game::solver::{Outcome, Solve, Techniques};
use lightup_game::{Act, Game, Grid};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

const APP: &str = "lightup";
/// The grids the app allows a puzzle (`MAX_GRIDS` in the app).
const MAX_GRIDS: u32 = 400;
/// The texts beside the board while a puzzle is played, after the time
/// spent ([`beside`]): the two lines that say "Solved!" and the time when
/// it comes to that, and nothing else (the keys are a picture).
const PLAYING: [&str; 2] = ["", ""];

fn storage(nvdata: &Path) -> PathBuf {
    nvdata.join("appdata").join(APP)
}

/// The value of one of the app's keys on the phone, or "".
fn local(nvdata: &Path, key: &str) -> String {
    local_value(&storage(nvdata), key).map(|v| String::from_utf8(v).unwrap()).unwrap_or_default()
}

/// A rig whose app makes its puzzles from this seed on.
fn rig(model: Model, seed: u64) -> Rig {
    listen();
    let r = Rig::with_root(staged(), FakeConfig::default(), model);
    std::fs::create_dir_all(storage(&r.nvdata)).unwrap();
    std::fs::write(storage(&r.nvdata).join("seed"), seed.to_string()).unwrap();
    r
}

/// A rig on which the app finds this puzzle file.
fn rig_with(model: Model, puzzle: &str) -> Rig {
    let r = rig(model, 1);
    std::fs::write(storage(&r.nvdata).join("puzzle"), puzzle).unwrap();
    r
}

fn file(r: &Rig) -> String {
    local(&r.nvdata, "puzzle")
}

/// The grid and the cursor of a puzzle file.
fn parse(file: &str) -> (Grid, (usize, usize)) {
    let rows: Vec<&str> = file.lines().filter(|l| !l.contains('=')).collect();
    let cursor = file
        .lines()
        .find_map(|l| l.strip_prefix("cursor="))
        .expect("a cursor");
    let (x, y) = cursor.split_once(',').unwrap();
    (
        Grid::parse(&rows.join("\n")).expect("a grid"),
        (x.parse().unwrap(), y.parse().unwrap()),
    )
}

/// What the app has: something else takes the screen for a moment, which
/// makes the app write its puzzle down.
fn state(r: &mut Rig) -> (Grid, (usize, usize)) {
    r.ui.cover(APP, true);
    r.settle();
    r.ui.cover(APP, false);
    r.settle();
    parse(&file(r))
}

/// Whether a text is a time (`3:05`).
fn is_time(text: &str) -> bool {
    text.contains(':') && text.chars().all(|c| c.is_ascii_digit() || c == ':')
}

/// The time spent, as the playing screen shows it.
fn time_shown(r: &Rig) -> String {
    let texts = r.ui.texts(APP);
    assert!(texts.first().is_some_and(|t| is_time(t)), "{texts:?}");
    texts[0].clone()
}

/// The texts of the playing screen after the time spent, which is the
/// first of them and goes on.
fn beside(r: &Rig) -> Vec<String> {
    time_shown(r);
    r.ui.texts(APP).split_off(1)
}

fn generating(r: &Rig) -> bool {
    r.ui.texts(APP).iter().any(|t| t.starts_with("Generating"))
}

/// Lets the app work until its puzzle is made.
fn wait(r: &mut Rig) {
    let start = Instant::now();
    while generating(r) {
        assert!(
            start.elapsed() < Duration::from_secs(300),
            "still generating"
        );
        assert!(r.mgr.is_running(APP), "the app was stopped");
        std::thread::sleep(Duration::from_millis(1));
        r.mgr.pump();
    }
    r.settle();
}

/// Starts the puzzle of a row of the list of sizes and difficulties.
fn choose(r: &mut Rig, side: usize, difficulty: Difficulty) {
    let row = SIZES.iter().position(|s| *s == side).unwrap() * 3 + difficulty as usize;
    r.ui.highlight(APP, row as u32);
    r.softkey(APP, "Play");
}

fn native(side: usize, difficulty: Difficulty, seed: u64) -> Grid {
    generate(Params::preset(side, difficulty), seed, MAX_GRIDS).expect("a puzzle")
}

fn solution(puzzle: &Grid) -> Grid {
    let mut s = Solve::new(puzzle, Techniques::HARD);
    assert_eq!(s.run(), Outcome::Unique);
    s.solution().unwrap().clone()
}

/// The keys that take the cursor from one cell to another.
fn path(from: (usize, usize), to: (usize, usize), digits: bool) -> Vec<Key> {
    let mut keys = Vec::new();
    let (h, v) = (to.0 as i32 - from.0 as i32, to.1 as i32 - from.1 as i32);
    for _ in 0..h.abs() {
        keys.push(match (h > 0, digits) {
            (true, false) => Key::Right,
            (true, true) => Key::Digit(6),
            (false, false) => Key::Left,
            (false, true) => Key::Digit(4),
        });
    }
    for _ in 0..v.abs() {
        keys.push(match (v > 0, digits) {
            (true, false) => Key::Down,
            (true, true) => Key::Digit(5),
            (false, false) => Key::Up,
            (false, true) => Key::Digit(2),
        });
    }
    keys
}

fn keys(r: &mut Rig, keys: &[Key]) {
    for k in keys {
        r.key(APP, *k);
    }
}

/// What the app's canvases show, in the order they were made.
fn drawn(r: &Rig) -> Vec<String> {
    r.ui.drawings(APP)
        .into_iter()
        .map(|(_, ops)| format!("{ops:?}"))
        .collect()
}

#[test]
fn a_puzzle_from_new_to_solved_with_every_key() {
    let mut r = rig(Model::D60, 11);
    r.launch(APP);
    assert_eq!(r.ui.top().as_deref(), Some(APP));
    // Nothing is kept yet: the sizes and difficulties come first.
    assert_eq!(r.ui.texts(APP), ["New puzzle"]);
    r.softkey(APP, "Play");
    wait(&mut r);
    assert_eq!(beside(&r), PLAYING);

    // The puzzle is the one the seed gives, and it is written down.
    let puzzle = native(7, Difficulty::Easy, 11);
    assert_eq!(parse(&file(&r)), (puzzle.clone(), (0, 0)));
    let mut game = Game::new(puzzle.clone());

    // The arrows and 2, 4, 5, 6 move the cursor.
    for (key, dx, dy) in [
        (Key::Right, 1, 0),
        (Key::Digit(6), 1, 0),
        (Key::Down, 0, 1),
        (Key::Digit(5), 0, 1),
        (Key::Left, -1, 0),
        (Key::Digit(4), -1, 0),
        (Key::Up, 0, -1),
        (Key::Digit(2), 0, -1),
    ] {
        r.key(APP, key);
        assert!(game.move_cursor(dx, dy));
        assert_eq!(
            state(&mut r),
            (game.grid().clone(), game.cursor()),
            "{key:?}"
        );
    }

    // 1 and Select put a light, or take it off a cell that has one; 3
    // puts a no-light mark, or takes it off.  7, 9, 0, star, and pound do
    // nothing.  Each key draws again what it changed, and nothing else.
    let white = (0..puzzle.cells())
        .find(|i| !puzzle.is_black(*i))
        .unwrap();
    let at = |r: &mut Rig, game: &mut Game, i: usize| {
        let to = (i % 7, i / 7);
        keys(r, &path(game.cursor(), to, i.is_multiple_of(2)));
        game.set_cursor(to.0, to.1);
    };
    at(&mut r, &mut game, white);
    for (key, act) in [
        (Key::Digit(1), Some(Act::Light)),
        (Key::Digit(1), Some(Act::Light)),
        (Key::Digit(3), Some(Act::Mark)),
        (Key::Select, Some(Act::Light)),
        (Key::Digit(7), None),
        (Key::Digit(9), None),
        (Key::Digit(0), None),
        (Key::Star, None),
        (Key::Pound, None),
    ] {
        let (draws, before) = (r.ui.draw_count(APP), drawn(&r));
        r.key(APP, key);
        let changed = act.is_some_and(|a| game.toggle(a));
        assert_eq!(
            state(&mut r),
            (game.grid().clone(), game.cursor()),
            "{key:?}"
        );
        let differ = drawn(&r)
            .iter()
            .zip(&before)
            .filter(|(now, was)| now != was)
            .count() as u32;
        assert_eq!(differ > 0, changed, "{key:?}");
        assert_eq!(r.ui.draw_count(APP) - draws, differ, "{key:?}");
    }
    // What is drawn is what the app draws for the kept puzzle when it
    // starts again: nothing that changed was left as it was.
    let shown = drawn(&r);
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
    r.launch(APP);
    assert_eq!(drawn(&r), shown);

    // Now the solution: the light goes if it is wrong, and every light of
    // the solution is put, with 1 and with Select in turn.
    let solved = solution(&puzzle);
    if !solved.has_light(white) {
        at(&mut r, &mut game, white);
        r.key(APP, Key::Digit(1));
        game.act(Act::Clear);
    }
    let lights: Vec<usize> = (0..puzzle.cells())
        .filter(|i| solved.has_light(*i) && !game.grid().has_light(*i))
        .collect();
    for (n, cell) in lights.iter().enumerate() {
        assert_eq!(beside(&r), PLAYING);
        at(&mut r, &mut game, *cell);
        r.key(
            APP,
            if n.is_multiple_of(2) {
                Key::Digit(1)
            } else {
                Key::Select
            },
        );
        game.act(Act::Light);
    }
    assert!(game.solved());
    // Solved: said so, with the time taken; the solved puzzle is written
    // down at once.
    let texts = r.ui.texts(APP);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], "Solved!");
    let seconds: u32 = texts[1]
        .strip_prefix("Time 0:")
        .expect(&texts[1])
        .parse()
        .unwrap();
    assert!(seconds < 60);
    assert_eq!(parse(&file(&r)).0, *game.grid());
    // The keys do nothing any more.
    keys(
        &mut r,
        &[
            Key::Left,
            Key::Digit(1),
            Key::Digit(3),
            Key::Digit(5),
            Key::Select,
            Key::Digit(8),
        ],
    );
    assert_eq!(r.ui.texts(APP), texts);
    assert_eq!(state(&mut r).0, *game.grid());

    // New: the list again, at the size and difficulty played, and Back
    // (the softkey or Cancel) is the solved puzzle as it was.
    r.softkey(APP, "New");
    r.softkey(APP, "Back");
    assert_eq!(r.ui.texts(APP), texts);
    r.softkey(APP, "New");
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP), texts);
    // Another puzzle, chosen with Select on its row.  It is the first the
    // app makes since it was started again, so it is made from the seed
    // in the file.
    r.softkey(APP, "New");
    r.key(APP, Key::Down);
    r.key(APP, Key::Select);
    wait(&mut r);
    assert_eq!(beside(&r), PLAYING);
    assert_eq!(
        parse(&file(&r)),
        (native(7, Difficulty::Medium, 11), (0, 0))
    );

    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.top(), None);
    assert!(!r.mgr.is_running(APP), "Cancel stops the app");
}

/// A 7x7 puzzle with lights that see each other and a 1 with two lights.
const WRONG: &str = "size=7\ndifficulty=hard\nelapsed=125000\ncursor=3,1\n\
    ..#....\n\
    L.xL...\n\
    .L1L...\n\
    ...#...\n\
    ...0...\n\
    #......\n\
    ......2\n";

#[test]
fn it_is_drawn_in_color_or_in_the_gray_screens_black_and_white() {
    // The fake screen's foreground and background colors.
    let ink = |c: &opendeskphone_apps::Color| {
        [(32, 32, 32), (16, 16, 16)].contains(&(c.r, c.g, c.b))
    };
    for model in [Model::D60, Model::D40] {
        let mut r = rig_with(model, WRONG);
        r.launch(APP);
        let colors: Vec<opendeskphone_apps::Color> = r
            .ui
            .drawings(APP)
            .into_iter()
            .flat_map(|(_, ops)| ops)
            .map(|op| match op {
                DrawOp::Fill((_, color)) => color,
                DrawOp::Bitmap(b) => b.color,
                DrawOp::Text(t) => t.color,
            })
            .collect();
        assert!(!colors.is_empty(), "{model:?}");
        // The game's own colors on the color display; the screen's two,
        // and no others, on the gray one.
        if model.is_color() {
            assert!(colors.iter().any(|c| !ink(c)), "{model:?}");
        } else {
            assert!(colors.iter().all(ink), "{model:?}");
        }
    }
}

#[test]
fn the_puzzle_is_kept_and_a_call_does_not_end_it() {
    let mut r = rig_with(Model::D60, WRONG);
    r.launch(APP);
    keys(&mut r, &[Key::Right, Key::Digit(3)]);
    let shown = drawn(&r);
    let kept = WRONG
        .replace("L.xL...", "L.xLx..")
        .replace("cursor=3,1", "cursor=4,1");
    let rows = |f: &str| {
        f.lines()
            .filter(|l| !l.starts_with("elapsed=") && !l.starts_with("undo="))
            .map(String::from)
            .collect::<Vec<_>>()
    };
    let elapsed = |f: &str| {
        f.lines()
            .find_map(|l| l.strip_prefix("elapsed="))
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    // Not written at every key.
    assert_eq!(file(&r), WRONG);

    // A call's screen goes over the puzzle.  The puzzle, the marks, the
    // cursor, and the time so far are written down, and all is there
    // when the call is over.
    r.ui.cover(APP, true);
    r.settle();
    assert!(r.mgr.is_running(APP));
    assert_eq!(rows(&file(&r)), rows(&kept));
    let before = elapsed(&file(&r));
    assert!((125_000..135_000).contains(&before), "{before}");
    r.ui.cover(APP, false);
    r.settle();
    assert_eq!(drawn(&r), shown);
    assert_eq!(beside(&r), PLAYING);
    r.key(APP, Key::Digit(3));
    assert_ne!(drawn(&r), shown);

    // Leaving with Cancel stops the app; the next time the puzzle is
    // where it was.
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.top(), None);
    assert!(!r.mgr.is_running(APP));
    let left = WRONG.replace("cursor=3,1", "cursor=4,1");
    assert_eq!(rows(&file(&r)), rows(&left));
    r.launch(APP);
    assert_eq!(beside(&r), PLAYING);
    assert_eq!(state(&mut r), parse(&left));
    r.key(APP, Key::Cancel);

    // A solved puzzle is shown solved, with its time.
    let solved = native(10, Difficulty::Medium, 5);
    let mut done = solution(&solved);
    for i in 0..done.cells() {
        done.set_mark(i, false);
    }
    let puzzle = storage(&r.nvdata).join("puzzle");
    std::fs::write(
        &puzzle,
        format!(
            "size=10\ndifficulty=medium\nelapsed=3725999\ncursor=2,2\n{}",
            done.to_text()
        ),
    )
    .unwrap();
    r.launch(APP);
    assert_eq!(r.ui.texts(APP), ["Solved!", "Time 1:02:05"]);
    r.softkey(APP, "Exit");

    // A puzzle file that makes no sense is no puzzle: the list, at the
    // size and difficulty the file names if it names any.
    for nonsense in [
        "",
        "\u{0}\u{1}",
        "size=10\ndifficulty=hard\n",
        "size=9\ndifficulty=silly\nelapsed=x\ncursor=1\n...\n...\n...\n",
        "size=7\ndifficulty=easy\nelapsed=5\ncursor=0,0\n.......\n...\n",
        "size=7\ndifficulty=easy\nelapsed=5\ncursor=0,0\n..........\n",
        "size=7\ndifficulty=easy\ncursor=0,0\n.......\n.......\n...?...\n.......\n.......\n.......\n.......\n",
    ] {
        std::fs::write(&puzzle, nonsense).unwrap();
        r.launch(APP);
        assert_eq!(r.ui.texts(APP), ["New puzzle"], "{nonsense:?}");
        r.softkey(APP, "Exit");
        assert!(!r.mgr.is_running(APP));
    }
    // The cursor of a file is put inside the grid.
    std::fs::write(&puzzle, WRONG.replace("cursor=3,1", "cursor=99,x")).unwrap();
    r.launch(APP);
    assert_eq!(state(&mut r).1, (6, 0));
}

#[test]
fn undo_takes_changes_back_one_by_one_and_is_kept() {
    let (grid, cursor) = parse(WRONG);
    let mut game = Game::new(grid);
    game.set_cursor(cursor.0, cursor.1);
    let start = game.clone();
    let mut r = rig_with(Model::D60, WRONG);
    r.launch(APP);
    // Nothing to take back: nothing happens.
    r.softkey(APP, "Undo");
    assert_eq!(state(&mut r), (start.grid().clone(), start.cursor()));

    // Clearing the light under the cursor, and Undo: the light is back.
    r.key(APP, Key::Digit(1));
    assert!(game.act(Act::Clear));
    assert_eq!(state(&mut r), (game.grid().clone(), game.cursor()));
    r.softkey(APP, "Undo");
    assert!(game.undo());
    assert_eq!(game, start);
    assert_eq!(state(&mut r), (start.grid().clone(), start.cursor()));

    // Each kind of change, several on one cell and some elsewhere; the
    // cursor moves in between are no steps.
    keys(
        &mut r,
        &[
            Key::Right,
            Key::Down,
            Key::Down,
            Key::Digit(3),
            Key::Digit(1),
            Key::Digit(1),
            Key::Digit(3),
        ],
    );
    game.move_cursor(1, 0);
    game.move_cursor(0, 1);
    game.move_cursor(0, 1);
    for act in [Act::Mark, Act::Light, Act::Clear, Act::Mark] {
        assert!(game.act(act));
    }
    keys(
        &mut r,
        &[Key::Left, Key::Left, Key::Left, Key::Digit(1), Key::Up],
    );
    for _ in 0..3 {
        game.move_cursor(-1, 0);
    }
    assert!(game.act(Act::Light));
    game.move_cursor(0, -1);
    assert_eq!(state(&mut r), (game.grid().clone(), game.cursor()));

    // Two steps back now; each puts the cursor on its cell.
    for _ in 0..2 {
        r.softkey(APP, "Undo");
        assert!(game.undo());
        assert_eq!(state(&mut r), (game.grid().clone(), game.cursor()));
    }
    assert_eq!(game.cursor(), (4, 3));
    // The rest after the app was left and opened again.
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
    r.launch(APP);
    while game.undo() {
        r.softkey(APP, "Undo");
        assert_eq!(state(&mut r), (game.grid().clone(), game.cursor()));
    }
    assert_eq!(game.grid(), start.grid());
    // Nothing is left to take back.
    r.softkey(APP, "Undo");
    assert_eq!(state(&mut r), (start.grid().clone(), (4, 3)));

    // Changes kept in a file that makes no sense are no changes.
    r.key(APP, Key::Cancel);
    std::fs::write(
        storage(&r.nvdata).join("puzzle"),
        WRONG.replace("cursor=3,1\n", "cursor=3,1\nundo=7.999L\n"),
    )
    .unwrap();
    r.launch(APP);
    r.softkey(APP, "Undo");
    assert_eq!(state(&mut r), (start.grid().clone(), start.cursor()));

    // Key 8 does what the numbers settle, and Undo takes all of that back
    // at once.
    r.key(APP, Key::Digit(8));
    let mut settled = start.clone();
    assert!(settled.auto());
    assert_eq!(state(&mut r), (settled.grid().clone(), settled.cursor()));
    r.softkey(APP, "Undo");
    assert_eq!(state(&mut r).0, *start.grid());
}

#[test]
fn help_shows_how_to_play_and_leaves_the_puzzle_as_it_was() {
    use lightup_game::help::PAGES;
    // The page on show, by its text.
    let page = |r: &Rig, n: usize| {
        assert_eq!(r.ui.texts(APP)[1], PAGES[n].text, "page {n}");
    };
    let mut r = rig_with(Model::D60, WRONG);
    r.launch(APP);
    keys(&mut r, &[Key::Right, Key::Digit(3)]);
    let (shown, kept) = (drawn(&r), state(&mut r));

    r.softkey(APP, "Help");
    page(&r, 0);
    // Next, Select, and the arrows turn the pages, around the ends.
    r.softkey(APP, "Next");
    page(&r, 1);
    r.key(APP, Key::Select);
    page(&r, 2);
    r.key(APP, Key::Right);
    page(&r, 3);
    r.softkey(APP, "Next");
    page(&r, 4);
    r.key(APP, Key::Down);
    page(&r, 0);
    r.key(APP, Key::Left);
    page(&r, 4);
    r.key(APP, Key::Up);
    page(&r, 3);
    // The keys of the puzzle do nothing here.
    keys(
        &mut r,
        &[Key::Digit(5), Key::Digit(1), Key::Digit(3), Key::Digit(6)],
    );
    page(&r, 3);
    r.softkey(APP, "Back");
    assert_eq!(beside(&r), PLAYING);
    assert_eq!(drawn(&r), shown);
    assert_eq!(state(&mut r), kept);
    // The change before the help can still be taken back.
    r.softkey(APP, "Undo");
    assert_ne!(drawn(&r), shown);
    // Cancel leaves the help too, and something over the help (a call)
    // leaves it where it was.
    let shown = drawn(&r);
    r.softkey(APP, "Help");
    r.softkey(APP, "Next");
    r.ui.cover(APP, true);
    r.settle();
    r.ui.cover(APP, false);
    r.settle();
    page(&r, 1);
    r.key(APP, Key::Cancel);
    assert!(r.mgr.is_running(APP));
    assert_eq!(drawn(&r), shown);
    r.key(APP, Key::Cancel);

    // From the list of sizes, before any puzzle: back to the list.
    let mut r = rig(Model::D60, 3);
    r.launch(APP);
    r.softkey(APP, "Help");
    page(&r, 0);
    r.softkey(APP, "Back");
    assert_eq!(r.ui.texts(APP), ["New puzzle"]);
    r.softkey(APP, "Help");
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.texts(APP), ["New puzzle"]);
}

#[test]
fn generating_goes_on_in_turns_and_waits_while_a_call_has_the_screen() {
    let mut r = rig_with(Model::D60, WRONG);
    r.launch(APP);
    r.softkey(APP, "New");
    // The list is at the puzzle's size and difficulty.
    r.key(APP, Key::Select);
    assert_eq!(r.ui.texts(APP), ["7x7 Hard", "Generating\u{2026}"]);
    wait(&mut r);
    assert_eq!(parse(&file(&r)).0, native(7, Difficulty::Hard, 1));

    // A large hard one takes many turns.  Each is short: the app is
    // never stopped for taking too long, and softkeys are answered.
    choose_from_play(&mut r, 14, Difficulty::Hard);
    assert_eq!(r.ui.texts(APP), ["14x14 Hard", "Generating\u{2026}"]);
    assert!(r.ui.drawings(APP).is_empty());
    // Something else takes the screen: the work stops, and nothing
    // arrives however long that lasts.
    r.ui.cover(APP, true);
    r.settle();
    assert_eq!(r.mgr.pump(), None, "no turn of the work is due");
    assert!(generating(&r));
    assert_eq!(
        parse(&file(&r)).0,
        native(7, Difficulty::Hard, 1),
        "the puzzle before is still the one kept"
    );
    r.ui.cover(APP, false);
    r.settle();
    wait(&mut r);
    assert_eq!(beside(&r), PLAYING);
    // The same puzzle as when made in one go.
    let large = native(14, Difficulty::Hard, 2);
    assert_eq!(parse(&file(&r)), (large.clone(), (0, 0)));

    // New while generating is the list again; Back from there is the
    // puzzle there was, untouched.
    r.key(APP, Key::Digit(1));
    let played = state(&mut r);
    choose_from_play(&mut r, 14, Difficulty::Hard);
    assert!(generating(&r));
    r.softkey(APP, "New");
    assert_eq!(r.ui.texts(APP), ["New puzzle"]);
    assert_eq!(r.mgr.pump(), None, "no turn of the abandoned work is due");
    assert_eq!(
        r.ui.texts(APP),
        ["New puzzle"],
        "the abandoned puzzle does not turn up"
    );
    r.softkey(APP, "Back");
    assert_eq!(beside(&r), PLAYING);
    assert_eq!(state(&mut r), played);

    // Exit while generating: the app stops, and the puzzle there was is
    // there the next time.
    choose_from_play(&mut r, 10, Difficulty::Hard);
    assert!(generating(&r));
    r.softkey(APP, "Exit");
    assert!(!r.mgr.is_running(APP));
    r.launch(APP);
    assert_eq!(beside(&r), PLAYING);
    assert_eq!(state(&mut r), played);
    // Cancel while generating leaves too.
    choose_from_play(&mut r, 10, Difficulty::Hard);
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
}

fn choose_from_play(r: &mut Rig, side: usize, difficulty: Difficulty) {
    r.softkey(APP, "New");
    choose(r, side, difficulty);
}

#[test]
fn without_a_seed_file_every_puzzle_is_a_new_one() {
    // The phone's random source seeds each puzzle: two phones (or one
    // phone twice) do not get the same one.
    let mut puzzles = Vec::new();
    for _ in 0..3 {
        listen();
        let mut r = Rig::new(FakeConfig::default());
        r.launch(APP);
        choose(&mut r, 10, Difficulty::Easy);
        wait(&mut r);
        let (puzzle, _) = parse(&file(&r));
        puzzles.push(puzzle);
    }
    assert!(puzzles[0] != puzzles[1] && puzzles[1] != puzzles[2] && puzzles[0] != puzzles[2]);
}

// --------------------------------------------------------------- the log

struct Lines;

impl log::Log for Lines {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            // E.g., why the manager stopped the app.
            eprintln!("{}: {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

/// Shows what goes wrong around the app.
fn listen() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        log::set_logger(&Lines).unwrap();
        log::set_max_level(log::LevelFilter::Warn);
    });
}

// ------------------------------------------- the time and the best times

/// A puzzle file one light from solved, with the cursor on the cell of
/// that light: key 1 solves it.
fn almost(side: usize, difficulty: Difficulty, seed: u64, elapsed: u64) -> String {
    let puzzle = native(side, difficulty, seed);
    let solved = solution(&puzzle);
    let lights: Vec<usize> = (0..puzzle.cells())
        .filter(|i| solved.has_light(*i))
        .collect();
    let mut game = Game::new(puzzle);
    for i in &lights[1..] {
        game.set_cursor(i % side, i / side);
        game.act(Act::Light);
    }
    assert!(!game.solved());
    format!(
        "size={side}\ndifficulty={}\nelapsed={elapsed}\ncursor={},{}\n{}",
        difficulty.name().to_lowercase(),
        lights[0] % side,
        lights[0] / side,
        game.grid().to_text()
    )
}

fn put_puzzle(r: &Rig, puzzle: &str) {
    std::fs::create_dir_all(storage(&r.nvdata)).unwrap();
    std::fs::write(storage(&r.nvdata).join("seed"), "1").unwrap();
    std::fs::write(storage(&r.nvdata).join("puzzle"), puzzle).unwrap();
}

/// Nov 13, 2026, 2:15 PM UTC: what the wall clock of the phones of the
/// tests of the best times says.
const T: u64 = 1_794_579_300;
/// `T` as the phones show it.
const T_SHOWN: &str = "Nov 13, 2026 2:15 PM";

/// A configuration whose wall clock says `T`.
fn at_t() -> FakeConfig {
    FakeConfig {
        wall_time: Some(Arc::new(AtomicU64::new(T))),
        ..FakeConfig::default()
    }
}

/// A phone of a PBX (`FakeStore::phone`) on which the app finds this
/// puzzle file.
fn rig_on(app_data: FakeStore, puzzle: &str) -> Rig {
    listen();
    let r = Rig::with_app_data(staged(), at_t(), Model::D60, common::phone_budget(), app_data);
    put_puzzle(&r, puzzle);
    r
}

/// The times the phone has yet to write to the PBX, as it keeps them.
fn waiting(r: &Rig) -> String {
    local(&r.nvdata, "times")
}

/// The table of a kind of puzzle as the PBX has it.
fn table(pbx: &FakeStore, kind: Kind) -> Table {
    Table::parse(
        &pbx.value(APP, StoreScope::Shared, &kind.key())
            .unwrap_or_default(),
    )
}

/// The rows of a table: who, and the time in whole seconds.
fn rows(table: &Table) -> Vec<(&str, &str, u64)> {
    table
        .rows()
        .iter()
        .map(|r| (r.id.as_str(), r.name.as_str(), r.ms / 1000))
        .collect()
}

/// The lines of the table on the screen, each the place and the name,
/// when the puzzle was solved, and the time; none if it says something
/// in place of them.
fn lines(r: &Rig) -> Vec<[String; 3]> {
    // After the title, four texts a line: under it all (empty), and the
    // three.
    let texts = r.ui.texts(APP);
    let rest = &texts[1..];
    if !rest.len().is_multiple_of(4) || rest.iter().step_by(4).any(|t| !t.is_empty()) {
        return Vec::new();
    }
    rest.chunks(4)
        .map(|c| [c[1].clone(), c[2].clone(), c[3].clone()])
        .collect()
}

/// The place and the name, and the time, of each line on the screen.
fn listed(r: &Rig) -> Vec<(String, String)> {
    lines(r)
        .into_iter()
        .map(|[name, _, time]| (name, time))
        .collect()
}

/// Starts the app on its puzzle file, which is one light from solved,
/// and puts the light.
fn solve(r: &mut Rig) {
    r.launch(APP);
    r.key(APP, Key::Digit(1));
    assert_eq!(r.ui.texts(APP)[0], "Solved!");
}

/// Lets the phone run for a while.
fn pass(r: &mut Rig, time: Duration) {
    let start = Instant::now();
    while start.elapsed() < time {
        std::thread::sleep(Duration::from_millis(5));
        r.mgr.pump();
    }
    r.settle();
}

fn seconds(time: &str) -> u64 {
    time.split(':').fold(0, |s, n| s * 60 + n.parse::<u64>().unwrap())
}

#[test]
fn the_time_spent_is_shown_and_runs_while_the_puzzle_is_on_the_screen() {
    let mut r = rig_with(Model::D60, WRONG);
    r.launch(APP);
    // What the file says, and then it goes on.
    assert_eq!(seconds(&time_shown(&r)), 125);
    pass(&mut r, Duration::from_millis(2100));
    let shown = seconds(&time_shown(&r));
    assert!((127..130).contains(&shown), "{shown}");
    // It is the time the puzzle file gets.
    let kept = |r: &Rig| -> u64 {
        let file = file(r);
        let ms = file.lines().find_map(|l| l.strip_prefix("elapsed="));
        ms.expect("a time").parse().unwrap()
    };
    // Something else has the screen (e.g., a call), and the time runs
    // again when the puzzle is back.
    r.ui.cover(APP, true);
    r.settle();
    let covered = kept(&r);
    assert!((shown..shown + 2).contains(&(covered / 1000)), "{covered}");
    r.ui.cover(APP, false);
    r.settle();
    let back = seconds(&time_shown(&r));
    pass(&mut r, Duration::from_millis(1100));
    assert!(seconds(&time_shown(&r)) > back);
    // Undo and key 8 are moves like any other: the time goes on.
    let before = seconds(&time_shown(&r));
    r.key(APP, Key::Digit(8));
    r.softkey(APP, "Undo");
    assert!(seconds(&time_shown(&r)) >= before);
    // A solved puzzle shows the time it took.
    let mut r = rig_with(Model::D60, &almost(7, Difficulty::Easy, 5, 59_000));
    solve(&mut r);
    let texts = r.ui.texts(APP);
    assert!(texts[1] == "Time 0:59" || texts[1] == "Time 1:00", "{texts:?}");
}

#[test]
fn a_solved_puzzles_time_is_on_every_phones_table_with_who_solved_it() {
    let pbx = FakeStore::default();
    let easy = Kind::new(7, Difficulty::Easy);
    let mut alice = rig_on(
        pbx.phone("101", "Alice"),
        &almost(7, Difficulty::Easy, 5, 83_000),
    );
    let mut bob = rig_on(pbx.phone("102", "Bob"), "");

    // Bob has no puzzle: the list of sizes, from which Scores shows the
    // table of the row.  Nobody has solved a puzzle yet.
    bob.launch(APP);
    assert!(bob.app_data.watched().is_empty());
    bob.softkey(APP, "Scores");
    assert_eq!(
        bob.ui.texts(APP),
        ["Best times: 7x7 Easy", "No times yet."]
    );
    assert_eq!(bob.app_data.watched(), [APP]);

    // Alice solves hers, at T.
    solve(&mut alice);
    let ms = table(&pbx, easy).rows()[0].ms;
    assert_eq!(rows(&table(&pbx, easy)), [("101", "Alice", 83)]);
    assert_eq!(table(&pbx, easy).rows()[0].when, T);
    let time = lightup_game::scores::clock(ms);
    assert_eq!(alice.ui.texts(APP), ["Solved!", &format!("Time {time}")]);
    assert_eq!(waiting(&alice), "", "nothing waits");
    // Only the table of that kind of puzzle was written.
    for kind in Kind::all().filter(|k| *k != easy) {
        assert!(table(&pbx, kind).rows().is_empty());
    }

    // Bob's phone, with the table on its screen, is told, and shows it:
    // who, when, and the time.
    let first = [("1. Alice".to_string(), time.clone())];
    bob.settle();
    assert_eq!(bob.ui.texts(APP)[0], "Best times: 7x7 Easy");
    assert_eq!(lines(&bob), [["1. Alice", T_SHOWN, time.as_str()]]);

    // Scores on the solved puzzle: the table of its kind.
    alice.softkey(APP, "Scores");
    assert_eq!(alice.ui.texts(APP)[0], "Best times: 7x7 Easy");
    assert_eq!(listed(&alice), first);
    // The other kinds: the next size, the next difficulty, and left and
    // right.
    alice.softkey(APP, "Size");
    assert_eq!(
        alice.ui.texts(APP),
        ["Best times: 10x10 Easy", "No times yet."]
    );
    alice.softkey(APP, "Level");
    assert_eq!(alice.ui.texts(APP)[0], "Best times: 10x10 Medium");
    alice.key(APP, Key::Left);
    assert_eq!(alice.ui.texts(APP)[0], "Best times: 10x10 Easy");
    alice.key(APP, Key::Right);
    assert_eq!(alice.ui.texts(APP)[0], "Best times: 10x10 Medium");
    // Back, the softkey or Cancel, is the solved puzzle.
    alice.softkey(APP, "Back");
    assert_eq!(alice.ui.texts(APP), ["Solved!", &format!("Time {time}")]);
    alice.softkey(APP, "Scores");
    alice.key(APP, Key::Cancel);
    assert_eq!(alice.ui.texts(APP), ["Solved!", &format!("Time {time}")]);
    assert!(alice.mgr.is_running(APP));

    // Bob goes back to the list, to the row he came from, and his phone
    // no longer asks to be told.
    bob.softkey(APP, "Level");
    assert_eq!(bob.ui.texts(APP)[0], "Best times: 7x7 Medium");
    bob.softkey(APP, "Back");
    assert_eq!(bob.ui.texts(APP), ["New puzzle"]);
    assert!(bob.app_data.watched().is_empty());
    bob.softkey(APP, "Scores");
    assert_eq!(bob.ui.texts(APP)[0], "Best times: 7x7 Easy");
    bob.softkey(APP, "Back");
    bob.softkey(APP, "Exit");

    // He plays a puzzle of the same kind.  Scores while playing: the
    // table, and back to the puzzle.
    put_puzzle(&bob, &almost(7, Difficulty::Easy, 6, 125_000));
    bob.launch(APP);
    bob.softkey(APP, "Scores");
    assert_eq!(listed(&bob), first);
    bob.softkey(APP, "Back");
    assert_eq!(beside(&bob), PLAYING);
    // He solves it, in more time than Alice: the second row.
    bob.key(APP, Key::Digit(1));
    assert_eq!(bob.ui.texts(APP)[0], "Solved!");
    assert_eq!(
        rows(&table(&pbx, easy)),
        [("101", "Alice", 83), ("102", "Bob", 125)]
    );
    alice.softkey(APP, "Scores");
    assert_eq!(
        listed(&alice),
        [first[0].clone(), ("2. Bob".to_string(), "2:05".to_string())]
    );
    alice.softkey(APP, "Back");
    alice.softkey(APP, "Exit");

    // Alice twice again, slower than before and faster: each time is a
    // row of its own.
    put_puzzle(&alice, &almost(7, Difficulty::Easy, 7, 300_000));
    solve(&mut alice);
    alice.softkey(APP, "Exit");
    put_puzzle(&alice, &almost(7, Difficulty::Easy, 8, 40_000));
    solve(&mut alice);
    assert_eq!(
        rows(&table(&pbx, easy)),
        [
            ("101", "Alice", 40),
            ("101", "Alice", 83),
            ("102", "Bob", 125),
            ("101", "Alice", 300)
        ]
    );
    alice.softkey(APP, "Scores");
    assert_eq!(
        listed(&alice).iter().map(|i| i.0.as_str()).collect::<Vec<_>>(),
        ["1. Alice", "2. Alice", "3. Bob", "4. Alice"]
    );
    // Her best line is in the selection's colors; up and down move them,
    // around the ends.
    let selected = |r: &Rig| r.ui.highlighted(APP).get(1).cloned().unwrap_or_default();
    assert_eq!(selected(&alice), "1. Alice");
    alice.key(APP, Key::Down);
    alice.key(APP, Key::Down);
    assert_eq!(selected(&alice), "3. Bob");
    for _ in 0..3 {
        alice.key(APP, Key::Up);
    }
    assert_eq!(selected(&alice), "4. Alice");
    alice.key(APP, Key::Down);
    assert_eq!(selected(&alice), "1. Alice");
}

#[test]
fn two_phones_that_finish_at_once_are_both_on_the_table() {
    // Each phone reads the table to put its time in it, and one writes
    // it before the other's write arrives: that write is refused, and the
    // phone writes again over the table that has both.
    let pbx = FakeStore::default();
    let mut carol = rig_on(
        pbx.phone("103", "Carol"),
        &almost(14, Difficulty::Medium, 3, 900_000),
    );
    let mut dan = rig_on(
        pbx.phone("104", "Dan"),
        &almost(14, Difficulty::Medium, 4, 800_000),
    );
    carol.launch(APP);
    dan.launch(APP);
    carol.ui.press(APP, Key::Digit(1));
    dan.ui.press(APP, Key::Digit(1));
    for _ in 0..4 {
        carol.mgr.pump();
        dan.mgr.pump();
    }
    carol.settle();
    dan.settle();
    assert_eq!(
        rows(&table(&pbx, Kind::new(14, Difficulty::Medium))),
        [("104", "Dan", 800), ("103", "Carol", 900)]
    );
    // They did meet: three writes, one of them refused.
    let writes = |r: &Rig| {
        let ops = r.app_data.ops();
        ops.iter().filter(|op| matches!(op, StoreOp::Set { .. })).count()
    };
    assert_eq!(writes(&carol) + writes(&dan), 3);
    assert_eq!(waiting(&carol), "");
    assert_eq!(waiting(&dan), "");
}

#[test]
fn two_wins_in_the_very_same_time_are_two_rows() {
    // Two wins of Alice's of one kind, in the very same time, an hour
    // apart, wait on her phone.  The PBX has the first already: it was
    // written, and the answer did not come.
    let pbx = FakeStore::default();
    let easy = Kind::new(7, Difficulty::Easy);
    let mut first = Table::default();
    first.add(Win { ms: 83_000, when: T }, "101", "Alice");
    pbx.phone("101", "Alice").run(opendeskphone_apps::StoreOp::Set {
        app: APP.into(),
        scope: StoreScope::Shared,
        key: easy.key(),
        value: first.to_bytes(),
        expect: None,
    });
    let mut alice = rig_on(pbx.phone("101", "Alice"), "");
    let times = format!("7x7-easy 83000 {T}\n7x7-easy 83000 {}\n", T + 3600);
    std::fs::write(storage(&alice.nvdata).join("times"), times).unwrap();
    // The app starts: the first is not written again, and the second is
    // a row of its own.
    alice.launch(APP);
    assert_eq!(
        rows(&table(&pbx, easy)),
        [("101", "Alice", 83), ("101", "Alice", 83)]
    );
    let when: Vec<u64> = table(&pbx, easy).rows().iter().map(|r| r.when).collect();
    assert_eq!(when, [T, T + 3600]);
    assert_eq!(waiting(&alice), "");
    // Each line says when.
    alice.softkey(APP, "Scores");
    assert_eq!(
        lines(&alice),
        [
            ["1. Alice", T_SHOWN, "1:23"],
            ["2. Alice", "Nov 13, 2026 3:15 PM", "1:23"]
        ]
    );
}

#[test]
fn a_time_set_out_of_reach_is_kept_and_written_when_the_store_is_in_reach() {
    let pbx = FakeStore::default();
    let easy = Kind::new(7, Difficulty::Easy);
    let mut alice = rig_on(
        pbx.phone("101", "Alice"),
        &almost(7, Difficulty::Easy, 5, 83_000),
    );
    // Bob's time is on the PBX, which Alice's phone cannot reach.
    let mut theirs = Table::default();
    theirs.add(Win { ms: 70_000, when: 1_794_579_300 }, "102", "Bob");
    pbx.phone("102", "Bob").run(opendeskphone_apps::StoreOp::Set {
        app: APP.into(),
        scope: StoreScope::Shared,
        key: easy.key(),
        value: theirs.to_bytes(),
        expect: None,
    });
    alice.app_data.set_online(false);
    solve(&mut alice);
    let time = alice.ui.texts(APP)[1].strip_prefix("Time ").unwrap().to_string();
    assert_eq!(seconds(&time), 83);
    // The time waits on the phone.
    let kept = waiting(&alice);
    assert!(kept.starts_with("7x7-easy 83") && kept.ends_with('\n'), "{kept}");
    // The table says so, and has the time on this phone.
    alice.softkey(APP, "Scores");
    assert_eq!(alice.ui.texts(APP)[0], "Offline: 7x7 Easy");
    assert_eq!(lines(&alice), [["1. Alice", T_SHOWN, time.as_str()]]);
    alice.softkey(APP, "Size");
    assert_eq!(
        alice.ui.texts(APP),
        ["Offline: 10x10 Easy", "The PBX cannot be reached."]
    );
    alice.softkey(APP, "Back");
    assert_eq!(alice.ui.texts(APP), ["Solved!", &format!("Time {time}")]);
    alice.softkey(APP, "Scores");

    // The PBX is in reach again: the time is written, with the times
    // that were there, and the table on the screen is the PBX's.
    alice.app_data.set_online(true);
    alice.settle();
    assert_eq!(
        rows(&table(&pbx, easy)),
        [("102", "Bob", 70), ("101", "Alice", 83)]
    );
    assert_eq!(waiting(&alice), "");
    assert_eq!(alice.ui.texts(APP)[0], "Best times: 7x7 Easy");
    assert_eq!(
        listed(&alice),
        [
            ("1. Bob".to_string(), "1:10".to_string()),
            ("2. Alice".to_string(), time.clone())
        ]
    );

    // Out of reach again, two puzzles of one kind and one of another are
    // solved: each time waits.  The app is left, and the PBX comes in
    // reach while it does not run.
    alice.softkey(APP, "Back");
    alice.softkey(APP, "Exit");
    alice.app_data.set_online(false);
    let mut times = Vec::new();
    for (side, seed, elapsed) in [(7, 6, 60_000), (7, 7, 95_000), (10, 8, 400_000)] {
        put_puzzle(&alice, &almost(side, Difficulty::Easy, seed, elapsed));
        solve(&mut alice);
        times.push(alice.ui.texts(APP)[1].strip_prefix("Time ").unwrap().to_string());
        if seed == 7 {
            // The table on this phone has both times that wait.
            alice.softkey(APP, "Scores");
            assert_eq!(alice.ui.texts(APP)[0], "Offline: 7x7 Easy");
            assert_eq!(
                listed(&alice),
                [
                    ("1. Alice".to_string(), times[0].clone()),
                    ("2. Alice".to_string(), times[1].clone())
                ]
            );
            alice.softkey(APP, "Back");
        }
        alice.softkey(APP, "Exit");
        assert!(!alice.mgr.is_running(APP));
    }
    let kept = waiting(&alice);
    let lines: Vec<&str> = kept.lines().collect();
    assert_eq!(lines.len(), 3, "{kept}");
    assert!(lines[0].starts_with("7x7-easy 60"), "{kept}");
    assert!(lines[1].starts_with("7x7-easy 95"), "{kept}");
    assert!(lines[2].starts_with("10x10-easy 400"), "{kept}");
    alice.app_data.set_online(true);
    alice.settle();
    assert_eq!(table(&pbx, easy).rows().len(), 2);
    assert_eq!(waiting(&alice), kept);
    // When the app next starts, it writes them.
    alice.launch(APP);
    assert_eq!(
        rows(&table(&pbx, easy)),
        [
            ("101", "Alice", 60),
            ("102", "Bob", 70),
            ("101", "Alice", 83),
            ("101", "Alice", 95)
        ]
    );
    assert_eq!(
        rows(&table(&pbx, Kind::new(10, Difficulty::Easy))),
        [("101", "Alice", 400)]
    );
    assert_eq!(waiting(&alice), "");
}

#[test]
fn a_phone_without_a_pbx_store_plays_as_ever_and_keeps_its_times() {
    listen();
    let mut r = Rig::with_services(|mut services| {
        services.store = std::sync::Arc::new(opendeskphone_apps::NoStore);
        services
    });
    put_puzzle(&r, &almost(7, Difficulty::Easy, 5, 83_000));
    solve(&mut r);
    assert!(waiting(&r).starts_with("7x7-easy 83"));
    // The table of this phone's times; it does not know its user's name.
    r.softkey(APP, "Scores");
    assert_eq!(r.ui.texts(APP)[0], "Offline: 7x7 Easy");
    assert_eq!(listed(&r), [("1. You".to_string(), "1:23".to_string())]);
}

// ------------------------------------------------- the touchscreen (D80)

/// The canvas over the board, which takes the touches: its place in the
/// window and its side.  It is the canvas made after the rows.
fn board(r: &Rig, side: usize) -> (i32, i32, i32) {
    let (bounds, ops) = r.ui.drawings(APP)[side].clone();
    assert!(ops.is_empty(), "nothing is drawn over the board: there is no cursor");
    assert_eq!(bounds.width, bounds.height);
    (i32::from(bounds.x), i32::from(bounds.y), i32::from(bounds.width))
}

/// A touch in the middle of a cell.
fn touch(r: &mut Rig, side: usize, cell: (usize, usize)) {
    let (x, y, width) = board(r, side);
    let pitch = (width - 1) / side as i32;
    r.ui.tap(APP, x + cell.0 as i32 * pitch + pitch / 2, y + cell.1 as i32 * pitch + pitch / 2);
    r.settle();
}

#[test]
fn on_the_touchscreen_a_puzzle_is_played_by_touching_its_cells() {
    let mut r = rig(Model::D80, 7);
    r.launch(APP);
    // No puzzle yet: the list, whose rows are touched.
    assert_eq!(r.ui.texts(APP), ["New puzzle"]);
    r.ui.tap_row(APP, 0);
    r.settle();
    wait(&mut r);
    let puzzle = native(7, Difficulty::Easy, 7);
    assert_eq!(state(&mut r).0, puzzle);

    // The upright window: the time beside the back button, the board
    // under that line, as wide as the window allows, and under the board
    // what a touch does, which names no key.  There is no picture of keys.
    let (x, y, width) = board(&r, 7);
    assert_eq!((x, y, width), (0, 96, 7 * 114 + 1));
    assert!(!r.ui.takes_touches(APP, 0));
    assert_eq!(r.ui.drawings(APP).len(), 7 + 1, "the rows and what is touched");
    let texts = beside(&r);
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert!(texts[0].starts_with("Touch a cell"), "{texts:?}");
    assert!(!texts.concat().chars().any(|c| c.is_ascii_digit()), "{texts:?}");
    // Five softkeys: the phone shows three and "More".
    assert_eq!(r.ui.softkeys(APP), ["Undo", "Auto", "New", "Help", "Scores"]);

    // A touch puts a light, the next makes it a square, and the third
    // clears the cell; a black cell takes nothing.
    let white = (0..49).find(|i| !puzzle.is_black(*i)).unwrap();
    let black = (0..49).find(|i| puzzle.is_black(*i)).unwrap();
    let cell = |i: usize| (i % 7, i / 7);
    touch(&mut r, 7, cell(white));
    assert!(state(&mut r).0.has_light(white));
    touch(&mut r, 7, cell(white));
    let (grid, cursor) = state(&mut r);
    assert!(grid.is_marked(white) && !grid.has_light(white));
    assert_eq!(cursor, cell(white));
    touch(&mut r, 7, cell(white));
    assert_eq!(state(&mut r).0, puzzle);
    touch(&mut r, 7, cell(black));
    assert_eq!(state(&mut r).0, puzzle);
    // The last grid line is the last cell's, and beside the board nothing
    // is touched.
    let last = 48;
    r.ui.tap(APP, x + width - 1, y + width - 1);
    r.ui.tap(APP, x + width / 2, y + width + 20);
    r.ui.tap(APP, x + width / 2, y - 20);
    r.settle();
    let after = state(&mut r).0;
    assert_eq!(after.has_light(last), !puzzle.is_black(last));
    assert_eq!((0..48).filter(|i| after.has_light(*i)).count(), 0);
    // A drag does nothing to the board.
    r.ui.drag(APP, (x + 60, y + 60), (x + 400, y + 400));
    r.settle();
    assert_eq!(state(&mut r).0, after);
    // Undo takes the touches back, one by one.
    for _ in 0..4 {
        r.softkey(APP, "Undo");
    }
    assert_eq!(state(&mut r).0, puzzle);

    // "Auto" does what the key 8 does.
    let mut expected = Game::new(puzzle.clone());
    let auto = expected.auto();
    r.softkey(APP, "Auto");
    assert_eq!(state(&mut r).0, *expected.grid());
    if auto {
        r.softkey(APP, "Undo");
    }
    assert_eq!(state(&mut r).0, puzzle);

    // The lights of the solution, each with one touch: solved, with the
    // time where the line about touching was.
    let solved = solution(&puzzle);
    for i in (0..49).filter(|i| solved.has_light(*i)) {
        touch(&mut r, 7, cell(i));
    }
    let texts = r.ui.texts(APP);
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert_eq!(texts[0], "Solved!");
    assert!(texts[1].starts_with("Time "), "{texts:?}");
    assert_eq!(r.ui.softkeys(APP), ["New", "Help", "Scores"]);
    // A solved puzzle does not change.
    let kept = state(&mut r).0;
    touch(&mut r, 7, cell(white));
    assert_eq!(state(&mut r).0, kept);
    // The back button is Cancel, which leaves the app.
    r.key(APP, Key::Cancel);
    assert!(!r.mgr.is_running(APP));
}

#[test]
fn on_the_touchscreen_the_other_screens_name_no_key_and_fit() {
    use lightup_game::help::PAGES;
    let mut r = rig_with(Model::D80, WRONG);
    r.launch(APP);
    let kept = state(&mut r);

    // The help: a picture and, under it, the sentences for a touchscreen.
    r.softkey(APP, "Help");
    for (n, page) in PAGES.iter().enumerate() {
        let texts = r.ui.texts(APP);
        assert_eq!(texts[0], format!("How to play ({}/{})", n + 1, PAGES.len()));
        assert_eq!(texts[1], page.words(false));
        assert!(!texts[1].chars().any(|c| c.is_ascii_digit() && c != '0'), "{}", texts[1]);
        let (picture, _) = r.ui.drawings(APP)[0].clone();
        assert!(picture.y >= 96 && picture.x > 0 && picture.x as u16 + picture.width < 800, "{picture:?}");
        r.softkey(APP, "Next");
    }
    assert_eq!(r.ui.texts(APP)[1], PAGES[0].words(false));
    // Back (the back button is Cancel) to the puzzle as it was.
    r.key(APP, Key::Cancel);
    assert_eq!(state(&mut r), kept);
    assert!(beside(&r)[0].starts_with("Touch a cell"));

    // The tables, and from them the puzzle again.
    r.softkey(APP, "Scores");
    assert!(r.ui.texts(APP)[0].contains("7x7 Hard"), "{:?}", r.ui.texts(APP));
    r.softkey(APP, "Size");
    assert!(r.ui.texts(APP)[0].contains("10x10 Hard"), "{:?}", r.ui.texts(APP));
    assert_eq!(r.ui.softkeys(APP), ["Size", "Level"]);
    r.key(APP, Key::Cancel);
    assert_eq!(state(&mut r), kept);

    // The largest board is as wide as the window allows too.
    let mut r = rig_with(Model::D80, &fresh(14, 3));
    r.launch(APP);
    assert_eq!(board(&r, 14), (0, 96, 14 * 57 + 1));

    // The phones with keys have their sentences, their keys, and their
    // cursor, and take no touches.
    let mut r = rig_with(Model::D60, WRONG);
    r.launch(APP);
    assert_eq!(r.ui.softkeys(APP), ["Undo", "New", "Help", "Scores"]);
    let (_, cursor) = r.ui.drawings(APP)[7].clone();
    assert!(!cursor.is_empty(), "the cursor is drawn");
    r.ui.tap(APP, 20, 20);
    r.settle();
    assert_eq!(state(&mut r).0, parse(WRONG).0);
    r.softkey(APP, "Help");
    assert_eq!(r.ui.texts(APP)[1], PAGES[0].text);
}

/// A puzzle file of a puzzle nobody has played yet.
fn fresh(side: usize, seed: u64) -> String {
    format!("size={side}\ndifficulty=easy\nelapsed=65000\ncursor=0,0\n{}", native(side, Difficulty::Easy, seed).to_text())
}

/// A simulated D80 with the phone's own UI, which the app draws on and a
/// finger touches.
struct Phone {
    store: opendeskphone_state::Store,
    ui: opendeskphone_ui::Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    mgr: opendeskphone_apps::AppManager,
    nvdata: PathBuf,
    _dir: common::TempDir,
}

impl Phone {
    fn new(puzzle: &str) -> Phone {
        use opendeskphone_apps::testutil::{FakePbx, services_with_store};
        use opendeskphone_state::{Phase, Store, System};
        let model = Model::D80;
        let dir = common::temp_dir("lightup-phone");
        let nvdata = dir.0.join("nvdata");
        std::fs::create_dir_all(storage(&nvdata)).unwrap();
        std::fs::write(storage(&nvdata).join("puzzle"), puzzle).unwrap();
        let mut hal = opendeskphone_hal::Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        let config = opendeskphone_ui::UiConfig { backlight_timeout_secs: 0, ..Default::default() };
        let ui = opendeskphone_ui::Ui::from_hal(&mut hal, store.clone(), config);
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        let shell = opendeskphone_apps::uishell::UiShell::new(ui.handle(), model);
        let mut cfg = opendeskphone_apps::ManagerConfig::rooted(staged(), &nvdata);
        cfg.target = opendeskphone_apps::engine::Target::Pulley64;
        let services = services_with_store(&FakePbx::default(), FakeConfig::default(), &FakeStore::default());
        let mut mgr =
            opendeskphone_apps::AppManager::new(cfg, store.clone(), Box::new(shell), services).expect("engine");
        mgr.scan();
        let mut phone = Phone { store, ui, sim, mgr, nvdata, _dir: dir };
        phone.settle();
        phone.mgr.launch(APP).unwrap();
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

    fn tap(&mut self, x: i32, y: i32) {
        self.sim.touch_tap(x as u16, y as u16);
        self.settle();
    }

    /// Touches a softkey button: they are the four softkey keys, by
    /// their places.
    fn softkey(&mut self, n: u8) {
        let r = self.ui.canvas().hit_rect(opendeskphone_ui::Hit::Key(Key::Soft(n))).expect("a softkey button");
        self.tap(r.x + r.w / 2, r.y + r.h / 2);
    }

    fn pixel(&self, x: i32, y: i32) -> u32 {
        self.ui.canvas().pixel(x, y)
    }

    /// Writes the screen as a PNG, to look at.
    fn shot(&self, name: &str) {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lightup");
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (800, 1280);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let p = self.pixel(x as i32, y as i32);
                rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
        let file = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("D80-{name}.png"))).unwrap());
        let mut enc = png::Encoder::new(file, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    }

    /// The puzzle as the app keeps it: the phone's menu covers the app
    /// for a moment, at which the app writes its puzzle down.
    fn state(&mut self) -> Grid {
        self.settle();
        let p = self.store.send(opendeskphone_state::Command::ShowScreen { name: "menu".into() });
        self.settle();
        assert_eq!(p.wait(), Ok(opendeskphone_state::Reply::Done));
        self.mgr.launch(APP).unwrap();
        self.settle();
        assert_eq!(self.ui.screen_name(), "app:lightup");
        parse(&local(&self.nvdata, "puzzle")).0
    }
}

#[test]
fn on_a_d80_the_game_is_drawn_and_touched_on_the_phones_own_screen() {
    listen();
    for (side, puzzle) in [(7usize, WRONG.to_string()), (14, fresh(14, 3))] {
        let mut p = Phone::new(&puzzle);
        assert_eq!(p.ui.screen_name(), "app:lightup");
        let before = parse(&puzzle).0;
        p.shot(&format!("play-{side}"));
        // The board is under the title's line, across the window; the
        // back button's corner has the background, but for the arrow.
        let pitch = 799 / side as i32;
        let x0 = (800 - (pitch * side as i32 + 1)) / 2;
        let bg = p.pixel(799, 0);
        assert_eq!(p.pixel(2, 2), bg, "{side}: the corner is free");
        // The phone drew all of it: every row has its grid line at the
        // left edge of the board (a row over the limits of a canvas
        // would be missing).
        for row in 0..side as i32 {
            assert_ne!(p.pixel(x0, 96 + row * pitch + pitch / 2), bg, "{side}: row {row}");
        }
        // A touch on an empty white cell puts a light there.
        let i = (0..side * side)
            .find(|i| !before.is_black(*i) && !before.has_light(*i) && !before.is_marked(*i))
            .unwrap();
        let (cx, cy) = ((i % side) as i32, (i / side) as i32);
        p.tap(x0 + cx * pitch + pitch / 2, 96 + cy * pitch + pitch / 2);
        p.shot(&format!("touched-{side}"));
        assert!(p.state().has_light(i), "{side}");
        // The first softkey button is "Undo".
        p.softkey(1);
        assert_eq!(p.state(), before, "{side}");
        if side == 7 {
            // The fourth is "More"; then the first is "Help", and the
            // second "Scores".  The back button leaves either for the
            // puzzle.
            p.softkey(4);
            p.softkey(1);
            p.shot("help");
            p.tap(40, 40);
            p.softkey(4);
            p.softkey(2);
            p.shot("scores");
            p.tap(40, 40);
            assert_eq!(p.state(), before);
        }
    }
}
