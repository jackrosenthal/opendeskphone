//! The time in large digits ([`timer_face`]): a canvas for each
//! character, in the middle of the place given, and drawn again only when
//! its character changes.

use opendeskphone_app_sdk::canvas::{self, Bitmap, Color, Op, Role};
use opendeskphone_app_sdk::prelude::*;
use std::ops::Range;
use timer_face::{Cell, Face, LEVELS};

struct Shown {
    canvas: Widget,
    ch: char,
    lit: bool,
}

pub struct Digits {
    place: Rect,
    /// The widest text the place shows, which sets the digits' size.
    widest: &'static str,
    height: u16,
    shown: Vec<Shown>,
    /// The characters at `height`.
    cells: Vec<Drawn>,
}

/// A character's cell, and the bitmaps that draw it.
struct Drawn {
    ch: char,
    cell: Cell,
    layers: Vec<(u8, Vec<u8>)>,
}

/// `from` a part of the way to `to`.
fn mix(from: Color, to: Color, part: u8) -> Color {
    let m = |a: u8, b: u8| {
        ((u32::from(a) * u32::from(LEVELS - part) + u32::from(b) * u32::from(part))
            / u32::from(LEVELS)) as u8
    };
    Color {
        r: m(from.r, to.r),
        g: m(from.g, to.g),
        b: m(from.b, to.b),
    }
}

impl Digits {
    /// Roboto on a color display, and the gray displays' bitmap font.
    fn face() -> &'static Face {
        if ui::get_metrics().color {
            &timer_face::ROBOTO
        } else {
            &timer_face::HELVETICA
        }
    }

    /// The height of the digits of `widest`, across the window and at most
    /// `most` high.
    pub fn height(widest: &str, most: u16) -> u16 {
        let width = ui::get_metrics().width;
        Self::face().fit(widest, width - width / 10, most)
    }

    pub fn new(place: Rect, widest: &'static str) -> Digits {
        Digits {
            place,
            widest,
            height: 0,
            shown: Vec::new(),
            cells: Vec::new(),
        }
    }

    fn cell(&mut self, c: char) -> Option<&Drawn> {
        if !self.cells.iter().any(|d| d.ch == c) {
            let cell = Self::face().cell(c, self.height)?;
            let layers = timer_face::layers(&cell);
            self.cells.push(Drawn {
                ch: c,
                cell,
                layers,
            });
        }
        self.cells.iter().find(|d| d.ch == c)
    }

    /// Shows `text`, with its characters in `lit` highlighted.
    pub fn show(&mut self, text: &str, lit: Range<usize>) {
        let p = self.place;
        let height = Self::height(self.widest, p.height).min(Self::face().fit(
            text,
            p.width - p.width / 10,
            p.height,
        ));
        let digit = |c: char| c.is_ascii_digit();
        let same = height == self.height
            && self.shown.len() == text.chars().count()
            && self
                .shown
                .iter()
                .zip(text.chars())
                .all(|(s, c)| digit(s.ch) == digit(c));
        if !same {
            for s in self.shown.drain(..) {
                ui::destroy(s.canvas);
            }
            if height != self.height {
                self.cells.clear();
                self.height = height;
            }
            let mut x = p.x
                + ((u32::from(p.width).saturating_sub(Self::face().width(text, height))) / 2)
                    as i16;
            let y = p.y + (p.height.saturating_sub(height) / 2) as i16;
            for c in text.chars() {
                let Some(width) = self.cell(c).map(|d| d.cell.width) else {
                    continue;
                };
                let canvas = canvas::create(ROOT, Rect::new(x, y, width, height));
                // (Never drawn yet.)
                self.shown.push(Shown {
                    canvas,
                    ch: '\0',
                    lit: false,
                });
                x += width as i16;
            }
        }
        let (ink, back) = (
            canvas::theme_color(Role::Foreground),
            canvas::theme_color(Role::Background),
        );
        let (lit_ink, lit_back) = (
            canvas::theme_color(Role::HighlightText),
            canvas::theme_color(Role::Highlight),
        );
        for (i, c) in text.chars().enumerate() {
            let on = lit.contains(&i);
            let Some(s) = self.shown.get(i) else { break };
            if s.ch == c && s.lit == on {
                continue;
            }
            let canvas = s.canvas;
            let Some(Drawn { cell, layers, .. }) = self.cell(c) else {
                continue;
            };
            let (ink, back) = if on { (lit_ink, lit_back) } else { (ink, back) };
            let mut ops = Vec::new();
            if on {
                ops.push(Op::Fill((Rect::new(0, 0, cell.width, cell.height), back)));
            }
            for (level, bits) in layers {
                ops.push(Op::Bitmap(Bitmap {
                    x: 0,
                    y: 0,
                    width: cell.width,
                    height: cell.height,
                    scale: 1,
                    color: mix(back, ink, *level),
                    bits: bits.clone(),
                }));
            }
            let _ = canvas::draw(canvas, &ops);
            self.shown[i] = Shown {
                canvas,
                ch: c,
                lit: on,
            };
        }
    }
}
