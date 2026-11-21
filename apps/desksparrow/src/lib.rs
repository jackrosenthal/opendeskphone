//! Desk Sparrow: a sparrow that has to be kept in the air with one key and
//! flown through the openings between pipes.  Select, or any key of the
//! keypad (the digits, star, and pound), starts a game and flaps; each
//! opening is a point; a pipe or the floor ends the game.  The best score
//! is kept in the app's storage.
//!
//! The game has the whole display: there are no softkeys.  Cancel, which
//! the app leaves to the phone, closes it, as does line key 1.
//!
//! The touchscreen has no keys: a finger that comes down anywhere starts
//! a game and flaps, at once and not when it is lifted (the phone's back
//! button, in the corner, leaves), and between games a button under the
//! plate, tapped, shows the leaderboard and takes it away.
//!
//! The leaderboard is the best scores of the PBX's users, each with the
//! date and time it was set, the same on every phone: Down or Up shows it
//! between games, and takes it away again.  It is one shared key of the
//! app's store on the PBX (see [`board`]), which the game reads when it
//! starts and whenever another phone changes it.  A score that belongs on
//! it is first written to the phone's storage, and stays there until the
//! PBX has it: a phone whose PBX keeps no such store, or cannot be
//! reached, plays as any other, and shows the leaderboard as it last read
//! it with its scores added.
//!
//! The game is drawn on a canvas, once for every tick of a repeating timer
//! and in one call, so the phone repaints once a tick.  Between games
//! nothing moves and no timer runs.  The speed of the game is chosen for
//! the screen (see [`game::Rules`]): twenty ticks a second on the color
//! screens and ten on the gray ones, whose panels are slow.  There is no
//! sound.

mod board;
mod game;

use board::Row;
use opendeskphone_app_sdk::canvas::{self, Bitmap, Color, Op, Role};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::StorageLocation::{Local, PbxShared};
use opendeskphone_app_sdk::{Align, Font};
use game::{Game, Pipe, Rules};

/// The keys that start a game and flap: Select and the keypad.  Claimed,
/// so that a digit does not start a number to dial behind the game.
const PLAY_KEYS: [Key; 13] = [
    Key::Select,
    Key::Digit(0),
    Key::Digit(1),
    Key::Digit(2),
    Key::Digit(3),
    Key::Digit(4),
    Key::Digit(5),
    Key::Digit(6),
    Key::Digit(7),
    Key::Digit(8),
    Key::Digit(9),
    Key::Star,
    Key::Pound,
];
/// The keys that show the leaderboard between games, and the start screen
/// or the end of the game again.
const BOARD_KEYS: [Key; 2] = [Key::Down, Key::Up];

/// The file of the best score: a decimal number.
const BEST_FILE: &str = "best";
/// A file an administrator or a test may put beside it: a decimal seed
/// that makes every game the same game.
const SEED_FILE: &str = "seed";
/// How to play and how to leave, said once, on the start screen.
const START_HINT: &str = "Any key flaps.  Cancel leaves.";
/// Said on the start screen and after a game.
const BOARD_HINT: &str = "Down shows the leaderboard.";
/// The same where there are no keys: the leaderboard has a button there.
const TOUCH_START_HINT: &str = "Touch to flap.";
const TOUCH_AGAIN_HINT: &str = "Touch to play again.";
/// The button's label, and its label while the leaderboard shows.
const BOARD_BUTTON: &str = "Leaderboard";
const BACK_BUTTON: &str = "Back";

/// The leaderboard's key among the app's shared keys on the PBX.  Its
/// value is the rows as [`board::format`] writes them.
const BOARD_KEY: &str = "leaderboard";
/// The leaderboard as the phone last read it from the PBX, in the app's
/// storage on the phone, in the same form.
const BOARD_FILE: &str = "leaderboard";
/// The scores the PBX's leaderboard does not have yet, in the same form.
const WAITING_FILE: &str = "waiting";
/// The name of a score whose user the phone does not know yet.
const UNKNOWN_NAME: &str = "This phone";
/// A key just after the end of a game is the last flap of that game, not
/// the start of the next one.
const RESTART_AFTER_MS: u64 = 700;

// ----------------------------------------------------------------- art

