//! Touch (the touch skin, D80): the dial pad, and the buttons that the
//! idle screen, the dialer, and the call screen are made of.
//!
//! The dial pad's keys are the digit keys of a phone that has them: each
//! is a region that is its key ([`Hit::Key`]), so a digit touched goes the
//! way a digit pressed goes (the dialer's dial plan and tones, DTMF in a
//! call).

use super::*;
use crate::gfx::Rgb;

/// The inset of the touch screens' content from the screen's two sides.
pub(crate) const MARGIN: i32 = 24;

/// The keys of the dial pad, row by row, with the letters printed on a
/// phone's keys.
const KEYS: [(Key, &str, &str); 12] = [
    (Key::Digit(1), "1", ""),
    (Key::Digit(2), "2", "ABC"),
    (Key::Digit(3), "3", "DEF"),
    (Key::Digit(4), "4", "GHI"),
    (Key::Digit(5), "5", "JKL"),
    (Key::Digit(6), "6", "MNO"),
    (Key::Digit(7), "7", "PQRS"),
    (Key::Digit(8), "8", "TUV"),
    (Key::Digit(9), "9", "WXYZ"),
    (Key::Star, "*", ""),
    (Key::Digit(0), "0", ""),
    (Key::Pound, "#", ""),
];

/// The height of a dial pad whose keys are `key_h` tall.
pub(crate) fn pad_h(th: &Theme, key_h: i32) -> i32 {
    4 * key_h + 3 * th.pad
}

/// Draws the dial pad in `r`: four rows of three keys, `th.pad` apart.
/// Each key is a region that is its key.
pub(crate) fn pad(c: &mut Canvas, th: &Theme, r: IRect) {
    let gap = th.pad;
    let (kw, kh) = ((r.w - 2 * gap) / 3, (r.h - 3 * gap) / 4);
    let big = th.font(Font::Large);
    let small = th.font(Font::Small);
    let rad = th.px(6);
    for (i, (key, digit, letters)) in KEYS.iter().enumerate() {
        let k = IRect::new(r.x + (i % 3) as i32 * (kw + gap), r.y + (i / 3) as i32 * (kh + gap), kw, kh);
        c.fill_round_aa(k, rad, th.sk_edge);
        c.fill_round_aa(k.inset(2), rad - 2, th.sk_bg);
        // The digit sits a little above the middle, over where the
        // letters are (or would be), so that the digits of a row line up.
        let block = big.height() + small.height() - th.px(4);
        let top = k.y + (k.h - block) / 2;
        c.text_in(big, IRect::new(k.x, top, k.w, big.height()), digit, th.sk_fg, HAlign::Center);
        if !letters.is_empty() {
            c.text_in(small, IRect::new(k.x, top + block - small.height(), k.w, small.height()), letters, th.secondary, HAlign::Center);
        }
        c.hit(k, Hit::Key(*key));
    }
}

/// The name of the dial pad's icon for [`control`]: not one of the skin's
/// icons (those are the apps' too), but drawn here.
pub(crate) const KEYPAD: &str = "keypad";

/// The dial pad's icon, `th.icon` pixels each way from (`x`, `y`): three
/// rows of three dots, and one under the middle.
fn keypad_icon(c: &mut Canvas, th: &Theme, x: i32, y: i32, fg: Rgb) {
    let size = th.icon;
    let dot = size * 2 / 11;
    let pitch = (size - dot) / 3;
    let left = x + (size - 2 * pitch - dot) / 2;
    for (col, row) in (0..3).flat_map(|row| (0..3).map(move |col| (col, row))).chain([(1, 3)]) {
        c.fill_round_aa(IRect::new(left + col * pitch, y + row * pitch, dot, dot), dot / 2, fg);
    }
}

/// How a [`control`] looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    /// A button like the softkeys'.
    Plain,
    /// What the button stands for is on (muted, the path in use, held):
    /// the button is drawn in the label's color, and its label in the
    /// button's.
    On,
    /// Answer, call: green.
    Go,
    /// End, reject: red.
    Stop,
    /// On, and to be noticed (the microphone is muted): the warning color.
    Warn,
    /// Nothing to do now: dimmed, and not a region.
    Off,
}

