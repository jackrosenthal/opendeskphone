//! Light Up: put lights on the white cells of a grid until every cell is
//! lit, no light shines on another, and every numbered black cell has its
//! number of lights beside it.  The puzzle itself (rules, solver,
//! generator, pictures) is the crate `lightup-game`, in `game/` beside
//! this one; this is the app around it: the screens, the keys, and the
//! puzzle kept on the phone.
//!
//! The arrow keys, or 2, 4, 5, and 6 (up, left, down, and right), move
//! the cursor.  1 puts a light on the cell, or takes the light off a cell
//! that has one; 3 marks the cell with a square (no light here), or takes
//! the square off; each puts its own in place of the other's.  Select
//! does what 1 does.  8 does what the numbers settle as the board stands:
//! squares around each number that has its lights, and lights where a
//! number has just enough cells left.  "New" offers the sizes and
//! difficulties; each puzzle is made on the spot and has exactly one
//! solution.  "Undo" takes the last change to the board back (all of what
//! one press of 8 did), and the one before, as far as the fresh puzzle;
//! the changes are kept with the puzzle.  "Help" is five pages of how to
//! play, each a picture of a rule.  Beside the board is a picture
//! of the keys and what each does, and over it the time spent on the
//! puzzle, which runs while the unsolved puzzle is on the screen.
//!
//! The time of a solved puzzle goes to the table of the best times of
//! its kind (its size and difficulty), which the phones of a PBX share:
//! the app's store on the PBX has a key for each kind (see
//! `lightup_game::scores`).  "Scores" shows a table; there, "Size" and
//! "Level" (or left and right) go to the tables of the other kinds.
//! Each row has the player, when the puzzle was solved, and the time.  A
//! time that cannot be written, because the PBX is out of reach or keeps
//! no such store, is kept on the phone and written when the store comes
//! into reach; until then the table shows it on this phone only.
//!
//! Making a puzzle takes a while on the phone, so it is done in short
//! turns from a timer while "Generating…" is on the screen, and only
//! while the app is on the screen: a call stops it until the app is back.
//! The board is one canvas per row, and the cursor one more over them, so
//! a key draws again only the rows it changed.  The app makes no sound,
//! and like any app it leaves the screen to a call.
//!
//! On a phone without keys (the touchscreen) there is no cursor and no
//! picture of keys.  The board is as wide as the upright window, under a
//! line with the time; a touch on a cell puts a light there, a touch on a
//! light makes it a square, and a touch on a square clears the cell.
//! "Auto" is a softkey there, and does what 8 does.  A line under the
//! board says what a touch does.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::StorageLocation::{Local, PbxShared};
use opendeskphone_app_sdk::{Align, Font, canvas, read_string};
use lightup_game::draw::{
    self, FontSize, Fonts, Layout, Look, Op, Painter, Palette, Rgb, TextFont,
};
use lightup_game::generate::{Difficulty, Generator, Params, Progress, SIZES};
use lightup_game::help::PAGES;
use lightup_game::scores::{Kind, Pending, Table, Win, clock};
use lightup_game::{Act, Game, Grid};

const SK_NEW: u32 = 1;
const SK_EXIT: u32 = 2;
const SK_PLAY: u32 = 3;
const SK_BACK: u32 = 4;
const SK_RETRY: u32 = 5;
const SK_HELP: u32 = 6;
const SK_NEXT: u32 = 7;
const SK_UNDO: u32 = 8;
const SK_SCORES: u32 = 9;
const SK_SIZE: u32 = 10;
const SK_LEVEL: u32 = 11;
/// On a phone without keys: what the key 8 does.
const SK_AUTO: u32 = 12;

/// The softkeys the touchscreen does without: the back button is the way
/// back and out, and a touch chooses a row.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT, SK_BACK, SK_PLAY])
}

/// Sets the window's softkeys: of `all`, those the phone needs.
fn set_keys(all: &[Softkey]) {
    ui::set_softkeys(ROOT, &keys(all));
}

/// What the line under the board says on a phone without keys.
const TOUCH_HINT: &str = "Touch a cell: a light, then a square, then empty.";

/// The key, in the app's storage, that holds the puzzle with its
/// lights and marks, the time spent on it, and the cursor.
const PUZZLE: &str = "puzzle";
/// A file an administrator or a test may put beside it: a decimal seed
/// for the puzzles (the first is made from it, the next from it plus 1,
/// and so on).  Without it the phone's random source seeds each puzzle.
const SEED: &str = "seed";
/// The key, in the app's storage, that holds the times of solved puzzles
/// that the tables on the PBX do not have yet.
const TIMES: &str = "times";

/// How long one turn of making a puzzle lasts, and the pause before the
/// next, in milliseconds: the keys, the screen, and the rest of the
/// phone get their turn in between.
const TURN_MS: u64 = 50;
const PAUSE_MS: u32 = 5;
/// How many grids a puzzle may take.  Of 3,000 puzzles of each size and
/// difficulty none took more than 81; this many means something is wrong,
/// and the user is asked.
const MAX_GRIDS: u32 = 400;

#[derive(Default, PartialEq)]
enum Screen {
    #[default]
    None,
    Choose,
    Generating,
    Failed,
    Play,
    Help,
    Scores,
}

/// The widgets of the playing screen.
struct Board {
    painter: Painter,
    /// A canvas per row, and what it shows.
    rows: Vec<(Widget, Vec<Look>)>,
    /// The canvas over the board: the cursor's, and on a phone without
    /// keys, where there is no cursor, the one that takes the touches.
    cursor: Widget,
    /// Whether the cells are touched (and no cursor is drawn).
    touch: bool,
    cursor_at: Option<(usize, usize)>,
    /// The time spent, over the picture of the keys, and what it says.
    clock: Widget,
    clock_shown: String,
    /// The picture of the keys beside the board (on a phone without
    /// keys, the line that says what a touch does); "Solved!" and the
    /// time take the place of both.
    keys: Widget,
    solved_shown: bool,
    status: Widget,
    time: Widget,
}

/// The rows of a table on the screen, a line for each in a scroll, as a
/// list shows its rows, but with a column more than a list has room for
/// on the gray displays.
struct Lines {
    /// The widgets of each line: the whole line, which has the focus when
    /// the line is selected (the scroll follows the focus), and what the
    /// selection's colors go on: one under it all, which they fill, and a
    /// text for each of [`COLUMNS`].
    lines: Vec<(Widget, [Widget; 4])>,
    /// The line in the selection's colors, which Up and Down move.
    selected: usize,
}

