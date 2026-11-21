//! Box Pusher, the puzzle: the app's life under the manager with the fake
//! screen.  The rules, the levels, the solver that proves them, and which
//! progress counts when the phone's and the PBX's differ are tested in the
//! crate `boxpusher-game` (`apps/boxpusher/game`).
//!
//! The progress is kept on the PBX too, whose store of app data is a
//! `FakeStore` here: the last tests are of a second phone, a PBX out of
//! reach, and a phone and a PBX that differ.

mod common;

use common::{Rig, staged};
use opendeskphone_apps::testutil::{FakeConfig, FakeStore, local_value};
use opendeskphone_apps::{DrawOp, StoreOp, StoreReply, StoreScope};
use opendeskphone_state::{Key, Model};
use std::path::{Path, PathBuf};

const APP: &str = "boxpusher";

fn rig() -> Rig {
    Rig::new(FakeConfig::default())
}

/// The level number and the two counts beside the board.
fn panel(r: &Rig) -> Vec<String> {
    r.ui.texts(APP)
}

/// What the canvas over the level shows: the boxes and the figure.
fn pieces(r: &Rig) -> String {
    let drawn = r.ui.drawings(APP);
    format!("{:?}", drawn.last().expect("the board").1)
}

/// The value of one of the game's keys on the phone.
fn local(nvdata: &Path, key: &str) -> Option<String> {
    let value = local_value(&nvdata.join("appdata").join(APP), key)?;
    Some(String::from_utf8(value).unwrap())
}

fn progress(r: &Rig) -> String {
    local(&r.nvdata, "progress").unwrap_or_default()
}

fn keys(r: &mut Rig, keys: &[Key]) {
    for k in keys {
        r.key(APP, *k);
    }
}

#[test]
fn a_level_from_the_first_step_to_solved() {
    let mut r = rig();
    r.launch(APP);
    assert_eq!(r.ui.top().as_deref(), Some(APP));
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0"]);
    let start = pieces(&r);

    // A step: the figure is drawn where it went.
    r.key(APP, Key::Right);
    assert_eq!(panel(&r), ["Level 1", "Moves 1", "Pushes 0"]);
    assert_ne!(pieces(&r), start);

    // A wall: nothing happens.
    keys(&mut r, &[Key::Up, Key::Down, Key::Digit(2), Key::Digit(8)]);
    assert_eq!(panel(&r), ["Level 1", "Moves 1", "Pushes 0"]);

    // 4 and 6 are Left and Right.
    r.key(APP, Key::Digit(4));
    assert_eq!(panel(&r), ["Level 1", "Moves 2", "Pushes 0"]);
    assert_eq!(pieces(&r), start, "back where it started");
    keys(&mut r, &[Key::Digit(6), Key::Digit(6)]);
    assert_eq!(panel(&r), ["Level 1", "Moves 4", "Pushes 1"]);

    // Undo takes the push back; Restart takes everything back.
    r.softkey(APP, "Undo");
    assert_eq!(panel(&r), ["Level 1", "Moves 3", "Pushes 0"]);
    r.key(APP, Key::Right);
    assert_eq!(panel(&r), ["Level 1", "Moves 4", "Pushes 1"]);
    r.softkey(APP, "Restart");
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0"]);
    assert_eq!(pieces(&r), start);
    r.softkey(APP, "Undo");
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0"]);

    // Three steps to the right solve the level.
    keys(&mut r, &[Key::Right, Key::Right, Key::Right]);
    assert_eq!(
        panel(&r),
        ["Level 1 solved!", "Moves: 3", "Pushes: 2", "Fewest pushes: 2"]
    );
    assert!(r.ui.drawings(APP).is_empty());
    assert_eq!(progress(&r), "level=1\nsolved=1\nmoves=\n");
    // The arrows do nothing here.
    r.key(APP, Key::Right);
    assert_eq!(panel(&r)[0], "Level 1 solved!");

    r.softkey(APP, "Again");
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0"]);
    keys(&mut r, &[Key::Right, Key::Right, Key::Right]);
    r.softkey(APP, "Next");
    assert_eq!(panel(&r), ["Level 2", "Moves 0", "Pushes 0"]);
    // Level 1 again, and Select on its solved screen goes on too.
    r.softkey(APP, "Levels");
    r.softkey(APP, "Play");
    assert_eq!(panel(&r), ["Level 2", "Moves 0", "Pushes 0"]);
    r.softkey(APP, "Levels");
    r.key(APP, Key::Up);
    r.key(APP, Key::Select);
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0"]);
    keys(&mut r, &[Key::Right, Key::Right, Key::Right, Key::Select]);
    assert_eq!(panel(&r), ["Level 2", "Moves 0", "Pushes 0"]);

    r.softkey(APP, "Exit");
    assert_eq!(r.ui.top(), None);
    assert!(!r.mgr.is_running(APP), "Exit stops the app");
}

