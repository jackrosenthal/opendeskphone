//! Buttons and switches for the touchscreen, which the widgets do not
//! have.  Each control is a canvas ([`canvas`]) that takes touches
//! ([`ui::claim_touch`]), drawn in the skin's colors in the shape of the
//! phone's own buttons.
//!
//! An app makes a control and hands it every [`Touch`] that comes for a
//! widget; the control's `touched` says when the control was touched:
//!
//! ```ignore
//! use opendeskphone_app_sdk::controls::{self, Button, Look};
//!
//! let (_, bottom) = controls::split_bottom(area);
//! let save = Button::new(ROOT, bottom, "Save", Look::Primary);
//! // ...
//! Event::Touch((w, t)) if self.save.touched(w, &t) => self.save(),
//! ```
//!
//! A control looks pressed while a finger is on it (from `Touch::Down`)
//! and is touched when the finger is lifted where it came down
//! (`Touch::Tap`); a finger that slides off touches nothing.
//!
//! The controls are for a phone without keys ([`crate::has_keys`]): there,
//! what an app does on its screen is touched, and a phone with keys has
//! softkeys for the same things.
//!
//! [`Touch`]: crate::Touch

use crate::canvas::{self, Bitmap, Color, Op, Role, Text};
use crate::{Font, Icon, Image, Rect, Touch, Widget, WidgetKind, ui};

/// What a button is for, which is how it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The screen's main action: filled in the selection colors.
    Primary,
    /// Any other action: outlined.
    Secondary,
    /// An action that erases or deletes: filled red.
    Destructive,
    /// An action that places a call: filled green.
    Call,
}

/// The space around controls and between them, in pixels.
pub const GAP: u16 = 16;

/// The height of a button: a list's row (what a finger needs).
pub fn height() -> u16 {
    ui::get_metrics().row_height
}

/// `area` without the bottom, and the place of a row of buttons across
/// the bottom, [`GAP`] in from the sides and the bottom.
pub fn split_bottom(area: Rect) -> (Rect, Rect) {
    split_bottom_n(area, 1)
}

/// [`split_bottom`] for `rows` rows of buttons, [`GAP`] apart: the place
/// of them all.
pub fn split_bottom_n(area: Rect, rows: u16) -> (Rect, Rect) {
    let h = height();
    let used = (rows * h + rows.saturating_sub(1) * GAP + GAP).min(area.height);
    let rest = Rect::new(area.x, area.y, area.width, area.height - used);
    let place = Rect::new(
        area.x + GAP as i16,
        area.y + (area.height - used) as i16,
        area.width.saturating_sub(2 * GAP),
        used.saturating_sub(GAP),
    );
    (rest, place)
}

/// `n` places side by side in `row`, [`GAP`] apart, of the same width
/// (the last takes what is left over).
pub fn columns(row: Rect, n: u16) -> Vec<Rect> {
    let n = n.max(1);
    let w = row.width.saturating_sub((n - 1) * GAP) / n;
    (0..n)
        .map(|i| {
            let x = row.x + (i * (w + GAP)) as i16;
            let width = if i + 1 == n { (row.x + row.width as i16 - x).max(0) as u16 } else { w };
            Rect::new(x, row.y, width, row.height)
        })
        .collect()
}

/// The `i`-th of the rows of buttons in `place` (of
/// [`split_bottom_n`]), from the top.
pub fn row(place: Rect, i: u16) -> Rect {
    let h = height();
    Rect::new(place.x, place.y + (i * (h + GAP)) as i16, place.width, h)
}

// ------------------------------------------------------------- buttons

/// A button: a rounded shape with a label, or one of the phone's icons,
/// in its middle.
pub struct Button {
    /// The canvas: what an [`crate::Event::Touch`] names.
    pub widget: Widget,
    icon: Option<Widget>,
    width: u16,
    height: u16,
    label: String,
    font: Option<Font>,
    look: Look,
    on: bool,
    enabled: bool,
    pressed: bool,
}

impl Button {
    /// Makes a button in `bounds` of `parent` (`ROOT`: the window).
    pub fn new(parent: Widget, bounds: Rect, label: &str, look: Look) -> Button {
        let widget = canvas::create(parent, bounds);
        ui::claim_touch(widget, true);
        let b = Button {
            widget,
            icon: None,
            width: bounds.width,
            height: bounds.height,
            label: label.to_string(),
            font: None,
            look,
            on: false,
            enabled: true,
            pressed: false,
        };
        b.draw();
        b
    }