/// The space at either end of a line, as a list leaves it.
const INSET: u16 = 4;

/// The columns of a line: the place and the name, when the puzzle was
/// solved, and the time.  When is in the small font, beside the others.
const COLUMNS: [(Font, Align); 3] = [
    (Font::Normal, Align::Left),
    (Font::Small, Align::Left),
    (Font::Normal, Align::Right),
];

impl Lines {
    /// The rows of a table in `area`, the line of row `selected` in the
    /// selection's colors.
    fn new(area: Rect, table: &Table, selected: usize) -> Lines {
        let m = ui::get_metrics();
        let texts: Vec<[String; 3]> = table
            .rows()
            .iter()
            .enumerate()
            .map(|(n, row)| {
                [
                    format!("{}. {}", n + 1, row.name),
                    l10n::format_time(row.when, TimeStyle::DateAndTime),
                    clock(row.ms),
                ]
            })
            .collect();
        // Whole lines, as a list shows them, and room for the scroll bar
        // if they do not all fit.
        let row_h = m.row_height.max(1);
        let height = area.height / row_h * row_h;
        let bar = if texts.len() as u16 * row_h > height {
            m.scroll_bar_width
        } else {
            0
        };
        let width = area.width.saturating_sub(bar);
        // When and the time are as wide as the widest of them, with a
        // digit's width to spare; the place and the name have the rest,
        // but for a digit's width before when.
        let digit = canvas::text_size(Font::Normal, "0").width;
        let need = |column: usize| {
            let font = COLUMNS[column].0;
            let widths = texts.iter().map(|t| canvas::text_size(font, &t[column]).width);
            widths.max().unwrap_or(0) + digit
        };
        let (when_w, time_w) = (need(1), need(2));
        let name_w = width.saturating_sub(2 * INSET + digit + when_w + time_w);
        let columns = [
            (INSET, name_w),
            (INSET + name_w + digit, when_w),
            (width.saturating_sub(INSET + time_w), time_w),
        ];
        let scroll = ui::create(
            WidgetKind::Scroll,
            ROOT,
            Rect::new(area.x, area.y, area.width, height),
        );
        // On the color displays a hairline is under each line, as under
        // the rows of a list, and the selection's colors leave it be.
        let hairline = m.color.then(|| canvas::theme_color(canvas::Role::Border));
        let line_h = row_h - u16::from(hairline.is_some());
        let lines = texts
            .iter()
            .enumerate()
            .map(|(n, texts)| {
                let y = (n as u16 * row_h) as i16;
                if let Some(color) = hairline {
                    let length = width.saturating_sub(24);
                    let rule = canvas::create(scroll, Rect::new(16, y + line_h as i16, length, 1));
                    let fill = canvas::Op::Fill((Rect::new(0, 0, length, 1), color));
                    if let Err(e) = canvas::draw(rule, &[fill]) {
                        opendeskphone_app_sdk::log_error(&format!("draw: {e}"));
                    }
                }
                let whole = ui::create(WidgetKind::Group, scroll, Rect::new(0, y, width, row_h));
                let under = ui::create(WidgetKind::Text, scroll, Rect::new(0, y, width, line_h));
                let mut colored = [under; 4];
                for (column, text) in texts.iter().enumerate() {
                    let ((x, w), (font, align)) = (columns[column], COLUMNS[column]);
                    let bounds = Rect::new(x as i16, y, w, line_h);
                    colored[column + 1] = text_in(scroll, bounds, font, align, text);
                }
                (whole, colored)
            })
            .collect();
        let lines = Lines { lines, selected };
        lines.show(true);
        lines
    }

    /// Puts the selection's colors on the selected line, or takes them
    /// off.  The scroll follows the focus, which goes with them.
    fn show(&self, on: bool) {
        if let Some((whole, colored)) = self.lines.get(self.selected) {
            for w in colored {
                ui::set_highlight(*w, on);
            }
            if on {
                ui::set_focus(*whole);
            }
        }
    }

    /// Moves the selection up or down a line, around the ends.
    fn step(&mut self, by: isize) {
        let len = self.lines.len().max(1) as isize;
        self.show(false);
        self.selected = (self.selected as isize + by).rem_euclid(len) as usize;
        self.show(true);
    }
}

/// A puzzle being made.
struct Making {
    generator: Generator,
    side: usize,
    difficulty: Difficulty,
    timer: Option<u32>,
    /// Milliseconds worked so far.
    worked: u64,
}

struct LightUp {
    screen: Screen,
    /// The size and difficulty of the puzzle being played.
    side: usize,
    difficulty: Difficulty,
    game: Option<Game>,
    /// The size and difficulty asked for last.
    wanted: (usize, Difficulty),
    /// Milliseconds spent on the puzzle, up to `since`.
    elapsed: u64,
    /// When the clock was started, if it runs.
    since: Option<u64>,
    /// The timer of the next second of the time on the screen.
    tick: Option<u32>,
    making: Option<Making>,
    /// Puzzles made since the app started.
    made: u64,
    board: Option<Board>,
    list: Option<Widget>,
    /// The table on the screen.
    lines: Option<Lines>,
    /// The page of the help on show, and whether it was opened from the
    /// list of sizes (else from the puzzle).
    page: usize,
    help_from_list: bool,
    /// What the puzzle file holds.
    saved: String,
    /// The tables of the best times as the PBX last gave them, by
    /// `Kind::index`.
    tables: Vec<Option<Table>>,
    /// The times the tables on the PBX do not have yet.
    pending: Pending,
    /// The kinds of puzzle whose table the PBX would not write; they are
    /// tried again when the app next starts.
    refused: Vec<Kind>,
    /// The win being written to its table: the kind, the win, and the
    /// request to the PBX under way (reading the table, or writing it).
    writing: Option<(Kind, Win, u32)>,
    /// The table on show, and the request reading it.
    shown: Kind,
    reading: Option<u32>,
    /// Whether the PBX answered when it was last asked for the table on
    /// show (`None` until it does).
    reached: Option<bool>,
    /// Whether the tables were opened from the list of sizes (else from
    /// the puzzle), and the row of the list to come back to.
    scores_from_list: bool,
    row: Option<u32>,
}