#[test]
fn the_levels_open_one_by_one() {
    let mut r = rig();
    r.launch(APP);
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP), [("Level 1".to_string(), String::new())]);
    // Back, by the softkey or by Cancel, is the game as it was.
    r.softkey(APP, "Back");
    r.key(APP, Key::Right);
    r.softkey(APP, "Levels");
    r.key(APP, Key::Cancel);
    assert_eq!(panel(&r), ["Level 1", "Moves 1", "Pushes 0"]);

    keys(&mut r, &[Key::Right, Key::Right]);
    assert_eq!(panel(&r)[0], "Level 1 solved!");
    r.softkey(APP, "Levels");
    assert_eq!(
        r.ui.items(APP),
        [
            ("Level 1".to_string(), "Solved".to_string()),
            ("Level 2".to_string(), String::new())
        ]
    );
    // From the solved screen, Back goes on to the next level.
    r.softkey(APP, "Back");
    assert_eq!(panel(&r), ["Level 2", "Moves 0", "Pushes 0"]);
    // Level 2: up, right, up, right, down.
    keys(&mut r, &[Key::Up, Key::Right, Key::Up, Key::Right, Key::Down]);
    assert_eq!(
        panel(&r),
        ["Level 2 solved!", "Moves: 5", "Pushes: 2", "Fewest pushes: 2"]
    );
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP).len(), 3);
    assert_eq!(progress(&r), "level=2\nsolved=2\nmoves=\n");
}

#[test]
fn progress_is_kept_and_a_call_does_not_end_the_game() {
    let mut r = rig();
    r.launch(APP);
    keys(&mut r, &[Key::Right, Key::Right, Key::Right]);
    r.softkey(APP, "Next");
    keys(&mut r, &[Key::Up, Key::Right]);
    assert_eq!(panel(&r), ["Level 2", "Moves 2", "Pushes 1"]);
    let shown = pieces(&r);

    // A call's screen goes over the game.  The steps so far are written
    // down, and the game is there when the call is over.
    r.ui.cover(APP, true);
    r.settle();
    assert!(r.mgr.is_running(APP));
    assert_eq!(progress(&r), "level=1\nsolved=1\nmoves=ur\n");
    r.ui.cover(APP, false);
    r.settle();
    assert_eq!(panel(&r), ["Level 2", "Moves 2", "Pushes 1"]);
    assert_eq!(pieces(&r), shown);
    r.key(APP, Key::Up);
    assert_eq!(panel(&r), ["Level 2", "Moves 3", "Pushes 1"]);

    // Leaving with Cancel stops the app; the next time it is where it
    // was, and the steps can still be undone.
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.top(), None);
    assert!(!r.mgr.is_running(APP));
    assert_eq!(progress(&r), "level=1\nsolved=1\nmoves=uru\n");
    r.launch(APP);
    assert_eq!(panel(&r), ["Level 2", "Moves 3", "Pushes 1"]);
    r.softkey(APP, "Undo");
    assert_eq!(pieces(&r), shown);
    r.softkey(APP, "Exit");

    // A progress file that makes no sense is the beginning, on a phone
    // whose PBX is out of reach.
    r.app_data.set_online(false);
    let file = r.nvdata.join("appdata").join(APP).join("progress");
    for nonsense in ["", "level=99\nsolved=3\nmoves=zz\n", "solved=x\nlevel=5\n", "\u{0}\u{1}"] {
        std::fs::write(&file, nonsense).unwrap();
        r.launch(APP);
        let level = if nonsense.contains("solved=3") { "Level 4" } else { "Level 1" };
        assert_eq!(panel(&r), [level, "Moves 0", "Pushes 0"], "{nonsense:?}");
        r.softkey(APP, "Exit");
    }
    // The last level solved: there is no next one.
    std::fs::write(&file, "level=27\nsolved=28\nmoves=\n").unwrap();
    r.launch(APP);
    assert_eq!(panel(&r)[0], "Level 28");
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP).len(), 28);
    r.softkey(APP, "Back");
    r.softkey(APP, "Exit");

    // With the PBX in reach, a progress file that was lost is the PBX's
    // progress, and the levels the phone has solved go to the PBX.
    r.app_data.set_online(true);
    std::fs::write(&file, "").unwrap();
    r.launch(APP);
    assert_eq!(panel(&r), ["Level 2", "Moves 2", "Pushes 1"]);
    std::fs::write(&file, "level=27\nsolved=28\nmoves=\n").unwrap();
    r.softkey(APP, "Exit");
    r.launch(APP);
    assert_eq!(panel(&r)[0], "Level 28");
    let on_pbx = r.app_data.value(APP, StoreScope::User, "progress").unwrap();
    assert_eq!(String::from_utf8(on_pbx).unwrap(), "level=27\nsolved=28\nmoves=\n");
}

