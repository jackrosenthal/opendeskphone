//! Box Pusher: push every box onto a target.  The game itself (rules,
//! levels, pictures) is the crate `boxpusher-game`, in `game/` beside this
//! one; this is the app around it: the screens, the keys, and the progress,
//! kept on the phone and on the PBX.
//!
//! The arrow keys, or 2, 4, 6, and 8, move the figure.  On a touchscreen,
//! which has no keys, a swipe moves it a step the way of the swipe, and so
//! does a touch beside it, toward the touch.  A level's floor,
//! walls, and targets are drawn once on one canvas; the boxes and the
//! figure are on a second canvas over it, the only thing drawn again
//! after a step.  The app makes no sound and uses no timer, and like any
//! app it leaves the screen to a call.
//!
//! The progress is written to the phone's storage, and from there to the
//! user's keys on the PBX, so a phone that was reset, or another phone of
//! the same user, goes on where the user was.  The game never waits for
//! the PBX: it starts with what the phone has, and what the PBX answers
//! is taken up when it comes (see `Progress::reconcile` for which of the
//! two counts).  Without a PBX that keeps app data, the game is as it is
//! with one out of reach: the progress stays on the phone.

use boxpusher_game::draw::{Layout, Op, Painter, Palette, Rgb};
use boxpusher_game::progress::{Known, Progress};
use boxpusher_game::{Dir, Game, LEVELS, Level, Step};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::StorageLocation::{Local, PbxPrivate};
use opendeskphone_app_sdk::{Align, Font, canvas, log_warn, read_string};

/// The least length of a swipe, in pixels.
const SWIPE: i32 = 24;

/// The way a finger went, or the way from the figure to a touch: the
/// direction of the longer of the two distances, if that is at least
/// `least` pixels.
fn toward(dx: i32, dy: i32, least: i32) -> Option<Dir> {
    if dx.abs().max(dy.abs()) < least {
        None
    } else if dx.abs() > dy.abs() {
        Some(if dx > 0 { Dir::Right } else { Dir::Left })
    } else {
        Some(if dy > 0 { Dir::Down } else { Dir::Up })
    }
}

const SK_UNDO: u32 = 1;
const SK_RESTART: u32 = 2;
const SK_LEVELS: u32 = 3;
const SK_EXIT: u32 = 4;
const SK_NEXT: u32 = 5;
const SK_AGAIN: u32 = 6;
const SK_PLAY: u32 = 7;
const SK_BACK: u32 = 8;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out, and a touch plays a level.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT, SK_BACK, SK_PLAY])
}

/// Sets the window's softkeys: of `all`, those the phone needs.
fn set_keys(all: &[Softkey]) {
    ui::set_softkeys(ROOT, &keys(all));
}

/// The key that holds the progress (the level being played, how many
/// levels are solved, and the steps taken in the level so far), in the
/// app's storage on the phone and among the user's keys on the PBX.
const PROGRESS: &str = "progress";
/// The key, in the app's storage, that holds what the PBX was last known
/// to have, and whose it is.
const KNOWN: &str = "pbx";

/// The keys, in the app's storage, of the progress and of what the PBX
/// was known to have for a user who left the phone with progress that the
/// PBX did not have yet.
fn left_keys(user: &str) -> (String, String) {
    let hex: String = user.bytes().map(|b| format!("{b:02x}")).collect();
    (format!("{PROGRESS}-{hex}"), format!("{KNOWN}-{hex}"))
}

#[derive(Default, PartialEq)]
enum Screen {
    #[default]
    None,
    Play,
    Solved,
    Levels,
}

/// The widgets of the playing screen.
struct Board {
    pieces: Widget,
    moves: Widget,
    pushes: Widget,
}