impl Default for LightUp {
    fn default() -> LightUp {
        LightUp {
            screen: Screen::None,
            side: SIZES[0],
            difficulty: Difficulty::Easy,
            game: None,
            wanted: (SIZES[0], Difficulty::Easy),
            elapsed: 0,
            since: None,
            tick: None,
            making: None,
            made: 0,
            board: None,
            list: None,
            lines: None,
            page: 0,
            help_from_list: false,
            saved: String::new(),
            tables: vec![None; Kind::COUNT],
            pending: Pending::default(),
            refused: Vec::new(),
            writing: None,
            shown: Kind::new(SIZES[0], Difficulty::Easy),
            reading: None,
            reached: None,
            scores_from_list: false,
            row: None,
        }
    }
}

fn color(c: Rgb) -> canvas::Color {
    canvas::Color {
        r: c.0,
        g: c.1,
        b: c.2,
    }
}

/// The colors of the pictures on this display.
/// The display's fonts, the bold ones first, as the pictures name them
/// and as the phone does.
const FONTS: [(TextFont, Font); 5] = [
    (TextFont::Bold, Font::Bold),
    (TextFont::Title, Font::Title),
    (TextFont::Large, Font::Large),
    (TextFont::Normal, Font::Normal),
    (TextFont::Small, Font::Small),
];

/// The colors of the pictures on this display, and its fonts with the
/// size of their digits: the numbers are drawn in the largest that fits.
fn palette() -> Palette {
    let mut palette = if ui::get_metrics().color {
        Palette::color()
    } else {
        let fg = canvas::theme_color(canvas::Role::Foreground);
        let bg = canvas::theme_color(canvas::Role::Background);
        Palette::gray(Rgb(fg.r, fg.g, fg.b), Rgb(bg.r, bg.g, bg.b))
    };
    palette.fonts = Fonts::new(&FONTS.map(|(font, ui_font)| FontSize {
        font,
        // The widest of the digits, and the tallest.
        width: canvas::text_size(ui_font, "8").width,
        height: canvas::text_size(ui_font, "0123456789").height,
    }));
    palette
}

fn paint(w: Widget, ops: Vec<Op>) {
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
                width,
                height,
                scale,
                bits,
                color: c,
            } => canvas::Op::Bitmap(canvas::Bitmap {
                x,
                y,
                width,
                height,
                scale,
                color: color(c),
                bits: bits.to_vec(),
            }),
            Op::Text {
                x,
                y,
                width,
                height,
                font,
                text,
                color: c,
            } => canvas::Op::Text(canvas::Text {
                box_: Rect::new(x, y, width, height),
                font: FONTS
                    .iter()
                    .find(|f| f.0 == font)
                    .map_or(Font::Normal, |f| f.1),
                color: color(c),
                text: text.to_string(),
            }),
        })
        .collect();
    if let Err(e) = canvas::draw(w, &ops) {
        opendeskphone_app_sdk::log_error(&format!("draw: {e}"));
    }
}

fn text(bounds: Rect, font: Font, align: Align, content: &str) -> Widget {
    text_in(ROOT, bounds, font, align, content)
}

fn text_in(parent: Widget, bounds: Rect, font: Font, align: Align, content: &str) -> Widget {
    let w = ui::create(WidgetKind::Text, parent, bounds);
    ui::set_font(w, font);
    ui::set_align(w, align);
    ui::set_text(w, content);
    w
}

/// Lines of text in the middle of a rectangle.
fn centered(area: Rect, lines: &[&str]) {
    let m = ui::get_metrics();
    let spare = area
        .height
        .saturating_sub(lines.len() as u16 * m.line_height);
    for (n, line) in lines.iter().enumerate() {
        let y = area.y + (spare / 2 + n as u16 * m.line_height) as i16;
        text(
            Rect::new(area.x, y, area.width, m.line_height),
            Font::Normal,
            Align::Center,
            line,
        );
    }
}

/// The phone's user as the tables know them: the id at the PBX and the
/// name to show.  A phone that has not reached the PBX does not know
/// its user.
fn player() -> Option<(String, String)> {
    let user = storage::user()?;
    let name = [&user.name, &user.extension, &user.id]
        .into_iter()
        .find(|n| !n.trim().is_empty())?
        .clone();
    Some((user.id, name))
}

/// The largest board offered on the touchscreen: a cell of a 10x10 board
/// is about 80 pixels across the D80's window, as wide as a fingertip; one
/// of 14x14 is 57, which a finger misses.
const TOUCH_SIDE: usize = 10;

/// The sizes of board offered: on the touchscreen, those whose cells a
/// finger can tell apart.
fn sizes() -> impl Iterator<Item = usize> {
    let touch = !has_keys();
    SIZES.into_iter().filter(move |side| !touch || *side <= TOUCH_SIDE)
}

fn size_name(side: usize) -> String {
    format!("{side}x{side}")
}

fn difficulty_named(name: &str) -> Option<Difficulty> {
    Difficulty::ALL
        .into_iter()
        .find(|d| d.name().eq_ignore_ascii_case(name))
}

impl LightUp {
    /// Reads the puzzle file; what it does not say, or says wrongly, is
    /// no puzzle.
    fn restore(&mut self) {
        self.saved = read_string(PUZZLE).unwrap_or_default();
        let saved = self.saved.clone();
        let field = |name: &str| {
            saved
                .lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
                .unwrap_or("")
        };
        if let Ok(side) = field("size").parse::<usize>()
            && SIZES.contains(&side)
        {
            self.side = side;
        }
        if let Some(d) = difficulty_named(field("difficulty")) {
            self.difficulty = d;
        }
        self.wanted = (self.side, self.difficulty);
        let rows: Vec<&str> = saved
            .lines()
            .filter(|l| !l.is_empty() && !l.contains('='))
            .collect();
        let Some(grid) = Grid::parse(&rows.join("\n")) else {
            return;
        };
        if grid.width() != self.side || grid.height() != self.side {
            return;
        }
        let mut game = Game::new(grid);
        let mut at = field("cursor")
            .split(',')
            .map(|n| n.parse::<usize>().unwrap_or(0));
        game.set_cursor(at.next().unwrap_or(0), at.next().unwrap_or(0));
        game.set_changes(field("undo"));
        self.elapsed = field("elapsed").parse().unwrap_or(0);
        self.game = Some(game);
    }