#[test]
fn it_is_drawn_in_color_or_in_the_gray_screens_black_and_white() {
    // The fake screen's foreground and background colors.
    let ink = |c: &opendeskphone_apps::Color| {
        [(32, 32, 32), (16, 16, 16)].contains(&(c.r, c.g, c.b))
    };
    for model in [Model::D60, Model::D40] {
        let mut r = Rig::with_root(staged(), FakeConfig::default(), model);
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

// ------------------------------------------------- progress on the PBX

const LEVEL_1: [Key; 3] = [Key::Right, Key::Right, Key::Right];
const LEVEL_2: [Key; 5] = [Key::Up, Key::Right, Key::Up, Key::Right, Key::Down];

/// A phone of `user` on the PBX of `pbx`, with nothing of the game on it.
fn phone(pbx: &FakeStore, user: &str) -> Rig {
    Rig::on_pbx(pbx.phone(user, user))
}

/// What the PBX has of a user's progress, read from another phone of the
/// user (one that is in reach when the user's is not).
fn on_pbx(pbx: &FakeStore, user: &str) -> Option<String> {
    let value = pbx.phone(user, user).value(APP, StoreScope::User, "progress");
    value.map(|v| String::from_utf8(v).unwrap())
}

/// Writes a user's progress on the PBX as another phone of the user
/// does; returns the entry's version.
fn write_on_pbx(pbx: &FakeStore, user: &str, progress: &str) -> u64 {
    let reply = pbx.phone(user, user).run(StoreOp::Set {
        app: APP.into(),
        scope: StoreScope::User,
        key: "progress".into(),
        value: progress.as_bytes().to_vec(),
        expect: None,
    });
    match reply {
        StoreReply::Stored(entry) => entry.version,
        other => panic!("{other:?}"),
    }
}

fn storage(r: &Rig) -> PathBuf {
    r.nvdata.join("appdata").join(APP)
}

/// Puts a file in the game's storage on the phone.
fn put(r: &Rig, key: &str, value: &str) {
    std::fs::create_dir_all(storage(r)).unwrap();
    std::fs::write(storage(r).join(key), value).unwrap();
}

/// The game's storage of one phone, copied to another: the same phone
/// with another user provisioned.
fn carry(from: &Rig, to: &Rig) {
    std::fs::create_dir_all(storage(to)).unwrap();
    for file in std::fs::read_dir(storage(from)).unwrap() {
        let file = file.unwrap();
        std::fs::copy(file.path(), storage(to).join(file.file_name())).unwrap();
    }
}

/// The requests to write that the phone made.
fn writes(r: &Rig) -> usize {
    let ops = r.app_data.ops();
    ops.iter().filter(|op| matches!(op, StoreOp::Set { .. })).count()
}

#[test]
fn a_reset_phone_and_a_second_phone_go_on_where_the_user_was() {
    let pbx = FakeStore::default();
    let mut first = phone(&pbx, "101");
    first.launch(APP);
    // Nothing is written for a game that has not begun.
    assert_eq!(on_pbx(&pbx, "101"), None);
    keys(&mut first, &LEVEL_1);
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=\n"));
    first.softkey(APP, "Next");
    keys(&mut first, &[Key::Up, Key::Right]);
    // The steps in a level go when the game leaves the screen.
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=\n"));
    first.softkey(APP, "Exit");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=ur\n"));

    // The phone after a reset, or another phone of the user: nothing of
    // the game is on it.
    let mut second = phone(&pbx, "101");
    second.launch(APP);
    assert_eq!(panel(&second), ["Level 2", "Moves 2", "Pushes 1"]);
    assert_eq!(progress(&second), "level=1\nsolved=1\nmoves=ur\n");
    assert_eq!(writes(&second), 0, "the PBX has it already");
    // The steps can be undone there too.
    second.softkey(APP, "Undo");
    assert_eq!(panel(&second), ["Level 2", "Moves 1", "Pushes 1"]);
    // The user leaves the second phone a step earlier in the level than
    // the first, and finds the first there too: what the first sent as
    // the game was left counts as arrived.
    second.softkey(APP, "Exit");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=u\n"));
    first.launch(APP);
    assert_eq!(panel(&first), ["Level 2", "Moves 1", "Pushes 1"]);
    first.softkey(APP, "Exit");
    second.launch(APP);
    assert_eq!(panel(&second), ["Level 2", "Moves 1", "Pushes 1"]);
}

#[test]
fn a_game_left_running_takes_up_what_was_played_on_another_phone() {
    let pbx = FakeStore::default();
    let mut r = phone(&pbx, "101");
    r.launch(APP);
    r.key(APP, Key::Right);
    // A call's screen is over the game, and meanwhile the user solves
    // five levels at another phone.
    r.ui.cover(APP, true);
    r.settle();
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=0\nsolved=0\nmoves=r\n"));
    write_on_pbx(&pbx, "101", "level=5\nsolved=5\nmoves=\n");
    r.ui.cover(APP, false);
    r.settle();
    assert_eq!(panel(&r), ["Level 6", "Moves 0", "Pushes 0"]);
    assert_eq!(progress(&r), "level=5\nsolved=5\nmoves=\n");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=5\nsolved=5\nmoves=\n"));
}

#[test]
fn what_is_played_out_of_reach_goes_to_the_pbx_when_it_is_in_reach() {
    let pbx = FakeStore::default();
    let mut r = phone(&pbx, "101");
    r.app_data.set_online(false);
    r.launch(APP);
    keys(&mut r, &LEVEL_1);
    r.softkey(APP, "Next");
    r.key(APP, Key::Up);
    // The game goes on as ever, and keeps its progress on the phone.
    assert_eq!(panel(&r), ["Level 2", "Moves 1", "Pushes 1"]);
    assert_eq!(progress(&r), "level=1\nsolved=1\nmoves=\n");
    assert_eq!(writes(&r), 0, "nothing is asked of a PBX out of reach");

    // The PBX comes into reach while the game is on screen.
    r.app_data.set_online(true);
    r.settle();
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=u\n"));
    assert_eq!(panel(&r), ["Level 2", "Moves 1", "Pushes 1"]);

    // Out of reach again until after the game was left: the progress
    // goes the next time the game is started.
    r.app_data.set_online(false);
    keys(&mut r, &[Key::Right, Key::Up, Key::Right, Key::Down]);
    assert_eq!(panel(&r)[0], "Level 2 solved!");
    r.softkey(APP, "Exit");
    assert_eq!(progress(&r), "level=2\nsolved=2\nmoves=\n");
    r.app_data.set_online(true);
    r.settle();
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=u\n"));
    r.launch(APP);
    assert_eq!(panel(&r), ["Level 3", "Moves 0", "Pushes 0"]);
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=2\nsolved=2\nmoves=\n"));
}

#[test]
fn a_phone_whose_pbx_keeps_no_app_data_plays_and_keeps_its_progress() {
    let mut r = Rig::with_services(|mut services| {
        services.store = std::sync::Arc::new(opendeskphone_apps::NoStore);
        services
    });
    r.launch(APP);
    keys(&mut r, &LEVEL_1);
    assert_eq!(panel(&r)[0], "Level 1 solved!");
    r.softkey(APP, "Next");
    keys(&mut r, &LEVEL_2);
    assert_eq!(panel(&r)[0], "Level 2 solved!");
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP).len(), 3);
    r.softkey(APP, "Back");
    r.key(APP, Key::Down);
    r.softkey(APP, "Exit");
    assert_eq!(progress(&r), "level=2\nsolved=2\nmoves=d\n");
    r.launch(APP);
    assert_eq!(panel(&r), ["Level 3", "Moves 1", "Pushes 0"]);
}