#[derive(Default)]
struct BoxPusher {
    screen: Screen,
    painter: Option<Painter>,
    /// The level being played, counted from 0.
    level: usize,
    /// How many levels are solved: the levels up to this one are open.
    solved: usize,
    game: Option<Game>,
    /// The level whose solved screen is on show.
    finished: usize,
    board: Option<Board>,
    list: Option<Widget>,
    /// What the progress file holds.
    saved: String,
    /// What the PBX was last known to have.
    known: Known,
    /// The request that reads the PBX's progress.
    asked: Option<u32>,
    /// The request that writes the progress to the PBX, and what it
    /// writes.
    writing: Option<(u32, Progress)>,
    /// The player has pressed a key since the game came on screen.
    played: bool,
}

fn color(c: Rgb) -> canvas::Color {
    canvas::Color {
        r: c.0,
        g: c.1,
        b: c.2,
    }
}

fn draw(w: Widget, ops: Vec<Op>) {
    let ops: Vec<canvas::Op> = ops
        .into_iter()
        .map(|op| match op {
            Op::Fill {
                x,
                y,
                width,
                height,
                color: c,
            } => canvas::Op::Fill((Rect::new(x, y, width, height), color(c))),
            Op::Bitmap {
                x,
                y,
                size,
                bits,
                color: c,
            } => canvas::Op::Bitmap(canvas::Bitmap {
                x,
                y,
                width: size,
                height: size,
                scale: 1,
                color: color(c),
                bits: bits.to_vec(),
            }),
        })
        .collect();
    if let Err(e) = canvas::draw(w, &ops) {
        opendeskphone_app_sdk::log_error(&format!("draw: {e}"));
    }
}

fn load(level: usize) -> Game {
    // The levels are checked when the firmware is built (the tests of
    // `boxpusher-game`, which the `tests` check runs).
    Game::new(Level::parse(LEVELS[level].text).expect("a level"))
}

fn text(bounds: Rect, font: Font, align: Align, content: &str) -> Widget {
    let w = ui::create(WidgetKind::Text, ROOT, bounds);
    ui::set_font(w, font);
    ui::set_align(w, align);
    ui::set_text(w, content);
    w
}

impl BoxPusher {
    /// Reads the progress file; what it does not say, or says wrongly, is
    /// the beginning.
    fn restore(&mut self) {
        self.saved = read_string(PROGRESS).unwrap_or_default();
        self.known = Known::parse(&read_string(KNOWN).unwrap_or_default());
        self.take(&Progress::parse(&self.saved));
        if let Some(me) = storage::user() {
            self.change_user(&me.id);
        }
    }

    /// Goes on from `progress`.
    fn take(&mut self, progress: &Progress) {
        self.solved = progress.solved;
        self.level = progress.level;
        let mut game = load(self.level);
        game.replay(&progress.moves);
        if game.solved() {
            game.restart();
        }
        self.game = Some(game);
    }

    /// Where the player is now.
    fn progress(&self) -> Progress {
        Progress {
            level: self.level,
            solved: self.solved,
            moves: self.game.as_ref().map(Game::record).unwrap_or_default(),
        }
    }

    /// Writes the progress file if it would change.
    fn keep(&mut self) {
        // Before the file is read there is nothing to write over it.
        if self.screen == Screen::None {
            return;
        }
        let now = self.progress().text();
        if now == self.saved {
            return;
        }
        let reply = storage::set(Local, PROGRESS, now.as_bytes(), None);
        if let StorageReply::Done(StorageOutcome::Stored(_)) = reply {
            self.saved = now;
        }
    }

    /// Writes the progress down: on the phone, and from there to the PBX.
    fn save(&mut self) {
        self.keep();
        self.send();
    }

    /// Writes the progress file to the PBX, over the entry the phone
    /// knows, if the PBX is in reach and does not have it.  Out of reach,
    /// the file waits for `Event::StorageOnline`.
    fn send(&mut self) {
        if self.screen == Screen::None || self.writing.is_some() || !storage::online() {
            return;
        }
        // Another user's progress is not written to this user's keys: the
        // PBX's answer to `ask` sorts that out first.
        let me = storage::user().map(|u| u.id).unwrap_or_default();
        if me.is_empty() || !(self.known.user.is_empty() || self.known.user == me) {
            return;
        }
        let now = Progress::parse(&self.saved);
        if now == self.known.progress {
            return;
        }
        let reply = storage::set(PbxPrivate, PROGRESS, now.text().as_bytes(), Some(self.known.version));
        let StorageReply::Pending(id) = reply else {
            return;
        };
        // The answer may not come (the game is left, or the PBX goes out
        // of reach): what was sent is kept until it does.
        self.known.sent = Some(now.clone());
        let _ = storage::set(Local, KNOWN, self.known.text().as_bytes(), None);
        self.writing = Some((id, now));
    }