    /// A button with one of the phone's icons in place of a label.  The
    /// phone draws the icon in the text's color: for a
    /// [`Look::Secondary`] button.
    pub fn with_icon(parent: Widget, bounds: Rect, icon: Icon, look: Look) -> Button {
        let mut b = Button::new(parent, bounds, "", look);
        let image = ui::create(WidgetKind::Image, parent, bounds);
        ui::set_image(image, &Image::Icon(icon));
        b.icon = Some(image);
        b
    }

    /// The button with its label in `font` (a digit of a number pad in
    /// `Font::Large`); otherwise the label is in the font of the phone's
    /// buttons.
    pub fn with_font(mut self, font: Font) -> Button {
        self.font = Some(font);
        self.draw();
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn set_label(&mut self, label: &str) {
        if self.label != label {
            self.label = label.to_string();
            self.draw();
        }
    }

    /// Draws the button as chosen (`true`), in the selection colors, or
    /// not: for a button that is one of a set of choices, such as a day
    /// of the week or one of AM and PM.
    pub fn set_on(&mut self, on: bool) {
        if self.on != on {
            self.on = on;
            self.draw();
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// A button that is not enabled is drawn dim and is not touched.
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.pressed &= enabled;
            self.draw();
        }
    }

    pub fn set_visible(&self, visible: bool) {
        ui::set_visible(self.widget, visible);
        if let Some(icon) = self.icon {
            ui::set_visible(icon, visible);
        }
    }

    /// What a finger did on `w`: `true` when it touched this button.
    pub fn touched(&mut self, w: Widget, t: &Touch) -> bool {
        if w != self.widget {
            return false;
        }
        let (pressed, tapped) = press(t, self.enabled, self.width, self.height);
        if pressed != self.pressed {
            self.pressed = pressed;
            self.draw();
        }
        tapped
    }

    fn draw(&self) {
        let (fill, edge, ink) = colors(self.look, self.on, self.pressed, self.enabled);
        let mut ops = rounded(self.width, self.height, RADIUS, edge, fill);
        if !self.label.is_empty() {
            let inner = self.width.saturating_sub(2 * GAP);
            let font = match self.font {
                Some(font) => font,
                // The smaller font for a label the usual one has no room
                // for, as the phone does.
                None if canvas::text_size(Font::Bold, &self.label).width > inner => Font::Small,
                None => Font::Bold,
            };
            ops.push(Op::Text(Text {
                box_: Rect::new(GAP as i16, 0, inner, self.height),
                font,
                color: ink,
                text: self.label.clone(),
            }));
        }
        let _ = canvas::draw(self.widget, &ops);
    }
}

/// Whether a finger is on a control after `t`, and whether `t` touched
/// it: lifted where it came down.
fn press(t: &Touch, enabled: bool, width: u16, height: u16) -> (bool, bool) {
    let inside = |x: i16, y: i16| x >= 0 && y >= 0 && (x as u16) < width && (y as u16) < height;
    match t {
        Touch::Down(_) => (enabled, false),
        Touch::Drag((_, at)) => (enabled && inside(at.x, at.y), false),
        Touch::DragEnd(_) => (false, false),
        Touch::Tap(_) => (false, enabled),
    }
}

/// The corners of a button, as the phone rounds its own.
const RADIUS: u16 = 12;
/// The width of a button's outline.
const EDGE: u16 = 2;

const WHITE: Color = Color { r: 0xFF, g: 0xFF, b: 0xFF };
const BLACK: Color = Color { r: 0, g: 0, b: 0 };
/// The phone's colors for an action that stops or erases, and for one
/// that calls (its softkeys' "stop" and "go").
const RED: Color = Color { r: 0xB7, g: 0x1C, b: 0x1C };
const GREEN: Color = Color { r: 0x38, g: 0x8E, b: 0x3C };

/// `a`, `num`/`den` of the way to `b`.
fn mix(a: Color, b: Color, num: u16, den: u16) -> Color {
    let m = |x: u8, y: u8| ((x as i32 * (den - num) as i32 + y as i32 * num as i32) / den as i32) as u8;
    Color { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

/// The fill, outline, and text of a button.
fn colors(look: Look, on: bool, pressed: bool, enabled: bool) -> (Color, Color, Color) {
    let bg = canvas::theme_color(Role::Background);
    let fg = canvas::theme_color(Role::Foreground);
    // As the phone's own buttons: a fill a little off the background,
    // and an outline a third of the way to the text.
    let plain = (mix(bg, fg, 1, 16), mix(bg, fg, 1, 3), fg);
    if !enabled {
        return (bg, mix(bg, fg, 1, 5), canvas::theme_color(Role::Secondary));
    }
    let filled = |c: Color, ink: Color| {
        let c = if pressed { mix(c, BLACK, 1, 4) } else { c };
        (c, c, ink)
    };
    let selected = || filled(canvas::theme_color(Role::Highlight), canvas::theme_color(Role::HighlightText));
    match look {
        Look::Primary => selected(),
        Look::Destructive => filled(RED, WHITE),
        Look::Call => filled(GREEN, WHITE),
        // A finger on it shows what touching it chooses.
        Look::Secondary if on || pressed => selected(),
        Look::Secondary => plain,
    }
}

/// How far in from the side row `i` of a rounded corner of radius `r`
/// begins (row 0 is the outermost).
fn corner_inset(r: u16, i: u16) -> u16 {
    // The first pixel whose middle is in the circle.
    let (r2, dy) = (2 * r as i32, 2 * (r as i32 - i as i32) - 1);
    (0..r).find(|x| {
        let dx = 2 * (r as i32 - *x as i32) - 1;
        dx * dx + dy * dy <= r2 * r2
    })
    .unwrap_or(r)
}

/// A rounded rectangle at (`x`, `y`) as strips of `color`.
fn round_rect(ops: &mut Vec<Op>, x: i16, y: i16, w: u16, h: u16, r: u16, color: Color) {
    let r = r.min(w / 2).min(h / 2);
    for i in 0..r {
        let inset = corner_inset(r, i);
        let width = w.saturating_sub(2 * inset);
        for row in [y + i as i16, y + (h - 1 - i) as i16] {
            ops.push(Op::Fill((Rect::new(x + inset as i16, row, width, 1), color)));
        }
    }
    ops.push(Op::Fill((Rect::new(x, y + r as i16, w, h - 2 * r), color)));
}

/// A button's shape, `w` by `h`: `edge` with `fill` inside it.
fn rounded(w: u16, h: u16, r: u16, edge: Color, fill: Color) -> Vec<Op> {
    let mut ops = Vec::new();
    round_rect(&mut ops, 0, 0, w, h, r, edge);
    if edge != fill && w > 2 * EDGE && h > 2 * EDGE {
        round_rect(&mut ops, EDGE as i16, EDGE as i16, w - 2 * EDGE, h - 2 * EDGE, r.saturating_sub(EDGE), fill);
    }
    ops
}

/// The bits of a filled circle `d` pixels across, as a [`Bitmap`] has
/// them.
fn disc_bits(d: u16) -> Vec<u8> {
    let stride = d.div_ceil(8) as usize;
    let mut bits = vec![0u8; stride * d as usize];
    let d2 = d as i32 * d as i32;
    for y in 0..d as i32 {
        for x in 0..d as i32 {
            let (dx, dy) = (2 * x + 1 - d as i32, 2 * y + 1 - d as i32);
            if dx * dx + dy * dy <= d2 {
                bits[y as usize * stride + x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    bits
}

/// A filled circle `d` pixels across with its top-left corner at
/// (`x`, `y`).
pub fn disc(x: i16, y: i16, d: u16, color: Color) -> Op {
    Op::Bitmap(Bitmap { x, y, width: d, height: d, scale: 1, color, bits: disc_bits(d) })
}

// ------------------------------------------------------------ switches

/// A switch: a setting that is on or off, such as whether an alarm
/// rings.  A touch turns it over.  The switch is drawn at the right of
/// its bounds, in the middle of their height; the rest of the bounds is
/// what a finger may touch.
pub struct Switch {
    /// The canvas: what an [`crate::Event::Touch`] names.
    pub widget: Widget,
    width: u16,
    height: u16,
    on: bool,
    pressed: bool,
    /// A hairline across the bottom, as under a row of a list.
    line: bool,
}

impl Switch {
    pub fn new(parent: Widget, bounds: Rect, on: bool) -> Switch {
        let widget = canvas::create(parent, bounds);
        ui::claim_touch(widget, true);
        let s = Switch { widget, width: bounds.width, height: bounds.height, on, pressed: false, line: false };
        s.draw();
        s
    }

    /// A switch at the end of a row of a list: with the hairline that the
    /// phone draws under a row, from the left of `bounds`.
    pub fn in_row(parent: Widget, bounds: Rect, on: bool) -> Switch {
        let mut s = Switch::new(parent, bounds, on);
        s.line = true;
        s.draw();
        s
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn set(&mut self, on: bool) {
        if self.on != on {
            self.on = on;
            self.draw();
        }
    }

    /// What a finger did on `w`: `true` when it turned this switch over.
    pub fn touched(&mut self, w: Widget, t: &Touch) -> bool {
        if w != self.widget {
            return false;
        }
        let (pressed, tapped) = press(t, true, self.width, self.height);
        self.pressed = pressed;
        if tapped {
            self.on = !self.on;
        }
        self.draw();
        tapped
    }

    fn draw(&self) {
        let bg = canvas::theme_color(Role::Background);
        let fg = canvas::theme_color(Role::Foreground);
        let hi = canvas::theme_color(Role::Highlight);
        let mut ops = Vec::new();
        if self.line {
            // As `widgets::list` draws it: to 16 pixels from the right.
            let w = self.width.saturating_sub(GAP);
            ops.push(Op::Fill((Rect::new(0, self.height as i16 - 1, w, 1), canvas::theme_color(Role::Border))));
        }
        // The track, a pill twice as wide as high, and the knob in it, at
        // the right when the switch is on.
        let h = (height() * 7 / 12).min(self.height);
        let w = 2 * h - h / 4;
        let x = self.width.saturating_sub(w + GAP) as i16;
        let y = ((self.height - h) / 2) as i16;
        let track = match (self.on, self.pressed) {
            (true, false) => hi,
            (true, true) => mix(hi, BLACK, 1, 4),
            (false, false) => mix(bg, fg, 3, 8),
            (false, true) => mix(bg, fg, 1, 2),
        };
        ops.push(disc(x, y, h, track));
        ops.push(disc(x + (w - h) as i16, y, h, track));
        ops.push(Op::Fill((Rect::new(x + (h / 2) as i16, y, w - h, h), track)));
        let inset = h / 8;
        let knob = h - 2 * inset;
        let kx = if self.on { x + (w - h + inset) as i16 } else { x + inset as i16 };
        ops.push(disc(kx, y + inset as i16, knob, WHITE));
        let _ = canvas::draw(self.widget, &ops);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corner_is_rounder_toward_its_point() {
        let insets: Vec<u16> = (0..12).map(|i| corner_inset(12, i)).collect();
        assert!(insets[0] > 6, "{insets:?}");
        assert!(insets.windows(2).all(|w| w[0] >= w[1]), "{insets:?}");
        assert_eq!(insets[11], 0, "{insets:?}");
    }

    #[test]
    fn a_disc_is_round_and_fills_its_middle() {
        let bits = disc_bits(10);
        let at = |x: usize, y: usize| bits[y * 2 + x / 8] & (0x80 >> (x % 8)) != 0;
        assert!(at(5, 5) && at(0, 5) && at(9, 4) && at(5, 0));
        assert!(!at(0, 0) && !at(9, 9) && !at(0, 9) && !at(9, 0));
    }

    #[test]
    fn a_finger_presses_and_touches_only_where_it_came_down() {
        let p = |x, y| crate::Point { x, y };
        assert_eq!(press(&Touch::Down(p(5, 5)), true, 10, 10), (true, false));
        assert_eq!(press(&Touch::Tap(p(5, 5)), true, 10, 10), (false, true));
        assert_eq!(press(&Touch::Drag((p(5, 5), p(12, 5))), true, 10, 10), (false, false));
        assert_eq!(press(&Touch::Drag((p(5, 5), p(6, 5))), true, 10, 10), (true, false));
        assert_eq!(press(&Touch::DragEnd((p(5, 5), p(6, 5))), true, 10, 10), (false, false));
        assert_eq!(press(&Touch::Tap(p(5, 5)), false, 10, 10), (false, false));
    }

    #[test]
    fn columns_share_a_row() {
        let c = columns(Rect::new(16, 0, 768, 96), 3);
        assert_eq!(c.iter().map(|r| (r.x, r.width)).collect::<Vec<_>>(), [(16, 245), (277, 245), (538, 246)]);
    }
}