/// A button of the touch screens in `r`: `icon` (one of the skin's, or
/// [`KEYPAD`]; none if empty or unknown) and `label` (none if empty), the icon above the label where
/// the button is tall enough for both and beside it where it is not.  It
/// is a region that is `hit`, unless its look is [`Look::Off`].
pub(crate) fn control(c: &mut Canvas, th: &Theme, r: IRect, icon: &str, label: &str, look: Look, hit: Hit) {
    let dim = crate::gfx::mix(th.sk_bg, th.sk_fg, 3);
    let (fill, edge, fg): (Rgb, Option<Rgb>, Rgb) = match look {
        // (The roles of every screen's buttons.)
        Look::Plain => widgets::ButtonStyle::Secondary.colors(th),
        Look::On => widgets::ButtonStyle::On.colors(th),
        Look::Go => widgets::ButtonStyle::Call.colors(th),
        Look::Stop => widgets::ButtonStyle::Destructive.colors(th),
        Look::Warn => (th.warn, None, idle::on_warn(th)),
        Look::Off => (th.sk_bg, Some(crate::gfx::mix(th.sk_bg, th.sk_edge, 3)), dim),
    };
    let rad = th.px(6);
    match edge {
        Some(edge) => {
            c.fill_round_aa(r, rad, edge);
            c.fill_round_aa(r.inset(2), rad - 2, fill);
        }
        None => c.fill_round_aa(r, rad, fill),
    }
    let inner = r.inset_xy(th.px(4), 0);
    // The smaller font for a label the usual one has no room for.
    let font_for = |w: i32| {
        let f = th.font(Font::Bold);
        if f.width(label) > w { th.font(Font::Small) } else { f }
    };
    // The icon's size, and how it is drawn: one of the skin's, or the
    // dial pad's, which is drawn here.
    let glyph: Option<(i32, i32)> = match icon {
        KEYPAD => Some((th.icon, th.icon)),
        name => th.icon(name).map(|i| (i.width(), i.height())),
    };
    let draw = |c: &mut Canvas, x: i32, y: i32| match (icon, th.icon(icon)) {
        (KEYPAD, _) => keypad_icon(c, th, x, y, fg),
        (_, Some(i)) => c.icon(i, x, y, fg),
        _ => {}
    };
    match glyph {
        Some((w, h)) if label.is_empty() => draw(c, r.x + (r.w - w) / 2, r.y + (r.h - h) / 2),
        Some((w, h)) if r.h >= h + th.line_h + th.pad => {
            let f = font_for(inner.w);
            let block = h + th.px(2) + f.height();
            let top = r.y + (r.h - block) / 2;
            draw(c, r.x + (r.w - w) / 2, top);
            c.text_in(f, IRect::new(inner.x, top + block - f.height(), inner.w, f.height()), label, fg, HAlign::Center);
        }
        Some((w, h)) => {
            let gap = th.pad;
            let f = font_for(inner.w - w - gap);
            let all = (w + gap + f.width(label)).min(inner.w);
            let x = inner.x + (inner.w - all) / 2;
            draw(c, x, r.y + (r.h - h) / 2);
            c.text_in(f, IRect::new(x + w + gap, r.y, all - w - gap, r.h), label, fg, HAlign::Left);
        }
        None => c.text_in(font_for(inner.w), inner, label, fg, HAlign::Center),
    }
    if look != Look::Off {
        c.hit(r, hit);
    }
}

/// `n` rectangles side by side across `r`, `gap` apart, of one width.
pub(crate) fn columns(r: IRect, n: usize, gap: i32) -> Vec<IRect> {
    let n = n.max(1) as i32;
    let w = (r.w - (n - 1) * gap) / n;
    (0..n).map(|i| IRect::new(r.x + i * (w + gap), r.y, w, r.h)).collect()
}
