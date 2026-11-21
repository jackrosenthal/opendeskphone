//! The on-screen keyboard of the touch skin (D80): keys drawn on the
//! screen, which take the place of the dial pad's multi-tap
//! ([`crate::multitap`]) on a phone that has no keys.
//!
//! A screen that takes text keeps a [`Keyboard`] beside its text.  As it
//! draws, it gives the keyboard a rectangle across the lower part of the
//! screen, [`Keyboard::height`] tall; the keyboard draws its keys there and
//! registers each as a region to touch ([`crate::gfx::Canvas::hit`]), a
//! [`Hit::Custom`] with an id of [`HIT_BASE`] or more.  The screen hands
//! every tap to [`Keyboard::edit`], which applies the key to the text and
//! says what the key was, or to [`Keyboard::tap`], which only says.
//!
//! The layout follows the [`InputMode`] of the text:
//!
//! - `Text` and `Password`: the letters in QWERTY rows.  Shift makes the
//!   next letter a capital; a second touch keeps the capitals on, and a
//!   third turns them off.  `?123` turns to a page of digits and symbols,
//!   and `=\<` from there to a second one; between them they have every
//!   printable ASCII character.  `ABC` turns back to the letters.
//! - `Numeric`: a number pad with a decimal point.
//! - `Pin`: a number pad with digits only.
//! - `Dial`: a number pad with `*` and `#`.
//!
//! Every layout has a backspace key and a done key (a check mark).  Each
//! key is at least [`Theme::touch_min`] tall in a rectangle of the height
//! [`Keyboard::height`] gives; the letters are as wide as ten in a row
//! allow, about 70 pixels on the D80.
//!
//! # Embedding
//!
//! ```text
//! struct Form { text: String, cursor: u16, kb: osk::Keyboard }
//!
//! impl Screen for Form {
//!     fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
//!         let th = cx.th;
//!         let (top, keys) = area.split_bottom(osk::Keyboard::height(th));
//!         let field = IRect::new(top.x + th.pad, top.y + th.pad, top.w - 2 * th.pad, th.touch_min);
//!         let mode = self.kb.mode();
//!         widgets::text_field(c, th, field, &self.text, self.cursor as usize, mode, true, false, "", Font::Normal);
//!         self.kb.draw(c, th, keys);
//!     }
//!     fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
//!         match self.kb.edit(hit, &mut self.text, &mut self.cursor, MAX_LEN) {
//!             Some(osk::Press::Done) => self.submit(cx),
//!             // Typed, deleted, or turned a page: drawn again.
//!             Some(_) => Nav::Stay,
//!             // Not a key of the keyboard.
//!             None => Nav::Ignored,
//!         }
//!     }
//! }
//! ```
//!
//! The screen's own [`Hit::Custom`] ids stay below [`HIT_BASE`].  The
//! keyboard does not know the text: a screen with more than one field
//! gives the keyboard the mode of the field that has the focus
//! ([`Keyboard::set_mode`]) and applies the keys to that field.  A screen
//! with a [`crate::multitap::MultiTap`] for keys from elsewhere (the web
//! mirror) resets it on each key of the keyboard.

use crate::appview::{Font, InputMode};
use crate::gfx::{Canvas, HAlign, IRect, Rgb};
use crate::multitap::MultiTap;
use crate::skin::Theme;
use crate::touch::Hit;
use crate::widgets;

/// The least [`Hit::Custom`] id of the keyboard's keys: every id from
/// here up is the keyboard's ([`Keyboard::owns`]).
pub const HIT_BASE: u32 = 0xF000_0000;

/// The bits of an id that are [`HIT_BASE`]'s.
const BASE_MASK: u32 = 0xF000_0000;
/// Ids of the keys that type no character: above every character.
const SPECIAL: u32 = 0x0100_0000;

/// What a key of the keyboard did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// A character, to go in at the cursor.
    Char(char),
    /// The character before the cursor is to go.
    Backspace,
    /// The text is done (the check mark).
    Done,
    /// The keyboard changed itself (shift, or another page of symbols),
    /// and nothing was typed: it is to be drawn again.
    Layout,
}

/// The pages of the QWERTY layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Letters,
    /// Digits and the commonest symbols (`?123`).
    Symbols,
    /// The other symbols (`=\<`).
    More,
}