    /// Asks the PBX for the user's progress.
    fn ask(&mut self) {
        if self.screen == Screen::None {
            return;
        }
        if let Some(earlier) = self.asked.take() {
            storage::cancel(earlier);
        }
        if let StorageReply::Pending(id) = storage::get(PbxPrivate, PROGRESS) {
            self.asked = Some(id);
        }
    }

    /// Notes what the PBX has.
    fn know(&mut self, user: &str, version: u64, progress: Progress) {
        let known = Known {
            user: user.to_string(),
            version,
            progress,
            sent: None,
        };
        if known != self.known {
            self.known = known;
            let _ = storage::set(Local, KNOWN, self.known.text().as_bytes(), None);
        }
    }

    /// The progress on the phone is that of the user it last went to the
    /// PBX for.  If the phone has another user now, what the PBX does not
    /// have of the earlier user's is set aside for when that user is back
    /// on this phone, and the game goes on with what this user left here,
    /// or from the beginning.  True if the user changed.
    fn change_user(&mut self, me: &str) -> bool {
        if self.known.user.is_empty() || self.known.user == me {
            return false;
        }
        self.keep();
        let (progress, known) = left_keys(&self.known.user);
        if Progress::parse(&self.saved) != self.known.progress {
            let _ = storage::set(Local, &progress, self.saved.as_bytes(), None);
            let _ = storage::set(Local, &known, self.known.text().as_bytes(), None);
        }
        let (progress, known) = left_keys(me);
        self.saved = read_string(&progress).unwrap_or_default();
        self.known = Known::parse(&read_string(&known).unwrap_or_default());
        self.known.user = me.to_string();
        let _ = storage::set(Local, PROGRESS, self.saved.as_bytes(), None);
        let _ = storage::set(Local, KNOWN, self.known.text().as_bytes(), None);
        let _ = storage::remove(Local, &progress, None);
        let _ = storage::remove(Local, &known, None);
        self.take(&Progress::parse(&self.saved));
        true
    }

    /// The PBX has `entry` for the user: goes on with whichever of the
    /// phone's progress and the PBX's counts, and writes it where it is
    /// missing.
    fn reconcile(&mut self, entry: Option<StorageEntry>) {
        let Some(me) = storage::user() else {
            return;
        };
        let changed = self.change_user(&me.id);
        if changed {
            self.played = false;
        }
        let mine = self.progress();
        let theirs = entry.map(|e| {
            (
                e.version,
                Progress::parse(&String::from_utf8_lossy(&e.value)),
            )
        });
        let next = mine.reconcile(
            &self.known,
            theirs
                .as_ref()
                .map(|(version, progress)| (*version, progress)),
            self.played,
        );
        let (version, theirs) = theirs.unwrap_or_default();
        self.know(&me.id, version, theirs);
        if changed || (next.level, &next.moves) != (mine.level, &mine.moves) {
            self.take(&next);
            self.show_play();
        } else if next.solved != mine.solved {
            self.solved = next.solved;
            if self.screen == Screen::Levels {
                self.show_levels();
            }
        }
        self.save();
    }