    /// Writes the puzzle file if it would change.
    fn save(&mut self) {
        let (side, difficulty) = match self.game {
            Some(_) => (self.side, self.difficulty),
            None => self.wanted,
        };
        let mut now = format!(
            "size={side}\ndifficulty={}\n",
            difficulty.name().to_lowercase()
        );
        if let Some(game) = &self.game {
            let (x, y) = game.cursor();
            now.push_str(&format!(
                "elapsed={}\ncursor={x},{y}\nundo={}\n{}",
                self.spent(),
                game.changes_text(),
                game.grid().to_text()
            ));
        }
        if now == self.saved {
            return;
        }
        if let StorageReply::Done(StorageOutcome::Stored(_)) =
            storage::set(Local, PUZZLE, now.as_bytes(), None)
        {
            self.saved = now;
        }
    }

    /// Milliseconds spent on the puzzle so far.
    fn spent(&self) -> u64 {
        self.elapsed + self.since.map_or(0, |t| timers::now_ms().saturating_sub(t))
    }

    /// The clock runs while an unsolved puzzle is on the screen.
    fn run_clock(&mut self, on: bool) {
        let on =
            on && self.screen == Screen::Play && self.game.as_ref().is_some_and(|g| !g.solved());
        if !on {
            self.elapsed = self.spent();
            self.since = None;
            if let Some(tick) = self.tick.take() {
                timers::cancel(tick);
            }
        } else {
            if self.since.is_none() {
                self.since = Some(timers::now_ms());
            }
            self.show_time();
        }
    }

    /// Shows the time spent on the puzzle, and, while the clock runs,
    /// asks to be called when its next second begins.
    fn show_time(&mut self) {
        let spent = self.spent();
        if let Some(board) = &mut self.board {
            let now = clock(spent);
            if board.clock_shown != now {
                ui::set_text(board.clock, &now);
                board.clock_shown = now;
            }
        }
        if let Some(tick) = self.tick.take() {
            timers::cancel(tick);
        }
        if self.since.is_some() {
            self.tick = timers::set((1000 - spent % 1000) as u32, false).ok();
        }
    }

    /// The kind of the puzzle being played.
    fn kind(&self) -> Kind {
        Kind::new(self.side, self.difficulty)
    }

    fn leave_screen(&mut self) {
        self.run_clock(false);
        if let Some(id) = self.reading.take() {
            storage::cancel(id);
        }
        if self.screen == Screen::Scores {
            storage::watch(false);
        }
        self.board = None;
        self.list = None;
        self.lines = None;
        if let Some(timer) = self.making.as_mut().and_then(|m| m.timer.take()) {
            timers::cancel(timer);
        }
    }

    /// The sizes and difficulties to choose from.
    fn show_choose(&mut self) {
        self.leave_screen();
        self.making = None;
        self.screen = Screen::Choose;
        let area = screen("New puzzle");
        let list = ui::create(WidgetKind::TextList, ROOT, area);
        let mut items = Vec::new();
        let mut selected = 0;
        for side in sizes() {
            for d in Difficulty::ALL {
                if (side, d) == self.wanted {
                    selected = items.len() as u32;
                }
                items.push(ListItem::new(format!("{} {}", size_name(side), d.name())));
            }
        }
        // Back from the tables, the row they were opened from.
        if let Some(row) = self.row.take() {
            selected = row.min(items.len() as u32 - 1);
        }
        ui::set_items(list, &items);
        ui::set_selected(list, selected);
        // Back to the puzzle, if there is one; else out.
        let other = if self.game.is_some() {
            Softkey::new(SK_BACK, "Back")
        } else {
            Softkey::new(SK_EXIT, "Exit")
        };
        set_keys(&[
            Softkey::new(SK_PLAY, "Play"),
            Softkey::new(SK_HELP, "Help"),
            other,
            Softkey::new(SK_SCORES, "Scores"),
        ]);
        ui::set_focus(list);
        self.list = Some(list);
    }

    /// Starts making the puzzle of a row of the list.
    fn choose(&mut self, row: usize) {
        let n = Difficulty::ALL.len();
        self.wanted = (
            SIZES[(row / n).min(SIZES.len() - 1)],
            Difficulty::ALL[row % n],
        );
        self.generate();
    }

    fn generate(&mut self) {
        let seed = match read_string(SEED).and_then(|s| s.trim().parse::<u64>().ok())
        {
            Some(seed) => seed.wrapping_add(self.made),
            None => runtime::random(),
        };
        self.made += 1;
        let (side, difficulty) = self.wanted;
        self.making = Some(Making {
            generator: Generator::new(Params::preset(side, difficulty), seed, MAX_GRIDS),
            side,
            difficulty,
            timer: None,
            worked: 0,
        });
        self.show_generating();
    }

    fn show_generating(&mut self) {
        self.leave_screen();
        self.screen = Screen::Generating;
        let name = format!("{} {}", size_name(self.wanted.0), self.wanted.1.name());
        if has_keys() {
            ui::clear(ROOT);
            ui::claim_keys(ROOT, &[Key::Cancel]);
            centered(content_rect(), &[&name, "Generating\u{2026}"]);
        } else {
            // Touch: the kind is the title.
            opendeskphone_app_sdk::nav::note(screen(&name), "Generating\u{2026}");
        }
        set_keys(&[Softkey::new(SK_NEW, "New"), Softkey::new(SK_EXIT, "Exit")]);
        if let Some(making) = &mut self.making {
            making.timer = timers::set(PAUSE_MS, false).ok();
        }
    }

    /// One turn of making the puzzle.
    fn work(&mut self) {
        let Some(making) = &mut self.making else {
            return;
        };
        making.timer = None;
        let start = timers::now_ms();
        let progress = loop {
            let progress = making.generator.step();
            if progress != Progress::Working || timers::now_ms().saturating_sub(start) >= TURN_MS {
                break progress;
            }
        };
        making.worked += timers::now_ms().saturating_sub(start);
        match progress {
            Progress::Working => making.timer = timers::set(PAUSE_MS, false).ok(),
            Progress::Done | Progress::Failed => {
                opendeskphone_app_sdk::log_info(&format!(
                    "{} {}: {} after {} ms, {} grids, {} steps",
                    size_name(making.side),
                    making.difficulty.name(),
                    if progress == Progress::Done {
                        "a puzzle"
                    } else {
                        "no puzzle"
                    },
                    making.worked,
                    making.generator.attempts(),
                    making.generator.steps()
                ));
                let puzzle = making.generator.puzzle().cloned();
                let made = (making.side, making.difficulty);
                self.making = None;
                match puzzle {
                    Some(grid) => {
                        (self.side, self.difficulty) = made;
                        self.game = Some(Game::new(grid));
                        self.elapsed = 0;
                        self.since = None;
                        self.save();
                        self.show_play();
                    }
                    None => self.show_failed(),
                }
            }
        }
    }