/// A phone of the user 101 on which the game has `progress`, and that
/// last knew the PBX to have `known`; meanwhile the PBX has `theirs`.
/// The game is started.
fn differing(progress: &str, known: Option<&str>, theirs: Option<&str>) -> (FakeStore, Rig) {
    let pbx = FakeStore::default();
    let r = phone(&pbx, "101");
    put(&r, "progress", progress);
    if let Some(known) = known {
        let version = write_on_pbx(&pbx, "101", known);
        put(&r, "pbx", &format!("user=101\nversion={version}\n{known}"));
    }
    if let Some(theirs) = theirs {
        write_on_pbx(&pbx, "101", theirs);
    }
    let mut r = r;
    r.launch(APP);
    (pbx, r)
}

#[test]
fn a_phone_and_a_pbx_that_differ_agree_on_the_right_progress() {
    let was = "level=1\nsolved=1\nmoves=\n";
    let mine = "level=3\nsolved=3\nmoves=\n";

    // The phone played out of reach, and the PBX has what it had: the
    // phone's progress goes to the PBX.
    let (pbx, r) = differing(mine, Some(was), None);
    assert_eq!(panel(&r)[0], "Level 4");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some(mine));

    // The phone has nothing new, and the PBX has: the PBX's progress is
    // gone on with, even to an earlier level, and no level closes.
    let (pbx, r) = differing(mine, Some(mine), Some("level=5\nsolved=5\nmoves=\n"));
    assert_eq!(panel(&r)[0], "Level 6");
    assert_eq!(progress(&r), "level=5\nsolved=5\nmoves=\n");
    assert_eq!(writes(&r), 0);
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=5\nsolved=5\nmoves=\n"));
    let (pbx, mut r) = differing(mine, Some(mine), Some("level=0\nsolved=2\nmoves=r\n"));
    assert_eq!(panel(&r), ["Level 1", "Moves 1", "Pushes 0"]);
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP).len(), 4);
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=0\nsolved=3\nmoves=r\n"));

    // Both have something new.  The PBX has more levels solved: the
    // PBX's.
    let (pbx, r) = differing(mine, Some(was), Some("level=4\nsolved=7\nmoves=\n"));
    assert_eq!(panel(&r)[0], "Level 5");
    assert_eq!(progress(&r), "level=4\nsolved=7\nmoves=\n");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=4\nsolved=7\nmoves=\n"));
    // The phone has more, or as many: the phone's.
    for theirs in ["level=2\nsolved=2\nmoves=u\n", "level=0\nsolved=3\nmoves=r\n"] {
        let (pbx, r) = differing(mine, Some(was), Some(theirs));
        assert_eq!(panel(&r), ["Level 4", "Moves 0", "Pushes 0"], "{theirs:?}");
        assert_eq!(on_pbx(&pbx, "101").as_deref(), Some(mine));
    }

    // A phone from before the progress went to the PBX: the same, with
    // a PBX that has nothing yet, less, or more.
    let (pbx, r) = differing(mine, None, None);
    assert_eq!(panel(&r)[0], "Level 4");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some(mine));
    let (pbx, r) = differing(mine, None, Some(was));
    assert_eq!(panel(&r)[0], "Level 4");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some(mine));
    let (pbx, r) = differing(mine, None, Some("level=9\nsolved=9\nmoves=\n"));
    assert_eq!(panel(&r)[0], "Level 10");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=9\nsolved=9\nmoves=\n"));

    // The PBX lost what it had: it gets the phone's again.
    let (pbx, r) = differing(mine, None, None);
    put(&r, "pbx", &format!("user=101\nversion=77\n{mine}"));
    let reply = pbx.phone("101", "101").run(StoreOp::Remove {
        app: APP.into(),
        scope: StoreScope::User,
        key: "progress".into(),
        expect: None,
    });
    assert!(matches!(reply, StoreReply::Removed), "{reply:?}");
    let mut r = r;
    r.softkey(APP, "Exit");
    r.launch(APP);
    assert_eq!(panel(&r)[0], "Level 4");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some(mine));

    // A value on the PBX that makes no sense closes no level.
    let (pbx, r) = differing(mine, Some(mine), Some("\u{0}nonsense"));
    assert_eq!(panel(&r)[0], "Level 1");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=0\nsolved=3\nmoves=\n"));
}