    /// The answer to a request to the PBX.
    fn answered(&mut self, id: u32, outcome: StorageOutcome) {
        if self.asked == Some(id) {
            self.asked = None;
            match outcome {
                StorageOutcome::Entry(entry) => self.reconcile(entry),
                // The progress stays on the phone until the PBX is in
                // reach.
                StorageOutcome::Unreachable(_) => {}
                other => log_warn(&format!("the progress was not read: {other:?}")),
            }
        } else if self.writing.as_ref().is_some_and(|w| w.0 == id) {
            let Some((_, sent)) = self.writing.take() else {
                return;
            };
            match outcome {
                StorageOutcome::Stored(entry) => {
                    let me = storage::user().map(|u| u.id).unwrap_or_default();
                    self.know(&me, entry.version, sent);
                    // What was played meanwhile.
                    self.send();
                }
                // Another phone of the user wrote first, or an earlier
                // write whose answer was lost.
                StorageOutcome::Conflict(entry) => self.reconcile(entry),
                // It may have arrived all the same: `ask`, when the PBX
                // is in reach, shows.
                StorageOutcome::Unreachable(_) => {}
                other => log_warn(&format!("the progress was not written: {other:?}")),
            }
        }
    }

    /// Where things go in the window, and the pictures at that size and
    /// in the display's colors.
    fn painter(&mut self) -> (&Painter, Layout) {
        let m = ui::get_metrics();
        // The touchscreen is upright: the panel is a line across the top,
        // as high as a title, beside the phone's back button.
        let layout = if m.skin == Skin::Touch {
            Layout::upright(m.width, m.height, m.title_height)
        } else {
            Layout::new(m.width, m.height)
        };
        let painter = self.painter.get_or_insert_with(|| {
            let palette = if m.color {
                Palette::color()
            } else {
                let fg = canvas::theme_color(canvas::Role::Foreground);
                let bg = canvas::theme_color(canvas::Role::Background);
                Palette::gray(Rgb(fg.r, fg.g, fg.b), Rgb(bg.r, bg.g, bg.b))
            };
            Painter::new(layout.tile, palette)
        });
        (painter, layout)
    }

    fn show_play(&mut self) {
        self.screen = Screen::Play;
        self.list = None;
        // Touch: the level is the title, as screen() draws titles.
        if has_keys() {
            ui::clear(ROOT);
        } else {
            screen(&format!("Level {}", self.level + 1));
        }
        ui::claim_keys(
            ROOT,
            &[
                Key::Up,
                Key::Down,
                Key::Left,
                Key::Right,
                Key::Digit(2),
                Key::Digit(4),
                Key::Digit(6),
                Key::Digit(8),
                Key::Cancel,
            ],
        );
        let m = ui::get_metrics();
        let number = self.level + 1;
        if self.game.is_none() {
            self.game = Some(load(self.level));
        }
        let (_, l) = self.painter();
        let bounds = Rect::new(l.board_x, l.board_y, l.board_width, l.board_height);
        let scenery = canvas::create(ROOT, bounds);
        let pieces = canvas::create(ROOT, bounds);
        if let (Some(painter), Some(game)) = (&self.painter, &self.game) {
            draw(scenery, painter.scenery(game.level()));
        }

        let board = if m.skin == Skin::Touch {
            // The panel: one line, after the back button; the level takes
            // the left of it, and the counts share the rest.
            let x = l.panel_x + m.back_width as i16;
            let width = l.panel_width.saturating_sub(m.back_width);
            let (level, count) = (width * 3 / 8, width * 5 / 16);
            let part = |x: i16, width: u16| Rect::new(x, 0, width, m.title_height);
            let board = Board {
                pieces,
                moves: text(part(x + level as i16, count), Font::Normal, Align::Left, ""),
                pushes: text(part(x + (level + count) as i16, count), Font::Normal, Align::Left, ""),
            };
            // There are no keys: a finger moves the figure, anywhere in
            // the window.
            ui::claim_touch(ROOT, true);
            let below = l.board_y + l.board_height as i16 + m.line_height as i16;
            text(
                Rect::new(0, below, m.width, m.line_height),
                Font::Normal,
                Align::Center,
                "Swipe to move",
            );
            board
        } else {
            // The panel: three lines in the middle of its height.
            let gap = (l.tile / 2) as i16;
            let (x, width) = (l.panel_x + gap, l.panel_width - gap as u16);
            let top = (m.height.saturating_sub(3 * m.line_height) / 2) as i16;
            let line = |n: u16| Rect::new(x, top + (n * m.line_height) as i16, width, m.line_height);
            text(line(0), Font::Bold, Align::Left, &format!("Level {number}"));
            Board {
                pieces,
                moves: text(line(1), Font::Normal, Align::Left, ""),
                pushes: text(line(2), Font::Normal, Align::Left, ""),
            }
        };
        set_keys(&[
            Softkey::new(SK_UNDO, "Undo"),
            Softkey::new(SK_RESTART, "Restart"),
            Softkey::new(SK_LEVELS, "Levels"),
            Softkey::new(SK_EXIT, "Exit"),
        ]);
        self.board = Some(board);
        self.update(true);
    }