    fn show_failed(&mut self) {
        self.leave_screen();
        self.screen = Screen::Failed;
        if has_keys() {
            ui::clear(ROOT);
            ui::claim_keys(ROOT, &[Key::Cancel]);
            centered(content_rect(), &["No puzzle was found.", "Try again."]);
        } else {
            let name = format!("{} {}", size_name(self.wanted.0), self.wanted.1.name());
            opendeskphone_app_sdk::nav::note(screen(&name), "No puzzle was found.  Try again, or choose another.");
        }
        set_keys(&[
            Softkey::new(SK_RETRY, "Retry"),
            Softkey::new(SK_NEW, "New"),
            Softkey::new(SK_EXIT, "Exit"),
        ]);
    }

    fn show_play(&mut self) {
        self.leave_screen();
        let Some(game) = &self.game else {
            return self.show_choose();
        };
        self.screen = Screen::Play;
        ui::clear(ROOT);
        let mut claims = vec![Key::Up, Key::Down, Key::Left, Key::Right, Key::Select, Key::Cancel];
        claims.extend([1, 2, 3, 4, 5, 6, 8].map(Key::Digit));
        ui::claim_keys(ROOT, &claims);
        let m = ui::get_metrics();
        if !m.keys {
            return self.show_play_touch();
        }
        let l = Layout::new(m.width, m.height, self.side);
        let palette = palette();
        let painter = Painter::new(self.side, l.pitch, palette);
        let rows = (0..game.grid().height())
            .map(|row| {
                let (x, y, width, height) = painter.row_bounds(row);
                let w =
                    canvas::create(ROOT, Rect::new(l.board_x + x, l.board_y + y, width, height));
                (w, Vec::new())
            })
            .collect();
        let cursor = canvas::create(
            ROOT,
            Rect::new(l.board_x, l.board_y, l.board_side, l.board_side),
        );

        // The panel beside the board: the time spent and, under it, the
        // picture of the keys, the two in the middle of its height.
        // "Solved!" and the time, when it comes to that, are two lines in
        // the middle.
        let gap = (m.line_height / 2) as i16;
        let (x, width) = (
            l.panel_x + gap,
            l.panel_width.saturating_sub(2 * gap as u16),
        );
        let ink = canvas::theme_color(canvas::Role::Foreground);
        let room = m.height.saturating_sub(4 + m.line_height);
        let keymap = draw::keymap(width, room, palette, Rgb(ink.r, ink.g, ink.b));
        let top = ((m.height - m.line_height - keymap.height) / 2) as i16;
        let shown = clock(self.elapsed);
        let time = Rect::new(x, top, width, m.line_height);
        let clock = text(time, Font::Bold, Align::Center, &shown);
        let (left, top) = (
            x + ((width - keymap.width) / 2) as i16,
            top + m.line_height as i16,
        );
        let keys = canvas::create(ROOT, Rect::new(left, top, keymap.width, keymap.height));
        paint(keys, keymap.ops);
        let top = (m.height.saturating_sub(2 * m.line_height) / 2) as i16;
        let line = |n: u16| Rect::new(x, top + (n * m.line_height) as i16, width, m.line_height);
        self.board = Some(Board {
            painter,
            rows,
            cursor,
            touch: false,
            cursor_at: None,
            clock,
            clock_shown: shown,
            keys,
            solved_shown: false,
            status: text(line(0), Font::Bold, Align::Left, ""),
            time: text(line(1), Font::Normal, Align::Left, ""),
        });
        // Undo first, where it is quickest to reach, and the tables of
        // the best times on the fourth key.  The Cancel key leaves the
        // app; a solved puzzle, which has nothing to take back, has a
        // softkey for that as well (`update` sets its keys).
        set_keys(&[
            Softkey::new(SK_UNDO, "Undo"),
            Softkey::new(SK_NEW, "New"),
            Softkey::new(SK_HELP, "Help"),
            Softkey::new(SK_SCORES, "Scores"),
        ]);
        self.update();
        self.run_clock(true);
    }

    /// The playing screen of a phone without keys, in its upright window:
    /// the title (the puzzle's size and difficulty) with the time at the
    /// right of it, under it the board, as wide as the window allows, and
    /// under that what a touch does; "Solved!" and the time take that
    /// line's place.  The cells are touched ([`LightUp::touched`]): there
    /// is no cursor, and no picture of keys.
    fn show_play_touch(&mut self) {
        screen(&format!("{} {}", size_name(self.side), self.difficulty.name()));
        let Some(game) = &self.game else { return };
        let m = ui::get_metrics();
        let cells = self.side.max(1) as u16;
        let top = m.title_height;
        // Under the board: half a line of space, and two lines.
        let under = 2 * m.line_height + m.line_height / 2;
        let room = m.width.min(m.height.saturating_sub(top + under));
        let pitch = (room.saturating_sub(1) / cells).max(4);
        let side = pitch * cells + 1;
        let (board_x, board_y) = ((m.width.saturating_sub(side) / 2) as i16, top as i16);
        let painter = Painter::new(self.side, pitch, palette());
        let rows = (0..game.grid().height())
            .map(|row| {
                let (x, y, width, height) = painter.row_bounds(row);
                let w = canvas::create(ROOT, Rect::new(board_x + x, board_y + y, width, height));
                (w, Vec::new())
            })
            .collect();
        // Nothing is drawn on this one: it is what a finger touches.
        let cursor = canvas::create(ROOT, Rect::new(board_x, board_y, side, side));
        ui::claim_touch(cursor, true);

        let shown = clock(self.elapsed);
        let gap = m.line_height / 2;
        let time = Rect::new(m.back_width as i16, 0, m.width.saturating_sub(m.back_width + gap), m.title_height);
        let clock = text(time, Font::Bold, Align::Right, &shown);
        let y = board_y + (side + gap) as i16;
        let line = |n: u16| {
            Rect::new(
                gap as i16,
                y + (n * m.line_height) as i16,
                m.width.saturating_sub(2 * gap),
                m.line_height,
            )
        };
        let hint = text(
            Rect::new(gap as i16, y, m.width.saturating_sub(2 * gap), 2 * m.line_height),
            Font::Normal,
            Align::Center,
            TOUCH_HINT,
        );
        ui::set_wrap(hint, true);
        self.board = Some(Board {
            painter,
            rows,
            cursor,
            touch: true,
            cursor_at: None,
            clock,
            clock_shown: shown,
            keys: hint,
            solved_shown: false,
            status: text(line(0), Font::Bold, Align::Center, ""),
            time: text(line(1), Font::Normal, Align::Center, ""),
        });
        // Four, so that there is no "More" page: Undo, "Auto" (what the
        // key 8 does), New, and Help.  The tables of the best times are
        // a softkey of "New" and of the solved puzzle.
        set_keys(&[
            Softkey::new(SK_UNDO, "Undo"),
            Softkey::new(SK_AUTO, "Auto"),
            Softkey::new(SK_NEW, "New"),
            Softkey::new(SK_HELP, "Help"),
        ]);
        self.update();
        self.run_clock(true);
    }