/// The sparrow, beak to the right: `o` dark feathers, `b` brown, `c` the
/// pale belly, `k` the beak, `e` the eye.
const SPARROW: [&str; 12] = [
    "......oooo......",
    "....oobbbbo.....",
    "o..obbbbbebo....",
    "oo.obbbbbbbkkk..",
    "ooobbbbbbbbkk...",
    ".oobbbbbbbcco...",
    "..obbbbbbccco...",
    "..obbbbbcccco...",
    "...obbccccco....",
    "....oocccco.....",
    "......oooo......",
    ".......o.o......",
];
/// The wing, drawn over the body: low while gliding, high after a flap.
const WING: [&str; 4] = ["ooooo.", "oooooo", ".ooooo", "..ooo."];
const WING_AT: (i32, i32) = (4, 5);
const WING_UP_AT: (i32, i32) = (4, 2);

/// The digits of the score, five by seven.
const DIGITS: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
    [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
    [0x1F, 0x02, 0x04, 0x06, 0x01, 0x11, 0x0E],
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
];

/// The pixels of `art` marked with `mark`, as a one-bit picture.
fn layer(art: &[&str], mark: u8) -> Vec<u8> {
    let stride = art[0].len().div_ceil(8);
    let mut bits = vec![0u8; stride * art.len()];
    for (y, row) in art.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            if c == mark {
                bits[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    bits
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b }
}

/// `pct` percent of the way from `a` to `b`.
fn mix(a: Color, b: Color, pct: u32) -> Color {
    let ch = |a: u8, b: u8| ((u32::from(a) * (100 - pct) + u32::from(b) * pct + 50) / 100) as u8;
    rgb(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b))
}

/// The colors of the pipes: the skin's own.
struct PipeColors {
    body: Color,
    light: Color,
    dark: Color,
    /// The clamping bands.
    band: Color,
}

/// Everything about how the game looks on this phone.
struct Look {
    rules: Rules,
    /// The window.
    width: i32,
    height: i32,
    /// Size of one pixel of the artwork: 2 on the color screens, 4 on
    /// the touchscreen.
    unit: i32,
    /// The phone has no keys: the game is played by touch.
    touch: bool,
    pipe: PipeColors,
    /// The sparrow's layers, back to front: the mark in the art and the
    /// color.
    feathers: [(u8, Color); 5],
    wing: Color,
    background: Color,
    foreground: Color,
    floor: Color,
    floor_edge: Color,
}

impl Look {
    fn new() -> Look {
        let m = ui::get_metrics();
        let rules = match m.skin {
            Skin::Color => Rules::COLOR,
            Skin::Touch => Rules::TOUCH,
            Skin::LargeGray => Rules::LARGE_GRAY,
            Skin::SmallGray => Rules::SMALL_GRAY,
        };
        let highlight = canvas::theme_color(Role::Highlight);
        let highlight_text = canvas::theme_color(Role::HighlightText);
        let (black, white) = (rgb(0, 0, 0), rgb(255, 255, 255));
        let (pipe, feathers, wing) = if m.color {
            (
                // The skin's blues.
                {
                    let dark = mix(canvas::theme_color(Role::TitleBackground), black, 25);
                    PipeColors {
                        body: highlight,
                        light: mix(highlight, highlight_text, 45),
                        dark,
                        band: dark,
                    }
                },
                [
                    (b'o', rgb(0x6E, 0x45, 0x22)),
                    (b'b', rgb(0xBC, 0x86, 0x4E)),
                    (b'c', rgb(0xF3, 0xE6, 0xCC)),
                    (b'k', rgb(0xF5, 0xA6, 0x23)),
                    (b'e', rgb(0x14, 0x14, 0x14)),
                ],
                rgb(0x6E, 0x45, 0x22),
            )
        } else {
            (
                // Four gray levels: black pipes with a light gray glint
                // and light gray bands.
                {
                    let light = mix(highlight, highlight_text, 66);
                    PipeColors {
                        body: highlight,
                        light,
                        dark: highlight,
                        band: light,
                    }
                },
                [
                    (b'o', black),
                    (b'b', black),
                    (b'c', white),
                    (b'k', black),
                    (b'e', white),
                ],
                rgb(0xAA, 0xAA, 0xAA),
            )
        };
        Look {
            rules,
            width: i32::from(m.width),
            height: i32::from(m.height),
            unit: match m.skin {
                Skin::Touch => 4,
                _ if m.color => 2,
                _ => 1,
            },
            touch: !m.keys,
            pipe,
            feathers,
            wing,
            background: canvas::theme_color(Role::Background),
            foreground: canvas::theme_color(Role::Foreground),
            floor: canvas::theme_color(Role::Border),
            floor_edge: canvas::theme_color(Role::Secondary),
        }
    }
}

// ------------------------------------------------------------- drawing

fn fill(ops: &mut Vec<Op>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w > 0 && h > 0 {
        ops.push(Op::Fill((
            Rect::new(x as i16, y as i16, w as u16, h as u16),
            color,
        )));
    }
}