/// The shift key's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shift {
    Off,
    /// The next letter is a capital.
    Once,
    /// Capitals until shift is touched again.
    Lock,
}

/// A key of the keyboard.  For tests and tools that touch one: its
/// region, while it is shown, is [`Cap::hit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    /// Types a character (a letter as it is shown: upper case while
    /// shift is on).
    Char(char),
    Shift,
    Backspace,
    Done,
    /// Turns to the letters (`ABC`).
    Letters,
    /// Turns to the digits and the commonest symbols (`?123`).
    Symbols,
    /// Turns to the other symbols (`=\<`).
    More,
}

impl Cap {
    /// The region of the key.
    pub fn hit(self) -> Hit {
        Hit::Custom(self.id())
    }

    fn id(self) -> u32 {
        HIT_BASE
            | match self {
                Cap::Char(ch) => ch as u32,
                Cap::Shift => SPECIAL,
                Cap::Backspace => SPECIAL + 1,
                Cap::Done => SPECIAL + 2,
                Cap::Letters => SPECIAL + 3,
                Cap::Symbols => SPECIAL + 4,
                Cap::More => SPECIAL + 5,
            }
    }

    fn from_id(id: u32) -> Option<Cap> {
        if id & BASE_MASK != HIT_BASE {
            return None;
        }
        match id & !BASE_MASK {
            n if n < SPECIAL => char::from_u32(n).map(Cap::Char),
            SPECIAL => Some(Cap::Shift),
            n if n == SPECIAL + 1 => Some(Cap::Backspace),
            n if n == SPECIAL + 2 => Some(Cap::Done),
            n if n == SPECIAL + 3 => Some(Cap::Letters),
            n if n == SPECIAL + 4 => Some(Cap::Symbols),
            n if n == SPECIAL + 5 => Some(Cap::More),
            _ => None,
        }
    }
}

/// A row of the QWERTY layout: each key with its width, in sixtieths of
/// the keyboard's width (a letter is 6).  A row narrower than 60 is
/// centered.
type Row = &'static [(Cap, i32)];

/// The width of a row of the QWERTY layout.
const UNITS: i32 = 60;

const fn ch(c: char) -> (Cap, i32) {
    (Cap::Char(c), 6)
}

const SPACE: (Cap, i32) = (Cap::Char(' '), 24);

const LETTERS: [Row; 4] = [
    &[ch('q'), ch('w'), ch('e'), ch('r'), ch('t'), ch('y'), ch('u'), ch('i'), ch('o'), ch('p')],
    &[ch('a'), ch('s'), ch('d'), ch('f'), ch('g'), ch('h'), ch('j'), ch('k'), ch('l')],
    &[(Cap::Shift, 9), ch('z'), ch('x'), ch('c'), ch('v'), ch('b'), ch('n'), ch('m'), (Cap::Backspace, 9)],
    &[(Cap::Symbols, 12), ch(','), SPACE, ch('.'), (Cap::Done, 12)],
];

const SYMBOLS: [Row; 4] = [
    &[ch('1'), ch('2'), ch('3'), ch('4'), ch('5'), ch('6'), ch('7'), ch('8'), ch('9'), ch('0')],
    &[ch('@'), ch('#'), ch('$'), ch('%'), ch('&'), ch('*'), ch('-'), ch('+'), ch('('), ch(')')],
    &[(Cap::More, 9), ch('!'), ch('"'), ch('\''), ch(':'), ch(';'), ch('/'), ch('?'), (Cap::Backspace, 9)],
    &[(Cap::Letters, 12), ch(','), SPACE, ch('.'), (Cap::Done, 12)],
];

const MORE: [Row; 4] = [
    &[ch('1'), ch('2'), ch('3'), ch('4'), ch('5'), ch('6'), ch('7'), ch('8'), ch('9'), ch('0')],
    &[ch('['), ch(']'), ch('{'), ch('}'), ch('<'), ch('>'), ch('='), ch('^'), ch('_'), ch('~')],
    &[(Cap::Symbols, 9), ch('`'), ch('|'), ch('\\'), ch('\''), ch('"'), ch('?'), ch('!'), (Cap::Backspace, 9)],
    &[(Cap::Letters, 12), ch(','), SPACE, ch('.'), (Cap::Done, 12)],
];

/// The rows of every layout.
const ROWS: i32 = 4;