#[test]
fn a_player_who_is_playing_is_not_moved_to_another_level() {
    // The PBX answers after the player's first step (here: it comes into
    // reach then).  It has more levels solved.
    let pbx = FakeStore::default();
    write_on_pbx(&pbx, "101", "level=5\nsolved=5\nmoves=\n");
    let mut r = phone(&pbx, "101");
    r.app_data.set_online(false);
    r.launch(APP);
    r.key(APP, Key::Right);
    r.app_data.set_online(true);
    r.settle();
    assert_eq!(panel(&r), ["Level 1", "Moves 1", "Pushes 0"]);
    // The levels are open all the same, and the PBX is told where the
    // player is.
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP).len(), 6);
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=0\nsolved=5\nmoves=r\n"));

    // The same with the list of levels on screen: it is the longer list.
    let pbx = FakeStore::default();
    write_on_pbx(&pbx, "101", "level=5\nsolved=5\nmoves=\n");
    let mut r = phone(&pbx, "101");
    r.app_data.set_online(false);
    r.launch(APP);
    r.softkey(APP, "Levels");
    assert_eq!(r.ui.items(APP).len(), 1);
    r.app_data.set_online(true);
    r.settle();
    assert_eq!(r.ui.items(APP).len(), 6);
}

#[test]
fn a_phone_that_gets_another_user_keeps_each_users_progress_apart() {
    let pbx = FakeStore::default();
    let mut desk = phone(&pbx, "101");
    desk.launch(APP);
    keys(&mut desk, &LEVEL_1);
    desk.softkey(APP, "Next");
    // The PBX goes out of reach, and 101 solves another level.
    desk.app_data.set_online(false);
    keys(&mut desk, &LEVEL_2);
    desk.softkey(APP, "Exit");
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=\n"));

    // The phone is provisioned for 102, who has not played.
    let mut desk_102 = phone(&pbx, "102");
    carry(&desk, &desk_102);
    desk_102.launch(APP);
    assert_eq!(panel(&desk_102), ["Level 1", "Moves 0", "Pushes 0"]);
    desk_102.softkey(APP, "Levels");
    assert_eq!(desk_102.ui.items(APP).len(), 1);
    desk_102.softkey(APP, "Back");
    keys(&mut desk_102, &LEVEL_1);
    desk_102.softkey(APP, "Exit");
    assert_eq!(on_pbx(&pbx, "102").as_deref(), Some("level=1\nsolved=1\nmoves=\n"));
    // Neither user's progress went to the other's keys.
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=1\nsolved=1\nmoves=\n"));

    // 101 is back at the phone: the level solved out of reach is there,
    // and goes to the PBX.
    let mut desk_101 = phone(&pbx, "101");
    carry(&desk_102, &desk_101);
    desk_101.launch(APP);
    assert_eq!(panel(&desk_101), ["Level 3", "Moves 0", "Pushes 0"]);
    assert_eq!(on_pbx(&pbx, "101").as_deref(), Some("level=2\nsolved=2\nmoves=\n"));
    assert_eq!(on_pbx(&pbx, "102").as_deref(), Some("level=1\nsolved=1\nmoves=\n"));
    // Nothing is set aside any longer.
    let mut files: Vec<String> = std::fs::read_dir(storage(&desk_101))
        .unwrap()
        .map(|f| f.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with('.'))
        .collect();
    files.sort();
    assert_eq!(files, ["pbx", "progress"]);
}