/// One obstacle: a pipe down from the top of the field and a pipe up from
/// the floor, with the opening between them.  Everything about what an
/// obstacle looks like is here and in [`pipe`].
fn obstacle(ops: &mut Vec<Op>, look: &Look, p: &Pipe) {
    let r = &look.rules;
    if p.x >= look.width || p.x + r.pipe_w <= 0 {
        return;
    }
    pipe(ops, look, p.x, 0, p.gap_y, true);
    pipe(ops, look, p.x, p.gap_y + r.gap, r.height, false);
}

/// A slim pipe from `top` to `bottom` whose open end, at the bottom if
/// `opens_down`, has a flat flange; clamping bands sit along it at
/// even distances from the flange.  It is lit from the left.
fn pipe(ops: &mut Vec<Op>, look: &Look, x: i32, top: i32, bottom: i32, opens_down: bool) {
    let (u, w, c) = (look.unit, look.rules.pipe_w, &look.pipe);
    let len = bottom - top;
    if len <= 0 {
        return;
    }
    // The pipe itself is narrower than its flange.
    let inset = 2 * u;
    fill(ops, x + inset, top, w - 2 * inset, len, c.body);
    fill(ops, x + inset + u, top, 2 * u, len, c.light);
    fill(ops, x + w - inset - 2 * u, top, 2 * u, len, c.dark);

    let flange = (5 * u).min(len);
    // Distance from the open end to a row of the pipe.
    let at = |d: i32, h: i32| if opens_down { bottom - d - h } else { top + d };
    let (band, pitch) = (3 * u, 20 * u);
    let mut d = flange + 9 * u;
    while d + band + u <= len {
        fill(ops, x + u, at(d, band), w - 2 * u, band, c.band);
        fill(ops, x + u, at(d, band), w - 2 * u, u, c.light);
        d += pitch;
    }

    let fy = at(0, flange);
    fill(ops, x, fy, w, flange, c.body);
    fill(ops, x, fy, w, u, c.light);
    fill(ops, x, fy + flange - u, w, u, c.dark);
    fill(ops, x + w - u, fy, u, flange, c.dark);
}

/// The sparrow with its top-left corner at (`x`, `y`).
fn sparrow(ops: &mut Vec<Op>, look: &Look, art: &Art, x: i32, y: i32, wing_up: bool) {
    let bitmap = |bits: &Vec<u8>, dx: i32, dy: i32, w: usize, h: usize, color: Color| {
        Op::Bitmap(Bitmap {
            x: (x + dx * look.unit) as i16,
            y: (y + dy * look.unit) as i16,
            width: w as u16,
            height: h as u16,
            scale: look.unit as u8,
            color,
            bits: bits.clone(),
        })
    };
    for (bits, (_, color)) in art.sparrow.iter().zip(look.feathers) {
        ops.push(bitmap(bits, 0, 0, SPARROW[0].len(), SPARROW.len(), color));
    }
    let (dx, dy) = if wing_up { WING_UP_AT } else { WING_AT };
    ops.push(bitmap(
        &art.wing,
        dx,
        dy,
        WING[0].len(),
        WING.len(),
        look.wing,
    ));
}

/// A plate with a frame: something to read on, whatever is behind.
fn plate(ops: &mut Vec<Op>, look: &Look, x: i32, y: i32, w: i32, h: i32) {
    let u = look.unit;
    fill(ops, x, y, w, h, look.foreground);
    fill(ops, x + u, y + u, w - 2 * u, h - 2 * u, look.background);
}

/// The score during a game: digits on a plate at the top of the field.
fn score(ops: &mut Vec<Op>, look: &Look, value: u32) {
    let text = value.to_string();
    let scale = match look.unit {
        4 => 6,
        2 => 3,
        _ => 2,
    };
    let n = text.len() as i32;
    let (dw, dh, pad) = (5 * scale, 7 * scale, 2 * scale);
    let w = n * dw + (n - 1) * scale + 2 * pad;
    let (x, y) = ((look.width - w) / 2, look.unit);
    plate(ops, look, x, y, w, dh + 2 * pad);
    for (i, d) in text.bytes().enumerate() {
        // Five-bit rows, moved to the top of their bytes.
        let bits = DIGITS[(d - b'0') as usize]
            .iter()
            .map(|row| row << 3)
            .collect();
        ops.push(Op::Bitmap(Bitmap {
            x: (x + pad + i as i32 * (dw + scale)) as i16,
            y: (y + pad) as i16,
            width: 5,
            height: 7,
            scale: scale as u8,
            color: look.foreground,
            bits,
        }));
    }
}