/// The on-screen keyboard of one text field (see the module's docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyboard {
    mode: InputMode,
    page: Page,
    shift: Shift,
}

impl Keyboard {
    /// A keyboard for text of `mode`, on its first page, lower case.
    pub fn new(mode: InputMode) -> Keyboard {
        Keyboard { mode, page: Page::Letters, shift: Shift::Off }
    }

    /// The mode of the text the keyboard is for.
    pub fn mode(&self) -> InputMode {
        self.mode
    }

    /// Makes the keyboard one for text of `mode` (another field has the
    /// focus); a new mode starts on the first page, lower case.
    pub fn set_mode(&mut self, mode: InputMode) {
        if mode != self.mode {
            *self = Keyboard::new(mode);
        }
    }

    /// The height of the keyboard on the skin of `th`, the same for every
    /// layout: four rows of keys [`Theme::touch_min`] tall at least.
    pub fn height(th: &Theme) -> i32 {
        ROWS * Self::pitch(th) + Self::gap(th)
    }

    /// The space between two keys.
    fn gap(th: &Theme) -> i32 {
        th.px(4)
    }

    /// The distance from a row of keys to the next one.
    fn pitch(th: &Theme) -> i32 {
        th.row_h.max(th.touch_min + Self::gap(th))
    }

    /// Whether a region with `hit` is one of the keyboard's keys.
    pub fn owns(hit: Hit) -> bool {
        matches!(hit, Hit::Custom(id) if Cap::from_id(id).is_some())
    }

    /// A tap on a region with `hit`: what the key did, `None` for a
    /// region that is not one of the keyboard's.
    pub fn tap(&mut self, hit: Hit) -> Option<Press> {
        let Hit::Custom(id) = hit else { return None };
        Some(match Cap::from_id(id)? {
            Cap::Char(c) => {
                if !self.allows(c) {
                    // (A region of another layout: nothing.)
                    return Some(Press::Layout);
                }
                if self.shift == Shift::Once {
                    self.shift = Shift::Off;
                }
                Press::Char(c)
            }
            Cap::Shift => {
                self.shift = match self.shift {
                    Shift::Off => Shift::Once,
                    Shift::Once => Shift::Lock,
                    Shift::Lock => Shift::Off,
                };
                Press::Layout
            }
            Cap::Backspace => Press::Backspace,
            Cap::Done => Press::Done,
            Cap::Letters => self.turn(Page::Letters),
            Cap::Symbols => self.turn(Page::Symbols),
            Cap::More => self.turn(Page::More),
        })
    }

    /// [`Keyboard::tap`], and the key applied to a one-line `text` with
    /// its `cursor` (in characters) and a length of at most `max_len`
    /// characters: a character goes in at the cursor
    /// ([`widgets::edit_insert`]), backspace takes out the one before it
    /// ([`widgets::edit_backspace`]).  What the key was is returned as
    /// `tap` returns it, whether or not the text had room.
    pub fn edit(&mut self, hit: Hit, text: &mut String, cursor: &mut u16, max_len: usize) -> Option<Press> {
        let press = self.tap(hit)?;
        match press {
            Press::Char(c) => {
                widgets::edit_insert(text, cursor, max_len, c);
            }
            Press::Backspace => {
                widgets::edit_backspace(text, cursor, &mut MultiTap::default());
            }
            Press::Done | Press::Layout => {}
        }
        Some(press)
    }

    /// Turns to `page`, lower case.
    fn turn(&mut self, page: Page) -> Press {
        self.page = page;
        self.shift = Shift::Off;
        Press::Layout
    }

    /// Whether the text of the keyboard's mode takes `c`.
    fn allows(&self, c: char) -> bool {
        match self.mode {
            InputMode::Text | InputMode::Password => true,
            InputMode::Numeric => c.is_ascii_digit() || c == '.',
            InputMode::Pin => c.is_ascii_digit(),
            InputMode::Dial => c.is_ascii_digit() || c == '*' || c == '#',
        }
    }

    /// Draws the keyboard in `r` (across the lower part of the screen,
    /// [`Keyboard::height`] tall) and registers its keys as regions.
    pub fn draw(&self, c: &mut Canvas, th: &Theme, r: IRect) {
        // A tray a little darker than the screen, under keys in the
        // softkey buttons' colors.
        c.fill(r, th.input_bg());
        match self.mode {
            InputMode::Text | InputMode::Password => self.draw_qwerty(c, th, r),
            InputMode::Numeric | InputMode::Pin | InputMode::Dial => self.draw_pad(c, th, r),
        }
    }