    /// Draws the boxes and the figure where they are, and the counts.
    fn update(&mut self, pushes_too: bool) {
        let (Some(painter), Some(game), Some(board)) = (&self.painter, &self.game, &self.board)
        else {
            return;
        };
        draw(board.pieces, painter.pieces(game));
        ui::set_text(board.moves, &format!("Moves {}", game.moves()));
        if pushes_too {
            ui::set_text(board.pushes, &format!("Pushes {}", game.pushes()));
        }
    }

    fn step(&mut self, dir: Dir) {
        let Some(game) = &mut self.game else { return };
        let step = game.step(dir);
        let solved = game.solved();
        match step {
            Step::Blocked => {}
            Step::Moved => self.update(false),
            Step::Pushed if solved => self.show_solved(),
            Step::Pushed => self.update(true),
        }
    }

    /// A touch at a place of the window: a step from the figure toward
    /// it, unless it is on the figure.
    fn touched(&mut self, at: Point) {
        let (_, l) = self.painter();
        let (Some(painter), Some(game)) = (&self.painter, &self.game) else {
            return;
        };
        let (x, y) = painter.corner(game.level(), game.player());
        let half = i32::from(l.tile / 2);
        let dx = i32::from(at.x) - (i32::from(l.board_x) + i32::from(x) + half);
        let dy = i32::from(at.y) - (i32::from(l.board_y) + i32::from(y) + half);
        if let Some(dir) = toward(dx, dy, half + 1) {
            self.step(dir);
        }
    }

    fn show_solved(&mut self) {
        let Some(game) = self.game.take() else { return };
        self.screen = Screen::Solved;
        self.board = None;
        let last = self.level + 1 == LEVELS.len();
        let area = screen(&format!("Level {} solved!", self.level + 1));
        let m = ui::get_metrics();
        let mut lines = vec![
            format!("Moves: {}", game.moves()),
            format!("Pushes: {}", game.pushes()),
            format!("Fewest pushes: {}", LEVELS[self.level].pushes),
        ];
        if last {
            lines.push("That was the last level.".into());
        }
        let spare = area
            .height
            .saturating_sub(lines.len() as u16 * m.line_height);
        let top = area.y + (spare / 2) as i16;
        for (n, line) in lines.iter().enumerate() {
            let y = top + (n as u16 * m.line_height) as i16;
            text(
                Rect::new(0, y, m.width, m.line_height),
                Font::Normal,
                Align::Center,
                line,
            );
        }
        ui::claim_keys(ROOT, &[Key::Select, Key::Cancel]);
        let mut keys = vec![
            Softkey::new(SK_AGAIN, "Again"),
            Softkey::new(SK_LEVELS, "Levels"),
            Softkey::new(SK_EXIT, "Exit"),
        ];
        if !last {
            keys.insert(0, Softkey::new(SK_NEXT, "Next"));
        }
        set_keys(&keys);

        // The next level is open, and it is where the game goes on.
        self.finished = self.level;
        self.solved = self.solved.max(self.level + 1);
        if !last {
            self.level += 1;
        }
        self.save();
    }

    /// Starts a level from its beginning.
    fn play(&mut self, level: usize) {
        self.level = level.min(self.solved).min(LEVELS.len() - 1);
        self.game = Some(load(self.level));
        self.save();
        self.show_play();
    }