    /// A finger touched the board at a place counted from its corner: the
    /// cell there gets a light; one that has a light gets a square in its
    /// place; and one that has a square is cleared.
    fn touched(&mut self, x: i16, y: i16) {
        let Some(board) = &self.board else { return };
        let pitch = board.painter.pitch().max(1) as i16;
        if x < 0 || y < 0 {
            return;
        }
        // (The last grid line belongs to the last cell.)
        let last = self.side.saturating_sub(1);
        let (cx, cy) = (((x / pitch) as usize).min(last), ((y / pitch) as usize).min(last));
        self.changed(|game| {
            game.set_cursor(cx, cy);
            let i = game.grid().index(cx, cy);
            game.act(if game.grid().has_light(i) {
                Act::Mark
            } else if game.grid().is_marked(i) {
                Act::Clear
            } else {
                Act::Light
            })
        });
    }

    /// Draws the rows that no longer show what the puzzle has, the cursor
    /// if it moved, and the time once the puzzle is solved.
    fn update(&mut self) {
        let (Some(game), Some(board)) = (&self.game, &mut self.board) else {
            return;
        };
        for (row, (canvas, shown)) in board.rows.iter_mut().enumerate() {
            let now = board.painter.row_state(game.grid(), row);
            if *shown != now {
                paint(*canvas, board.painter.row(game.grid(), row));
                *shown = now;
            }
        }
        let at = (!game.solved() && !board.touch).then(|| game.cursor());
        if board.cursor_at != at {
            paint(board.cursor, board.painter.cursor(game));
            board.cursor_at = at;
        }
        if game.solved() && !board.solved_shown {
            // The keys do nothing any more: their picture and the
            // running time make room.
            board.solved_shown = true;
            ui::set_visible(board.keys, false);
            ui::set_visible(board.clock, false);
            ui::set_text(board.status, "Solved!");
            ui::set_text(board.time, &format!("Time {}", clock(self.elapsed)));
            set_keys(&[
                Softkey::new(SK_NEW, "New"),
                Softkey::new(SK_HELP, "Help"),
                Softkey::new(SK_EXIT, "Exit"),
                Softkey::new(SK_SCORES, "Scores"),
            ]);
        }
    }

    /// A page of how to play: a picture of a rule, drawn as the board
    /// draws it, and its sentences beside it.
    fn show_help(&mut self, page: usize) {
        if self.screen != Screen::Help {
            self.help_from_list = self.screen == Screen::Choose;
        }
        // The time does not run meanwhile.
        self.leave_screen();
        self.screen = Screen::Help;
        self.page = page % PAGES.len();
        let area = screen(&format!("How to play ({}/{})", self.page + 1, PAGES.len()));
        ui::claim_keys(
            ROOT,
            &[Key::Up, Key::Down, Key::Left, Key::Right, Key::Select, Key::Cancel],
        );
        let m = ui::get_metrics();
        let shown = &PAGES[self.page];
        let grid = shown.grid();
        if !m.keys {
            // The upright window: the picture, six tenths of the width,
            // and the sentences under it.
            let pitch = m.width * 6 / 10 / grid.width() as u16;
            let (width, height, ops) = draw::picture(&grid, pitch, palette());
            let gap = m.line_height as i16;
            let picture = canvas::create(
                ROOT,
                Rect::new((m.width.saturating_sub(width) / 2) as i16, area.y + gap, width, height),
            );
            paint(picture, ops);
            let y = area.y + 2 * gap + height as i16;
            let words = text(
                Rect::new(
                    gap,
                    y,
                    m.width.saturating_sub(2 * gap as u16),
                    (area.y as u16 + area.height).saturating_sub(y as u16).min(5 * m.line_height),
                ),
                Font::Normal,
                Align::Left,
                shown.words(false),
            );
            ui::set_wrap(words, true);
            set_keys(&[Softkey::new(SK_NEXT, "Next"), Softkey::new(SK_BACK, "Back")]);
            return;
        }
        // The picture takes three tenths of the width.
        let pitch = (m.width * 3 / 10 / grid.width() as u16).max(7);
        let (mut width, mut height, mut ops) = draw::picture(&grid, pitch, palette());
        if !m.color {
            let ink = canvas::theme_color(canvas::Role::Foreground);
            (width, height, ops) = draw::framed(width, height, ops, Rgb(ink.r, ink.g, ink.b));
        }
        let gap = (m.line_height / 2) as i16;
        let top = area.y + (area.height.saturating_sub(height) / 2) as i16;
        let picture = canvas::create(ROOT, Rect::new(gap, top, width, height));
        paint(picture, ops);
        let x = 2 * gap + width as i16;
        let words = text(
            Rect::new(
                x,
                area.y,
                m.width.saturating_sub(x as u16 + gap as u16),
                area.height,
            ),
            Font::Normal,
            Align::Left,
            shown.text,
        );
        ui::set_wrap(words, true);
        set_keys(&[Softkey::new(SK_NEXT, "Next"), Softkey::new(SK_BACK, "Back")]);
    }

    /// Leaves the help or the tables for where they were opened: the
    /// puzzle as it was, or the list of sizes.
    fn back(&mut self) {
        let to_list = match self.screen {
            Screen::Help => self.help_from_list,
            Screen::Scores => self.scores_from_list,
            _ => false,
        };
        if to_list {
            self.show_choose();
        } else {
            self.show_play();
        }
    }