// ----------------------------------------------------------------- app

/// The sparrow's pictures, one for each color.
struct Art {
    sparrow: Vec<Vec<u8>>,
    wing: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// The start screen.
    Ready,
    Flying,
    /// The end of a game, since this time.
    Over(u64),
}

/// A row of the leaderboard as it is shown.
struct Shown {
    name: String,
    /// The date and time the score was set (empty if the row has none).
    when: String,
    score: String,
    /// A row of this phone's user.
    mine: bool,
}

/// What is to read on the plate.
enum Panel<'a> {
    /// Nothing, and no plate: a game is being played.
    None,
    /// A title and two lines.
    Lines([&'a str; 3]),
    /// The leaderboard.
    Board(&'a str, &'a [Shown]),
}

/// The leaderboard's widgets, on a plate.
struct BoardView {
    plate: Rect,
    title: Widget,
    /// Said in place of the rows when there are none.
    note: Widget,
    /// The widgets of each row: a group under the whole line, which the
    /// selection's colors fill between the texts, and the name, the date
    /// and time, and the score.
    rows: Vec<[Widget; 4]>,
    /// The first row: the left of the rows, the width they share, and
    /// the height of each; the others are under it.
    first: Rect,
    /// The width of the score's column, at the right of the line.
    score_w: u16,
}

/// The window: a canvas under three lines of text on a plate, or under
/// the leaderboard on another.
struct Screen {
    look: Look,
    art: Art,
    canvas: Widget,
    title: Widget,
    line: Widget,
    hint: Widget,
    /// The plate behind the three lines.
    plate: Rect,
    board: BoardView,
    /// The touchscreen's button for the leaderboard: its label, and how
    /// wide and high it is.  It is under whichever plate shows.
    button: Option<(Widget, u16, u16)>,
}

impl Screen {
    fn build() -> Screen {
        let look = Look::new();
        let m = ui::get_metrics();
        ui::clear(ROOT);
        let keys: Vec<Key> = PLAY_KEYS.iter().chain(&BOARD_KEYS).copied().collect();
        ui::claim_keys(ROOT, &keys);
        let canvas = canvas::create(ROOT, Rect::new(0, 0, m.width, m.height));

        let touch = m.skin == Skin::Touch;
        let (title_h, pad) = if touch {
            (m.title_height, 16)
        } else if m.color {
            (m.title_height, 8)
        } else {
            (m.line_height, 4)
        };
        let w = if touch {
            600
        } else if m.color {
            300
        } else {
            m.width * 3 / 4
        };
        // A plate `w` wide and `h` high, in the middle of the field with
        // the sparrow that sits on it.
        let plate_of = |w: u16, h: u16| {
            Rect::new(
                ((m.width - w) / 2) as i16,
                ((look.rules.height as u16 - h) / 2 + look.rules.bird_h as u16 / 2) as i16,
                w,
                h,
            )
        };
        let text = |bounds: Rect, font: Font, align: Align| {
            let t = ui::create(WidgetKind::Text, ROOT, bounds);
            ui::set_font(t, font);
            ui::set_align(t, align);
            ui::set_visible(t, false);
            t
        };
        let title_font = if m.color { Font::Title } else { Font::Bold };
        let line_h = m.line_height;
        let plate = plate_of(w, title_h + 2 * line_h + 2 * pad);
        let centered = |p: Rect, y: u16, h: u16, font: Font| {
            text(
                Rect::new(p.x, p.y + y as i16, p.width, h),
                font,
                Align::Center,
            )
        };
        let title = centered(plate, pad, title_h, title_font);
        let line = centered(plate, pad + title_h, line_h, Font::Normal);
        let hint = centered(plate, pad + title_h + line_h, line_h, Font::Normal);

        // The leaderboard: a title, and a line for each row, the name at
        // the left, then the date and time the score was set, and the
        // score at the right.  The date and time are in the small font,
        // left-aligned in a column as wide as the widest of them, which
        // [`Screen::texts`] places; the score is right-aligned in a column
        // as wide as a score of four digits with a gap before it.  The
        // plate is wider than the other to hold them, as wide as the
        // screen on the gray ones.
        let (board_w, score_w) = match m.skin {
            Skin::Color => (440, 44),
            Skin::Touch => (760, 88),
            Skin::LargeGray => (m.width, 35),
            Skin::SmallGray => (m.width, 30),
        };
        let b = plate_of(board_w, title_h + board::SIZE as u16 * line_h + 2 * pad);
        let first = Rect::new(
            b.x + pad as i16,
            b.y + (pad + title_h) as i16,
            board_w - 2 * pad,
            line_h,
        );
        let board = BoardView {
            plate: b,
            title: centered(b, pad, title_h, title_font),
            note: centered(b, pad + title_h + line_h, line_h, Font::Normal),
            rows: (0..board::SIZE as u16)
                .map(|i| {
                    let y = first.y + (i * line_h) as i16;
                    let under = ui::create(WidgetKind::Group, ROOT, Rect { y, ..first });
                    ui::set_visible(under, false);
                    let name_w = first.width - score_w;
                    let cell = |x: i16, w: u16, font: Font, align: Align| {
                        text(Rect::new(x, y, w, line_h), font, align)
                    };
                    [
                        under,
                        cell(first.x, name_w, Font::Normal, Align::Left),
                        cell(first.x, name_w, Font::Small, Align::Left),
                        cell(first.x + name_w as i16, score_w, Font::Normal, Align::Right),
                    ]
                })
                .collect(),
            first,
            score_w,
        };
        let art = Art {
            sparrow: look
                .feathers
                .iter()
                .map(|(mark, _)| layer(&SPARROW, *mark))
                .collect(),
            wing: layer(&WING, b'o'),
        };
        // Where there are no keys, a finger plays, anywhere on the
        // display, and the leaderboard has a button, as high as a row of a
        // list (which is made to be touched).
        let button = (!m.keys).then(|| {
            ui::claim_touch(ROOT, true);
            let (bw, bh) = (m.width * 9 / 20, m.row_height);
            let label = text(Rect::new(0, 0, bw, bh), Font::Bold, Align::Center);
            (label, bw, bh)
        });
        Screen {
            look,
            art,
            canvas,
            title,
            line,
            hint,
            plate,
            board,
            button,
        }
    }

    /// Where the touchscreen's button is: under the plate that shows, in
    /// the middle.
    fn button_rect(&self, board: bool) -> Option<Rect> {
        let (_, w, h) = self.button?;
        let p = if board { &self.board.plate } else { &self.plate };
        Some(Rect::new(
            p.x + (p.width as i16 - w as i16) / 2,
            p.y + (p.height + h / 3) as i16,
            w,
            h,
        ))
    }

    /// Puts the texts of `panel` on screen, and no others.
    fn texts(&self, panel: &Panel) {
        let lines = match panel {
            Panel::Lines(lines) => Some(lines),
            _ => None,
        };
        for (i, w) in [self.title, self.line, self.hint].into_iter().enumerate() {
            if let Some(lines) = lines {
                ui::set_text(w, lines[i]);
            }
            ui::set_visible(w, lines.is_some());
        }
        let b = &self.board;
        let (title, rows) = match panel {
            Panel::Board(title, rows) => (Some(*title), *rows),
            _ => (None, &[][..]),
        };
        if let Some((label, ..)) = self.button {
            if let Some(at) = self.button_rect(title.is_some()) {
                ui::set_bounds(label, at);
            }
            ui::set_text(label, if title.is_some() { BACK_BUTTON } else { BOARD_BUTTON });
            ui::set_visible(label, !matches!(panel, Panel::None));
        }
        if let Some(title) = title {
            ui::set_text(b.title, title);
            ui::set_text(b.note, "No scores yet.");
        }
        ui::set_visible(b.title, title.is_some());
        ui::set_visible(b.note, title.is_some() && rows.is_empty());
        if !rows.is_empty() {
            // The date and time in a column as wide as the widest of
            // them with a digit's width to spare, a digit's width after
            // the name; the name has the rest.
            let digit = canvas::text_size(Font::Normal, "0").width;
            let widest = rows
                .iter()
                .map(|r| canvas::text_size(Font::Small, &r.when).width);
            let when_w = widest.max().unwrap_or(0) + digit;
            let first = b.first;
            let name_w = first.width.saturating_sub(digit + when_w + b.score_w);
            for (i, cells) in b.rows.iter().enumerate().take(rows.len()) {
                let y = first.y + (i as u16 * first.height) as i16;
                let when_x = first.x + (name_w + digit) as i16;
                ui::set_bounds(cells[1], Rect::new(first.x, y, name_w, first.height));
                ui::set_bounds(cells[2], Rect::new(when_x, y, when_w, first.height));
            }
        }
        for (i, cells) in b.rows.iter().enumerate() {
            let row = rows.get(i);
            if let Some(r) = row {
                for (&w, text) in cells[1..].iter().zip([&r.name, &r.when, &r.score]) {
                    ui::set_text(w, text);
                }
            }
            for &w in cells {
                ui::set_highlight(w, row.is_some_and(|r| r.mine));
                ui::set_visible(w, row.is_some());
            }
        }
    }

    /// Draws the whole picture in one call: the game in `state`, under
    /// the leaderboard's plate if `board`.
    fn draw(&self, state: State, game: &Game, board: bool) {
        let look = &self.look;
        let r = &look.rules;
        let mut ops = Vec::with_capacity(96);
        fill(
            &mut ops,
            0,
            r.height,
            look.width,
            look.height - r.height,
            look.floor,
        );
        fill(
            &mut ops,
            0,
            r.height,
            look.width,
            look.unit,
            look.floor_edge,
        );
        let p = if board {
            &self.board.plate
        } else {
            &self.plate
        };
        let (px, py) = (i32::from(p.x), i32::from(p.y));
        match state {
            State::Ready => {
                // The sparrow waits on the plate.
                plate(
                    &mut ops,
                    look,
                    px,
                    py,
                    i32::from(p.width),
                    i32::from(p.height),
                );
                let x = px + (i32::from(p.width) - r.bird_w) / 2;
                sparrow(&mut ops, look, &self.art, x, py - r.bird_h, false);
            }
            State::Flying | State::Over(_) => {
                for pipe in &game.pipes {
                    obstacle(&mut ops, look, pipe);
                }
                let wing_up = game.flapped_at.is_some_and(|t| game.ticks - t < 2);
                sparrow(&mut ops, look, &self.art, r.bird_x, game.bird_y(), wing_up);
                if state == State::Flying {
                    score(&mut ops, look, game.score);
                } else {
                    plate(
                        &mut ops,
                        look,
                        px,
                        py,
                        i32::from(p.width),
                        i32::from(p.height),
                    );
                }
            }
        }
        if state != State::Flying
            && let Some(b) = self.button_rect(board)
        {
            plate(
                &mut ops,
                look,
                i32::from(b.x),
                i32::from(b.y),
                i32::from(b.width),
                i32::from(b.height),
            );
        }
        if let Err(e) = canvas::draw(self.canvas, &ops) {
            opendeskphone_app_sdk::log_warn(&format!("drawing refused: {e}"));
        }
    }
}

struct Sparrow {
    screen: Option<Screen>,
    state: State,
    game: Game,
    best: u32,
    /// Games started, so that two games begun in the same millisecond of
    /// two runs still differ.
    games: u32,
    timer: Option<u32>,
    /// The leaderboard is on screen, over the start screen or the end of
    /// a game.
    viewing: bool,
    /// The leaderboard as the PBX last had it, and the version of its
    /// entry there (0 if the key is not set).
    board: Vec<Row>,
    version: u64,
    /// Scores that belong on the leaderboard and that the PBX does not
    /// have yet.  Also in the app's storage.
    waiting: Vec<Row>,
    /// The request to the PBX's store that is under way.
    asked: Option<u32>,
    /// The finger that is down came down on the leaderboard's button.
    on_button: bool,
}

impl Default for Sparrow {
    fn default() -> Sparrow {
        Sparrow {
            screen: None,
            state: State::Ready,
            game: Game::new(Rules::COLOR, 1),
            best: 0,
            games: 0,
            timer: None,
            viewing: false,
            board: Vec::new(),
            version: 0,
            waiting: Vec::new(),
            asked: None,
            on_button: false,
        }
    }
}

/// Writes a key on the phone, and says in the log if the phone did not
/// keep it.
fn keep(key: &str, value: &[u8], what: &str) {
    let reply = storage::set(Local, key, value, None);
    if !matches!(reply, StorageReply::Done(StorageOutcome::Stored(_))) {
        opendeskphone_app_sdk::log_warn(&format!("{what} was not saved: {reply:?}"));
    }
}

impl Sparrow {
    fn stop_timer(&mut self) {
        if let Some(t) = self.timer.take() {
            timers::cancel(t);
        }
    }

    /// Shows the state: the texts and the picture.
    fn show(&mut self) {
        let viewing = self.viewing && self.state != State::Flying;
        let rows = if viewing { self.shown() } else { Vec::new() };
        let screen = self.screen.get_or_insert_with(Screen::build);
        let over;
        let panel = match self.state {
            State::Flying => Panel::None,
            _ if viewing => Panel::Board(
                if storage::online() {
                    "Leaderboard"
                } else {
                    "Leaderboard (offline)"
                },
                &rows,
            ),
            State::Ready if screen.look.touch => Panel::Lines(["Desk Sparrow", TOUCH_START_HINT, ""]),
            State::Ready => Panel::Lines(["Desk Sparrow", START_HINT, BOARD_HINT]),
            State::Over(_) => {
                over = format!("Score {}   Best {}", self.game.score, self.best);
                let hint = if screen.look.touch { TOUCH_AGAIN_HINT } else { BOARD_HINT };
                Panel::Lines(["Game over", &over, hint])
            }
        };
        screen.texts(&panel);
        screen.draw(self.state, &self.game, viewing);
    }

    /// The leaderboard to show: the PBX's with the scores that it does
    /// not have yet.
    fn shown(&self) -> Vec<Shown> {
        let me = storage::user().map(|u| u.id);
        board::merge(&self.board, &self.waiting)
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let name = match row.name.as_str() {
                    "" if row.id.is_empty() => UNKNOWN_NAME,
                    "" => &row.id,
                    name => name,
                };
                Shown {
                    name: format!("{}. {name}", i + 1),
                    when: row
                        .time
                        .map(|t| l10n::format_time(t, TimeStyle::DateAndTime))
                        .unwrap_or_default(),
                    score: row.score.to_string(),
                    mine: row.id.is_empty() || me.as_ref() == Some(&row.id),
                }
            })
            .collect()
    }

    /// One of the keys that show the leaderboard and take it away.
    fn turn(&mut self) {
        if self.screen.is_some() && self.state != State::Flying {
            self.viewing = !self.viewing;
            self.show();
        }
    }

    /// A finger came down at a place of the display (the touchscreen):
    /// anywhere but on the leaderboard's button between games, it is
    /// what a key of the keypad is.
    fn finger_down(&mut self, at: Point) {
        let viewing = self.viewing && self.state != State::Flying;
        let button = self.screen.as_ref().and_then(|s| s.button_rect(viewing));
        self.on_button = self.state != State::Flying
            && button.is_some_and(|b| {
                let (x, y) = (i32::from(at.x) - i32::from(b.x), i32::from(at.y) - i32::from(b.y));
                x >= 0 && y >= 0 && x < i32::from(b.width) && y < i32::from(b.height)
            });
        if !self.on_button {
            self.play();
        }
    }

    /// The finger tapped: the leaderboard's button, if it came down on
    /// it.  (A finger that slides off the button does nothing.)
    fn tapped(&mut self) {
        if std::mem::take(&mut self.on_button) && self.state != State::Flying {
            self.turn();
        }
    }

    fn keep_waiting(&mut self, waiting: Vec<Row>) {
        if waiting != self.waiting {
            self.waiting = waiting;
            keep(WAITING_FILE, &board::format(&self.waiting), "a score for the leaderboard");
        }
    }

    /// The game's score, if it belongs on the leaderboard as far as the
    /// phone knows it, waits to be written there with the date and time
    /// of now.  Says whether it does.
    fn note_score(&mut self) -> bool {
        let (id, name) = storage::user()
            .map(|u| (u.id, u.name))
            .unwrap_or_default();
        let row = Row::new(self.game.score, &id, &name, Some(timers::wall_time()));
        let waiting = board::unsettled(&self.board, &board::merge(&self.waiting, &[row]));
        let changed = waiting != self.waiting;
        self.keep_waiting(waiting);
        changed
    }

    /// Reads the leaderboard from the PBX.  What comes back is shown, and
    /// the scores that wait are added to it ([`Sparrow::read`]).
    fn ask(&mut self) {
        if let Some(id) = self.asked.take() {
            storage::cancel(id);
        }
        if let StorageReply::Pending(id) = storage::get(PbxShared, BOARD_KEY) {
            self.asked = Some(id);
        }
    }

    /// The PBX's leaderboard is `entry`: read, written by this phone, or
    /// written by another when this one tried.  The scores that wait and
    /// that it lacks are written over exactly this entry; if another
    /// phone writes first, the PBX answers with that phone's entry, and
    /// this is done again with it.
    fn read(&mut self, entry: Option<StorageEntry>) {
        let (rows, version) = match &entry {
            Some(e) => (board::parse(&e.value), e.version),
            None => (Vec::new(), 0),
        };
        if rows != self.board {
            keep(BOARD_FILE, &board::format(&rows), "the leaderboard");
            self.board = rows;
        }
        self.version = version;
        // The phone has reached the PBX: it knows its user now.
        let waiting = match storage::user() {
            Some(u) => board::claim(&self.waiting, &u.id, &u.name),
            None => self.waiting.clone(),
        };
        self.keep_waiting(board::unsettled(&self.board, &waiting));
        let known: Vec<Row> = self
            .waiting
            .iter()
            .filter(|row| !row.id.is_empty())
            .cloned()
            .collect();
        if !known.is_empty() {
            let value = board::format(&board::merge(&self.board, &known));
            if let StorageReply::Pending(id) =
                storage::set(PbxShared, BOARD_KEY, &value, Some(self.version))
            {
                self.asked = Some(id);
            }
        }
        if self.viewing {
            self.show();
        }
    }

    fn answered(&mut self, outcome: StorageOutcome) {
        match outcome {
            StorageOutcome::Entry(entry) | StorageOutcome::Conflict(entry) => self.read(entry),
            StorageOutcome::Stored(entry) => self.read(Some(entry)),
            // The scores wait in the phone's storage, and are written
            // when the PBX is in reach: `Event::StorageOnline`, or the
            // next time the game starts.
            StorageOutcome::Unreachable(_) => {}
            other => opendeskphone_app_sdk::log_warn(&format!("the leaderboard: {other:?}")),
        }
    }

    fn seed(&self) -> u32 {
        opendeskphone_app_sdk::read_string(SEED_FILE)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| (timers::now_ms() as u32) ^ self.games.wrapping_mul(0x9E37_79B9))
    }

    /// A score counts as the best as soon as it is one, and is written
    /// down then: once a game at most.
    fn keep_best(&mut self) {
        if self.game.score > self.best {
            self.best = self.game.score;
            keep(BEST_FILE, self.best.to_string().as_bytes(), "the best score");
        }
    }

    /// One of the keys that play.
    fn play(&mut self) {
        let Some(rules) = self.screen.as_ref().map(|s| s.look.rules) else {
            return;
        };
        match self.state {
            State::Flying => self.game.flap(),
            State::Over(at) if timers::now_ms() < at + RESTART_AFTER_MS => {}
            State::Ready | State::Over(_) => {
                self.games += 1;
                self.game = Game::new(rules, self.seed());
                // The key that starts the game is its first flap.
                self.game.flap();
                self.state = State::Flying;
                self.viewing = false;
                self.stop_timer();
                self.timer = timers::set(rules.tick_ms, true).ok();
                self.show();
            }
        }
    }

    fn tick(&mut self) {
        if self.state != State::Flying {
            return;
        }
        self.game.step();
        if self.game.over {
            self.stop_timer();
            self.keep_best();
            self.state = State::Over(timers::now_ms());
            self.show();
            if self.note_score() {
                self.ask();
            }
        } else if let Some(screen) = &self.screen {
            screen.draw(self.state, &self.game, false);
        }
    }

    /// The game leaves the screen (a call, another screen, or Cancel): a
    /// game being played is over as it stands.
    fn leave(&mut self) {
        self.stop_timer();
        if self.state == State::Flying {
            self.keep_best();
            // Written to the PBX the next time the game starts.
            self.note_score();
        }
        self.state = State::Ready;
        self.viewing = false;
    }
}