    /// The open levels, to choose from.
    fn show_levels(&mut self) {
        self.screen = Screen::Levels;
        self.board = None;
        let area = screen("Levels");
        let list = ui::create(WidgetKind::TextList, ROOT, area);
        let open = (self.solved + 1).min(LEVELS.len());
        let items: Vec<ListItem> = (0..open)
            .map(|i| {
                let state = if i < self.solved { "Solved" } else { "" };
                ListItem::with_detail(format!("Level {}", i + 1), state)
            })
            .collect();
        ui::set_items(list, &items);
        ui::set_selected(list, self.level.min(open - 1) as u32);
        set_keys(&[Softkey::new(SK_PLAY, "Play"), Softkey::new(SK_BACK, "Back")]);
        ui::set_focus(list);
        self.list = Some(list);
    }

    /// The user is done: the app leaves the screen and stops.
    fn leave(&mut self) {
        self.save();
        runtime::set_exit_after_background(true);
        runtime::background();
    }

    /// Leaves the list of levels for the game it was opened from.
    fn back(&mut self) {
        match self.game {
            Some(_) => self.show_play(),
            None => self.play(self.level),
        }
    }
}

impl App for BoxPusher {
    fn start(&mut self) {
        // A call over the game does not end it: the app stays until the
        // user leaves it.
        runtime::set_exit_after_background(false);
    }

    fn handle(&mut self, event: Event) {
        if matches!(
            event,
            Event::Softkey(_) | Event::Key(_) | Event::Widget(_) | Event::Touch(_)
        ) {
            self.played = true;
        }
        match event {
            Event::Foreground => {
                if self.screen == Screen::None {
                    self.restore();
                    self.show_play();
                }
                // The user may have played on another phone meanwhile.
                self.played = false;
                self.ask();
            }
            Event::Storage((id, outcome)) => self.answered(id, outcome),
            Event::StorageOnline => self.ask(),
            Event::Background | Event::Exit => self.save(),
            Event::Softkey(SK_EXIT) => self.leave(),
            Event::Softkey(SK_LEVELS) => self.show_levels(),
            Event::Softkey(SK_BACK) => self.back(),
            Event::Softkey(SK_PLAY) => {
                if let Some(row) = self.list.and_then(ui::selected) {
                    self.play(row as usize);
                }
            }
            Event::Widget((_, WidgetEvent::Activated(row))) if self.screen == Screen::Levels => {
                self.play(row as usize)
            }
            Event::Softkey(SK_NEXT) => self.play(self.level),
            Event::Softkey(SK_AGAIN) => self.play(self.finished),
            Event::Softkey(SK_UNDO) => {
                if self.game.as_mut().is_some_and(Game::undo) {
                    self.update(true);
                }
            }
            Event::Softkey(SK_RESTART) => {
                if let Some(game) = &mut self.game {
                    game.restart();
                    self.update(true);
                }
            }
            Event::Touch((_, Touch::Tap(at))) if self.screen == Screen::Play => self.touched(at),
            Event::Touch((_, Touch::DragEnd((from, to)))) if self.screen == Screen::Play => {
                let (dx, dy) = (i32::from(to.x) - i32::from(from.x), i32::from(to.y) - i32::from(from.y));
                if let Some(dir) = toward(dx, dy, SWIPE) {
                    self.step(dir);
                }
            }
            Event::Key(key) => match (key, &self.screen) {
                (Key::Up | Key::Digit(2), Screen::Play) => self.step(Dir::Up),
                (Key::Down | Key::Digit(8), Screen::Play) => self.step(Dir::Down),
                (Key::Left | Key::Digit(4), Screen::Play) => self.step(Dir::Left),
                (Key::Right | Key::Digit(6), Screen::Play) => self.step(Dir::Right),
                (Key::Select, Screen::Solved) => self.play(self.level),
                (Key::Cancel, Screen::Levels) => self.back(),
                (Key::Cancel, _) => self.leave(),
                _ => {}
            },
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: BoxPusher,
    name: "boxpusher",
    id: "org.opendeskphone.boxpusher",
    display_name: "Box Pusher",
    description: "Push every box onto a target",
    phone_icon: Some(Icon::AppBoxpusher),
}