// ------------------------------------------------------ the touchscreen

/// Where the middle of a square of level 1 is in the D80's window: the
/// level is 7 by 3 squares of 72 pixels, in the middle of a board of 11 by
/// 7 that is 4 pixels from the left and 376 from the top.
fn square(column: i32, row: i32) -> (i32, i32) {
    (4 + 2 * 72 + column * 72 + 36, 376 + 2 * 72 + row * 72 + 36)
}

#[test]
fn a_finger_moves_the_figure_where_there_are_no_keys() {
    let mut r = Rig::with_root(staged(), FakeConfig::default(), Model::D80);
    r.launch(APP);
    // The panel is a line across the top, and no key is named.
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0", "Swipe to move"]);
    assert!(r.ui.takes_touches(APP, 0));
    // The board is as wide as the upright window allows, under the line
    // with the back button.
    let (bounds, _) = r.ui.drawings(APP).remove(0);
    assert_eq!((bounds.x, bounds.y, bounds.width, bounds.height), (4, 376, 792, 504));
    let start = pieces(&r);
    let swipe = |r: &mut Rig, from: (i32, i32), to: (i32, i32)| {
        r.ui.drag(APP, from, to);
        r.settle();
    };
    let touch = |r: &mut Rig, at: (i32, i32)| {
        r.ui.tap(APP, at.0, at.1);
        r.settle();
    };

    // A swipe, anywhere, is a step the way it went.
    swipe(&mut r, (300, 1000), (420, 1030));
    assert_eq!(panel(&r)[1..3], ["Moves 1", "Pushes 0"]);
    assert_ne!(pieces(&r), start);
    // Into a wall, or too short to be meant: nothing.
    swipe(&mut r, (300, 900), (310, 700));
    swipe(&mut r, (300, 900), (310, 1100));
    swipe(&mut r, (300, 900), (315, 905));
    assert_eq!(panel(&r)[1..3], ["Moves 1", "Pushes 0"]);
    swipe(&mut r, (600, 200), (450, 260));
    assert_eq!(panel(&r)[1..3], ["Moves 2", "Pushes 0"]);
    assert_eq!(pieces(&r), start, "back where it started");

    // A touch beside the figure (it is in the second square of the row)
    // is a step toward the touch; a touch on it is none.
    touch(&mut r, square(1, 1));
    assert_eq!(panel(&r)[1..3], ["Moves 2", "Pushes 0"]);
    touch(&mut r, square(4, 1));
    assert_eq!(panel(&r)[1..3], ["Moves 3", "Pushes 0"]);
    touch(&mut r, square(5, 1));
    assert_eq!(panel(&r)[1..3], ["Moves 4", "Pushes 1"]);
    // Above it is a wall.
    touch(&mut r, square(3, 0));
    assert_eq!(panel(&r)[1..3], ["Moves 4", "Pushes 1"]);
    // The softkeys are as on every phone.
    r.softkey(APP, "Undo");
    assert_eq!(panel(&r)[1..3], ["Moves 3", "Pushes 0"]);

    // Two more steps solve the level; the next one is a softkey, and a
    // level of the list is touched.
    touch(&mut r, square(6, 1));
    swipe(&mut r, (100, 500), (300, 500));
    assert_eq!(panel(&r), ["Level 1 solved!", "Moves: 5", "Pushes: 2", "Fewest pushes: 2"]);
    assert!(!r.ui.takes_touches(APP, 0));
    touch(&mut r, square(6, 1));
    assert_eq!(panel(&r)[0], "Level 1 solved!");
    r.softkey(APP, "Next");
    assert_eq!(panel(&r), ["Level 2", "Moves 0", "Pushes 0", "Swipe to move"]);
    r.softkey(APP, "Levels");
    r.ui.tap_row(APP, 0);
    r.settle();
    assert_eq!(panel(&r), ["Level 1", "Moves 0", "Pushes 0", "Swipe to move"]);
    // The back button is Cancel: from the levels to the game, and from
    // the game out.
    r.softkey(APP, "Levels");
    r.key(APP, Key::Cancel);
    assert_eq!(panel(&r)[0], "Level 1");
    r.key(APP, Key::Cancel);
    assert_eq!(r.ui.top(), None);
}