    /// The table of the best times of a kind of puzzle: what the phone
    /// has of it at once, and what the PBX has when it answers.
    fn show_scores(&mut self, kind: Kind) {
        match self.screen {
            Screen::Scores => {}
            Screen::Choose => {
                self.scores_from_list = true;
                self.row = self.list.and_then(ui::selected);
            }
            _ => self.scores_from_list = false,
        }
        // The time does not run meanwhile.
        self.leave_screen();
        self.screen = Screen::Scores;
        self.shown = kind;
        self.reached = None;
        // Told of the times other phones write, the table is read again.
        storage::watch(true);
        self.read_shown();
        self.draw_scores();
    }

    /// Asks the PBX for the table on show.
    fn read_shown(&mut self) {
        if let Some(id) = self.reading.take() {
            storage::cancel(id);
        }
        if let StorageReply::Pending(id) = storage::get(PbxShared, &self.shown.key()) {
            self.reading = Some(id);
        }
    }

    /// Draws the table on show: the rows the PBX last gave, with the
    /// times of this phone that are not written there yet in their
    /// places.
    fn draw_scores(&mut self) {
        let kind = self.shown;
        let title = match self.reached {
            Some(false) => format!("Offline: {}", kind.title()),
            _ => format!("Best times: {}", kind.title()),
        };
        let area = screen(&title);
        ui::claim_keys(ROOT, &[Key::Up, Key::Down, Key::Left, Key::Right, Key::Cancel]);
        self.lines = None;
        let (id, name) = player().unwrap_or((String::new(), "You".to_string()));
        let mut table = self.tables[kind.index()].clone().unwrap_or_default();
        for win in self.pending.of(kind) {
            table.add(win, &id, &name);
        }
        if table.rows().is_empty() {
            centered(
                area,
                &[match self.reached {
                    None => "Loading\u{2026}",
                    Some(true) => "No times yet.",
                    Some(false) => "The PBX cannot be reached.",
                }],
            );
        } else {
            // The player's best row selected, if the table has one.
            let at = table.place(&id).unwrap_or(0);
            self.lines = Some(Lines::new(area, &table, at));
        }
        set_keys(&[
            Softkey::new(SK_SIZE, "Size"),
            Softkey::new(SK_LEVEL, "Level"),
            Softkey::new(SK_BACK, "Back"),
        ]);
    }

    /// A table as the PBX has it now.
    fn learn(&mut self, kind: Kind, table: Table) {
        self.tables[kind.index()] = Some(table);
        if self.screen == Screen::Scores && self.shown == kind {
            self.reached = Some(true);
            self.draw_scores();
        }
    }

    /// Writes the times that wait down, to outlast the app.
    fn keep_pending(&mut self) {
        let kept = if self.pending.is_empty() {
            storage::remove(Local, TIMES, None)
        } else {
            storage::set(Local, TIMES, self.pending.to_text().as_bytes(), None)
        };
        if !matches!(kept, StorageReply::Done(StorageOutcome::Stored(_) | StorageOutcome::Removed)) {
            opendeskphone_app_sdk::log_error(&format!("the times were not kept: {kept:?}"));
        }
    }

    /// The puzzle was solved just now: its time stands, the solved
    /// puzzle is kept, and the time goes to the table of its kind.
    fn solved(&mut self) {
        self.run_clock(false);
        self.update();
        self.save();
        let win = Win {
            ms: self.elapsed,
            when: timers::wall_time(),
        };
        if self.pending.add(self.kind(), win) {
            self.keep_pending();
        }
        self.sync();
    }

    /// Starts writing the next time that waits to its table, unless one
    /// is being written: the table is read, and written with the time in
    /// it over exactly what was read.
    fn sync(&mut self) {
        if self.writing.is_some() {
            return;
        }
        let next = self
            .pending
            .times()
            .iter()
            .find(|(kind, _)| !self.refused.contains(kind));
        if let Some(&(kind, win)) = next
            && let StorageReply::Pending(id) = storage::get(PbxShared, &kind.key())
        {
            self.writing = Some((kind, win, id));
        }
    }

    /// The answer of the PBX's store to a request.
    fn answered(&mut self, id: u32, outcome: StorageOutcome) {
        if self.reading == Some(id) {
            self.reading = None;
            match outcome {
                StorageOutcome::Entry(entry) => {
                    let table = entry.map(|e| Table::parse(&e.value)).unwrap_or_default();
                    self.learn(self.shown, table);
                }
                other => {
                    if !matches!(other, StorageOutcome::Unreachable(_)) {
                        opendeskphone_app_sdk::log_warn(&format!("the table was not read: {other:?}"));
                    }
                    self.reached = Some(false);
                    if self.screen == Screen::Scores {
                        self.draw_scores();
                    }
                }
            }
            return;
        }
        let Some((kind, win, asked)) = self.writing else {
            return;
        };
        if asked != id {
            return;
        }
        self.writing = None;
        match outcome {
            // The table as it is: after a conflict, the one another phone
            // wrote meanwhile.
            StorageOutcome::Entry(current) | StorageOutcome::Conflict(current) => {
                let (mut table, version) = match &current {
                    Some(entry) => (Table::parse(&entry.value), entry.version),
                    None => (Table::default(), 0),
                };
                self.learn(kind, table.clone());
                let Some((player, name)) = player() else {
                    // The time waits until the store is in reach again.
                    opendeskphone_app_sdk::log_warn("the PBX has not said who the user is");
                    return;
                };
                if table.add(win, &player, &name) {
                    let reply = storage::set(PbxShared, &kind.key(), &table.to_bytes(), Some(version));
                    if let StorageReply::Pending(id) = reply {
                        self.writing = Some((kind, win, id));
                    }
                    return;
                }
                // The table has ten times as good, or has this win (it
                // was written, and the answer did not come): there is
                // nothing to write.
                if self.pending.done(kind, win) {
                    self.keep_pending();
                }
            }
            StorageOutcome::Stored(entry) => {
                if self.pending.done(kind, win) {
                    self.keep_pending();
                }
                self.learn(kind, Table::parse(&entry.value));
            }
            // The time waits until the store is in reach.
            StorageOutcome::Unreachable(_) => return,
            other => {
                opendeskphone_app_sdk::log_warn(&format!(
                    "the table {} was not written: {other:?}",
                    kind.key()
                ));
                self.refused.push(kind);
            }
        }
        self.sync();
    }