impl App for Sparrow {
    fn start(&mut self) {
        // The whole display, from the first frame.
        ui::set_full_screen(true);
        self.best = opendeskphone_app_sdk::read_string(BEST_FILE)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let rows = |file: &str| match storage::get(Local, file) {
            StorageReply::Done(StorageOutcome::Entry(Some(e))) => board::parse(&e.value),
            _ => Vec::new(),
        };
        self.board = rows(BOARD_FILE);
        self.waiting = rows(WAITING_FILE);
        storage::watch(true);
        self.ask();
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => self.show(),
            Event::Background | Event::Exit => self.leave(),
            Event::Timer(id) if self.timer == Some(id) => self.tick(),
            Event::Key(k) if PLAY_KEYS.contains(&k) => self.play(),
            Event::Key(k) if BOARD_KEYS.contains(&k) => self.turn(),
            // The flap is the finger coming down: a game cannot wait for
            // it to be lifted.
            Event::Touch((_, Touch::Down(at))) => self.finger_down(at),
            Event::Touch((_, Touch::Tap(_))) => self.tapped(),
            Event::Storage((id, outcome)) if self.asked == Some(id) => {
                self.asked = None;
                self.answered(outcome);
            }
            // Another phone wrote the leaderboard, or the PBX is in reach
            // (again): read it, and add what waits.
            Event::StorageChanged(change) if change.key == BOARD_KEY => self.ask(),
            Event::StorageOnline => {
                self.ask();
                if self.viewing {
                    self.show();
                }
            }
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Sparrow,
    name: "desksparrow",
    id: "org.opendeskphone.desksparrow",
    display_name: "Desk Sparrow",
    description: "A one-key flying game",
    phone_icon: Some(Icon::AppDesksparrow),
}