#[test]
fn drawn_on_the_touchscreen() {
    use opendeskphone_apps::engine::Target;
    use opendeskphone_apps::testutil::{FakePbx, services_with_store};
    use opendeskphone_apps::uishell::UiShell;
    use opendeskphone_apps::{AppManager, ManagerConfig};
    use opendeskphone_state::{Phase, Store, System};
    use opendeskphone_ui::{Hit, Ui, UiConfig};

    // The phone UI on the simulator's display, and the manager with the
    // real app.
    let model = Model::D80;
    let dir = common::temp_dir("boxpusher-phone");
    let mut hal = opendeskphone_hal::Hal::sim(model);
    let sim = hal.sim.clone().unwrap();
    let store = Store::new(model);
    let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { theme: "harbor".into(), ..UiConfig::default() });
    store.update::<System, _>(|s| s.phase = Phase::Running);
    let mut cfg = ManagerConfig::rooted(staged(), &dir.0.join("nvdata"));
    cfg.target = Target::Pulley64;
    let services = services_with_store(&FakePbx::default(), FakeConfig::default(), &FakeStore::default());
    let mut mgr = AppManager::new(cfg, store.clone(), Box::new(UiShell::new(ui.handle(), model)), services).expect("engine");
    mgr.scan();
    let settle = |ui: &mut Ui, mgr: &mut AppManager| {
        for _ in 0..4 {
            ui.pump();
            mgr.pump();
        }
        ui.pump();
    };
    settle(&mut ui, &mut mgr);
    mgr.launch(APP).unwrap();
    settle(&mut ui, &mut mgr);
    assert_eq!(ui.screen_name(), "app:boxpusher");

    let pixels = |ui: &Ui| -> Vec<u32> {
        let c = ui.canvas();
        (0..1280).flat_map(|y| (0..800).map(move |x| (x, y))).map(|(x, y)| c.pixel(x, y)).collect()
    };
    let shot = |ui: &Ui, name: &str| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("box-pusher");
        std::fs::create_dir_all(&dir).unwrap();
        let rgb: Vec<u8> = pixels(ui).iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
        let file = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("D80-{name}.png"))).unwrap());
        let mut enc = png::Encoder::new(file, 800, 1280);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    };
    shot(&ui, "1-play");
    let start = pixels(&ui);
    // The game's own colors: the floor and the walls of the level.
    let count = |p: &[u32], color: u32| p.iter().filter(|c| **c == color).count();
    assert!(count(&start, 0xD9_D0BA) > 5 * 72 * 72 / 2, "the floor");
    assert!(count(&start, 0x5F_6E8C) > 16 * 72 * 72 / 2, "the walls");
    // The phone's back button and the three softkeys are there to touch:
    // no "Exit", which the back button is.
    let back = ui.canvas().hit_rect(Hit::Key(Key::Cancel)).expect("a back button");
    assert_eq!((back.x, back.y, back.w, back.h), (0, 0, 96, 96));
    for n in 1..=3 {
        assert!(ui.canvas().hit_rect(Hit::Key(Key::Soft(n))).is_some(), "softkey {n}");
    }
    assert!(ui.canvas().hit_rect(Hit::Key(Key::Soft(4))).is_none(), "three softkeys");

    // A swipe on the glass moves the figure; a touch beside it pushes the
    // box; one more swipe pushes it home.
    sim.touch_drag((300, 1000), (450, 1010), 4);
    settle(&mut ui, &mut mgr);
    let moved = pixels(&ui);
    assert_ne!(moved, start);
    let (x, y) = square(4, 1);
    sim.touch_tap(x as u16, y as u16);
    settle(&mut ui, &mut mgr);
    assert_ne!(pixels(&ui), moved);
    shot(&ui, "2-pushed");
    sim.touch_drag((100, 300), (300, 300), 2);
    settle(&mut ui, &mut mgr);
    shot(&ui, "3-solved");
    // The back button leaves.
    sim.touch_tap(48, 48);
    settle(&mut ui, &mut mgr);
    assert_ne!(ui.screen_name(), "app:boxpusher");
}