    fn step(&mut self, dx: i8, dy: i8) {
        if self.game.as_mut().is_some_and(|g| g.move_cursor(dx, dy)) {
            self.update();
        }
    }

    /// Puts a light or a mark on the cell under the cursor, or takes it
    /// off a cell that has it.
    fn toggle(&mut self, act: Act) {
        self.changed(|game| game.toggle(act));
    }

    /// Changes the board, and shows and keeps what came of it.
    fn changed(&mut self, change: impl FnOnce(&mut Game) -> bool) {
        let Some(game) = &mut self.game else { return };
        if !change(game) {
            return;
        }
        if game.solved() {
            self.solved();
        } else {
            self.update();
        }
    }

    /// The user is done: the app leaves the screen and stops.
    fn leave(&mut self) {
        self.run_clock(false);
        self.save();
        runtime::set_exit_after_background(true);
        runtime::background();
    }
}

impl App for LightUp {
    fn start(&mut self) {
        // A call over the puzzle does not end it: the app stays until
        // the user leaves it.
        runtime::set_exit_after_background(false);
        // The times that could not be written when the app last ran.
        self.pending = Pending::parse(&read_string(TIMES).unwrap_or_default());
        self.sync();
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => match self.screen {
                Screen::None => {
                    self.restore();
                    self.show_play();
                }
                Screen::Play => self.run_clock(true),
                Screen::Generating => {
                    if let Some(making) = &mut self.making
                        && making.timer.is_none()
                    {
                        making.timer = timers::set(PAUSE_MS, false).ok();
                    }
                }
                _ => {}
            },
            Event::Background | Event::Exit => {
                // Nothing is worked out while something else has the
                // screen (e.g., a call).
                if let Some(timer) = self.making.as_mut().and_then(|m| m.timer.take()) {
                    timers::cancel(timer);
                }
                self.run_clock(false);
                self.save();
            }
            Event::Timer(id) => {
                if self.tick == Some(id) {
                    self.tick = None;
                    self.show_time();
                } else if self.making.as_ref().is_some_and(|m| m.timer == Some(id)) {
                    self.work();
                }
            }
            Event::Storage((id, outcome)) => self.answered(id, outcome),
            Event::StorageChanged(change) => {
                // Another phone wrote the table on show.
                if self.screen == Screen::Scores
                    && matches!(change.location, PbxShared)
                    && change.key == self.shown.key()
                {
                    self.read_shown();
                }
            }
            Event::StorageOnline => {
                self.sync();
                if self.screen == Screen::Scores {
                    self.read_shown();
                }
            }
            Event::Softkey(SK_SCORES) => {
                // Of the row of the list of sizes, or of the puzzle.
                let kind = match (&self.screen, self.list) {
                    (Screen::Choose, Some(list)) => {
                        Kind::from_index(ui::selected(list).unwrap_or(0) as usize)
                    }
                    _ => self.kind(),
                };
                self.show_scores(kind);
            }
            Event::Softkey(SK_SIZE) => self.show_scores(self.shown.next_size()),
            Event::Softkey(SK_LEVEL) => self.show_scores(self.shown.next_difficulty()),
            Event::Softkey(SK_EXIT) => self.leave(),
            Event::Softkey(SK_NEW) => self.show_choose(),
            Event::Softkey(SK_BACK) => self.back(),
            Event::Softkey(SK_HELP) => self.show_help(0),
            Event::Softkey(SK_UNDO) => {
                if self.screen == Screen::Play && self.game.as_mut().is_some_and(Game::undo) {
                    if self.game.as_ref().is_some_and(Game::solved) {
                        self.solved();
                    } else {
                        self.update();
                    }
                }
            }
            Event::Softkey(SK_AUTO) if self.screen == Screen::Play => {
                self.changed(|game| game.auto())
            }
            Event::Touch((w, Touch::Tap(at)))
                if self.screen == Screen::Play
                    && self.board.as_ref().is_some_and(|b| b.touch && b.cursor == w) =>
            {
                self.touched(at.x, at.y)
            }
            Event::Softkey(SK_NEXT) => self.show_help(self.page + 1),
            Event::Softkey(SK_RETRY) => self.generate(),
            Event::Softkey(SK_PLAY) => {
                if let Some(row) = self.list.and_then(ui::selected) {
                    self.choose(row as usize);
                }
            }
            Event::Widget((_, WidgetEvent::Activated(row))) if self.screen == Screen::Choose => {
                self.choose(row as usize)
            }
            Event::Key(key) => match (key, &self.screen) {
                (Key::Up | Key::Digit(2), Screen::Play) => self.step(0, -1),
                (Key::Down | Key::Digit(5), Screen::Play) => self.step(0, 1),
                (Key::Left | Key::Digit(4), Screen::Play) => self.step(-1, 0),
                (Key::Right | Key::Digit(6), Screen::Play) => self.step(1, 0),
                (Key::Select | Key::Digit(1), Screen::Play) => self.toggle(Act::Light),
                (Key::Digit(3), Screen::Play) => self.toggle(Act::Mark),
                (Key::Digit(8), Screen::Play) => self.changed(|game| game.auto()),
                (Key::Right | Key::Down | Key::Select, Screen::Help) => self.show_help(self.page + 1),
                (Key::Left | Key::Up, Screen::Help) => self.show_help(self.page + PAGES.len() - 1),
                (Key::Cancel, Screen::Help | Screen::Scores) => self.back(),
                (Key::Left, Screen::Scores) => self.show_scores(self.shown.step(-1)),
                (Key::Up | Key::Down, Screen::Scores) => {
                    if let Some(lines) = &mut self.lines {
                        lines.step(if key == Key::Up { -1 } else { 1 });
                    }
                }
                (Key::Right, Screen::Scores) => self.show_scores(self.shown.step(1)),
                (Key::Cancel, Screen::Choose) if self.game.is_some() => self.show_play(),
                (Key::Cancel, _) => self.leave(),
                _ => {}
            },
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: LightUp,
    name: "lightup",
    id: "org.opendeskphone.lightup",
    display_name: "Light Up",
    description: "Light every cell of the grid",
    phone_icon: Some(Icon::AppLightup),
}