    fn rows(&self) -> &'static [Row; 4] {
        match self.page {
            Page::Letters => &LETTERS,
            Page::Symbols => &SYMBOLS,
            Page::More => &MORE,
        }
    }

    fn draw_qwerty(&self, c: &mut Canvas, th: &Theme, r: IRect) {
        let gap = Self::gap(th);
        let pitch = (r.h - gap) / ROWS;
        // The left edge of the cell at `u` sixtieths along.
        let x = |u: i32| r.x + gap / 2 + u * (r.w - gap) / UNITS;
        let upper = self.page == Page::Letters && self.shift != Shift::Off;
        for (i, row) in self.rows().iter().enumerate() {
            let y = r.y + gap + i as i32 * pitch;
            let mut u = (UNITS - row.iter().map(|(_, w)| w).sum::<i32>()) / 2;
            for &(cap, w) in row.iter() {
                let cap = match cap {
                    Cap::Char(l) if upper => Cap::Char(l.to_ascii_uppercase()),
                    other => other,
                };
                let key = IRect::new(x(u) + gap / 2, y, x(u + w) - x(u) - gap, pitch - gap);
                self.key(c, th, key, cap, Font::Title);
                u += w;
            }
        }
    }

    /// A number pad: three columns of characters, and a fourth with
    /// backspace and done, each two rows tall.
    fn draw_pad(&self, c: &mut Canvas, th: &Theme, r: IRect) {
        let gap = Self::gap(th);
        let pitch = (r.h - gap) / ROWS;
        let x = |col: i32| r.x + gap / 2 + col * (r.w - gap) / 4;
        let cell = |col: i32, row: i32, rows: i32| {
            IRect::new(x(col) + gap / 2, r.y + gap + row * pitch, x(col + 1) - x(col) - gap, rows * pitch - gap)
        };
        let (left, right) = match self.mode {
            InputMode::Numeric => (Some('.'), None),
            InputMode::Dial => (Some('*'), Some('#')),
            _ => (None, None),
        };
        let digits: [[Option<char>; 3]; 4] = [
            [Some('1'), Some('2'), Some('3')],
            [Some('4'), Some('5'), Some('6')],
            [Some('7'), Some('8'), Some('9')],
            [left, Some('0'), right],
        ];
        for (row, chars) in digits.iter().enumerate() {
            for (col, ch) in chars.iter().enumerate() {
                if let Some(ch) = ch {
                    self.key(c, th, cell(col as i32, row as i32, 1), Cap::Char(*ch), Font::Large);
                }
            }
        }
        self.key(c, th, cell(3, 0, 2), Cap::Backspace, Font::Title);
        self.key(c, th, cell(3, 2, 2), Cap::Done, Font::Title);
    }

    /// One key in `r`: its shape, its label, and its region.
    fn key(&self, c: &mut Canvas, th: &Theme, r: IRect, cap: Cap, font: Font) {
        let rad = th.px(5);
        // Characters in the softkeys' colors; the keys that type none in
        // the line keys'; done, and shift while it is on, in the
        // selection's.
        let (fill, edge, fg): (Rgb, Option<Rgb>, Rgb) = match cap {
            Cap::Char(_) => (th.sk_bg, Some(th.sk_edge), th.sk_fg),
            Cap::Done => (th.sel_bg, None, th.sel_fg),
            Cap::Shift if self.shift != Shift::Off => (th.sel_bg, None, th.sel_fg),
            Cap::Shift | Cap::Backspace | Cap::Letters | Cap::Symbols | Cap::More => (th.tab_bg, None, th.tab_fg),
        };
        match edge {
            Some(edge) => {
                c.fill_round_aa(r, rad, edge);
                c.fill_round_aa(r.inset(th.scale), rad - th.scale, fill);
            }
            None => c.fill_round_aa(r, rad, fill),
        }
        let icon = match cap {
            Cap::Shift => Some("up"),
            Cap::Backspace => Some("backspace"),
            Cap::Done => Some("ok"),
            _ => None,
        };
        if let Some(icon) = icon.and_then(|n| th.icon(n)) {
            c.icon_in(icon, r, fg);
            if cap == Cap::Shift && self.shift == Shift::Lock {
                // Capitals stay on: a bar under the arrow.
                let w = icon.width();
                let y = r.y + (r.h + icon.height()) / 2 + th.px(2);
                c.fill(IRect::new(r.x + (r.w - w) / 2, y, w, th.px(2)), fg);
            }
        } else {
            let label = match cap {
                Cap::Char(' ') => String::new(),
                Cap::Char(l) => l.to_string(),
                Cap::Letters => "ABC".into(),
                Cap::Symbols => "?123".into(),
                Cap::More => "=\\<".into(),
                _ => String::new(),
            };
            let f = match cap {
                Cap::Letters | Cap::Symbols | Cap::More => th.font(Font::Bold),
                _ => th.font(font),
            };
            c.text_in(f, r, &label, fg, HAlign::Center);
        }
        c.hit(r, Hit::Custom(cap.id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_state::Model;

    fn th() -> &'static Theme {
        Theme::for_model(Model::D80)
    }

    /// The keyboard drawn at the bottom of a D80 screen, and the canvas
    /// with its regions.
    fn drawn(kb: &Keyboard) -> Canvas {
        let th = th();
        let mut c = Canvas::new(th.width as u16, th.height as u16, false);
        let r = IRect::new(0, th.height - Keyboard::height(th), th.width, Keyboard::height(th));
        kb.draw(&mut c, th, r);
        c
    }

    /// The region of the key that types `ch`, as drawn.
    fn key_of(c: &Canvas, ch: char) -> Option<Hit> {
        let id = Cap::Char(ch).id();
        c.hits().iter().map(|(_, h)| *h).find(|h| *h == Hit::Custom(id))
    }

    fn special(c: &Canvas, cap: Cap) -> Hit {
        let hit = Hit::Custom(cap.id());
        assert!(c.hit_rect(hit).is_some(), "no {cap:?}");
        hit
    }

    /// Types `s` by touching the keys as a person would: the page with
    /// the character first, and shift for a capital.
    fn type_str(kb: &mut Keyboard, text: &mut String, cursor: &mut u16, s: &str) {
        for want in s.chars() {
            for _ in 0..4 {
                let c = drawn(kb);
                if let Some(hit) = key_of(&c, want) {
                    assert_eq!(kb.edit(hit, text, cursor, 127), Some(Press::Char(want)));
                    break;
                }
                // Not on this page: shift for a capital, else the next page.
                let next = if want.is_ascii_uppercase() && kb.page == Page::Letters {
                    Cap::Shift
                } else {
                    match kb.page {
                        Page::Letters => Cap::Symbols,
                        Page::Symbols => Cap::More,
                        Page::More => Cap::Letters,
                    }
                };
                assert_eq!(kb.tap(special(&c, next)), Some(Press::Layout));
            }
        }
    }

    #[test]
    fn every_printable_ascii_character_can_be_typed() {
        let mut kb = Keyboard::new(InputMode::Password);
        let (mut text, mut cursor) = (String::new(), 0);
        let all: String = (' '..='~').collect();
        type_str(&mut kb, &mut text, &mut cursor, &all);
        assert_eq!(text, all);
        assert_eq!(cursor as usize, all.chars().count());
    }

    #[test]
    fn shift_is_once_then_locked_then_off() {
        let mut kb = Keyboard::new(InputMode::Text);
        let (mut text, mut cursor) = (String::new(), 0);
        let c = drawn(&kb);
        let shift = special(&c, Cap::Shift);
        kb.tap(shift);
        let c = drawn(&kb);
        kb.edit(key_of(&c, 'A').unwrap(), &mut text, &mut cursor, 127);
        // One capital, then lower case.
        let c = drawn(&kb);
        assert!(key_of(&c, 'b').is_some() && key_of(&c, 'B').is_none());
        kb.edit(key_of(&c, 'b').unwrap(), &mut text, &mut cursor, 127);
        // Twice: capitals until shift again.
        kb.tap(shift);
        kb.tap(shift);
        for _ in 0..2 {
            let c = drawn(&kb);
            kb.edit(key_of(&c, 'C').unwrap(), &mut text, &mut cursor, 127);
        }
        kb.tap(shift);
        let c = drawn(&kb);
        kb.edit(key_of(&c, 'd').unwrap(), &mut text, &mut cursor, 127);
        assert_eq!(text, "AbCCd");
    }

    #[test]
    fn backspace_done_and_the_cursor() {
        let mut kb = Keyboard::new(InputMode::Text);
        let (mut text, mut cursor) = ("held".to_string(), 2u16);
        let c = drawn(&kb);
        assert_eq!(kb.edit(key_of(&c, 'x').unwrap(), &mut text, &mut cursor, 127), Some(Press::Char('x')));
        assert_eq!((text.as_str(), cursor), ("hexld", 3));
        assert_eq!(kb.edit(special(&c, Cap::Backspace), &mut text, &mut cursor, 127), Some(Press::Backspace));
        assert_eq!(kb.edit(special(&c, Cap::Backspace), &mut text, &mut cursor, 127), Some(Press::Backspace));
        assert_eq!((text.as_str(), cursor), ("hld", 1));
        // Full: the key is a key still, and the text stays.
        assert_eq!(kb.edit(key_of(&c, 'y').unwrap(), &mut text, &mut cursor, 3), Some(Press::Char('y')));
        assert_eq!(text, "hld");
        assert_eq!(kb.edit(special(&c, Cap::Done), &mut text, &mut cursor, 127), Some(Press::Done));
        // Not the keyboard's.
        assert_eq!(kb.tap(Hit::Custom(7)), None);
        assert_eq!(kb.tap(Hit::Row { list: 0, row: 1 }), None);
        assert!(!Keyboard::owns(Hit::Custom(HIT_BASE - 1)));
        assert!(Keyboard::owns(special(&c, Cap::Done)));
    }

    #[test]
    fn the_pads_have_what_their_mode_takes() {
        for (mode, extra, not) in [
            (InputMode::Numeric, ".", "*#"),
            (InputMode::Pin, "", ".*#"),
            (InputMode::Dial, "*#", "."),
        ] {
            let c = drawn(&Keyboard::new(mode));
            for d in "0123456789".chars().chain(extra.chars()) {
                assert!(key_of(&c, d).is_some(), "{mode:?}: no {d}");
            }
            for d in not.chars().chain("ab".chars()) {
                assert!(key_of(&c, d).is_none(), "{mode:?}: {d}");
            }
            special(&c, Cap::Backspace);
            special(&c, Cap::Done);
            // Large keys: the pad's are as wide as they are tall.
            for (r, _) in c.hits() {
                assert!(r.w >= th().touch_min && r.h >= th().touch_min, "{mode:?}: {r:?}");
            }
        }
        // A character of another layout types nothing into a pad.
        let mut kb = Keyboard::new(InputMode::Pin);
        let (mut text, mut cursor) = (String::new(), 0);
        assert_eq!(kb.edit(Hit::Custom(Cap::Char('a').id()), &mut text, &mut cursor, 127), Some(Press::Layout));
        assert!(text.is_empty());
    }

    #[test]
    fn keys_are_tall_enough_apart_and_within_the_keyboard() {
        let th = th();
        for mode in [InputMode::Text, InputMode::Numeric] {
            for page in [Page::Letters, Page::Symbols, Page::More] {
                let kb = Keyboard { mode, page, shift: Shift::Off };
                let c = drawn(&kb);
                let area = IRect::new(0, th.height - Keyboard::height(th), th.width, Keyboard::height(th));
                let hits = c.hits();
                assert!(hits.len() >= 13);
                for (i, (r, _)) in hits.iter().enumerate() {
                    assert!(r.h >= th.touch_min, "{r:?}");
                    // Ten letters to a row: as wide as that allows.
                    assert!(r.w >= (th.width / 10) * 7 / 8, "{r:?}");
                    assert_eq!(r.intersect(&area), *r);
                    for (o, _) in &hits[i + 1..] {
                        assert!(r.intersect(o).is_empty(), "{r:?} {o:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_new_mode_starts_over() {
        let mut kb = Keyboard::new(InputMode::Text);
        kb.tap(Hit::Custom(Cap::Symbols.id()));
        kb.set_mode(InputMode::Text);
        assert_eq!(kb.page, Page::Symbols);
        kb.set_mode(InputMode::Dial);
        assert_eq!(kb, Keyboard::new(InputMode::Dial));
    }
}
