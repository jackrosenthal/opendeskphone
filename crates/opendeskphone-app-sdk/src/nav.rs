//! Small screen helpers shared by the built-in apps: a message screen, a
//! labeled-input form, and a question before an action.

use crate::controls::{self, Button, Look};
use crate::{Align, InputMode, ROOT, Rect, Skin, Softkey, Touch, Widget, WidgetKind, row_height, screen, ui};

/// The space these screens leave around what they show, in pixels: at the
/// sides of the window, above and below a text, between the rows of a
/// form, and between a label and its input.  The touchscreen has about
/// twice the other screens' pixels to the inch, and its rows are apart
/// by what keeps a finger on the one it means.
struct Space {
    side: u16,
    edge: u16,
    row_gap: u16,
    label_gap: u16,
}

fn space() -> Space {
    match ui::get_metrics().skin {
        Skin::Touch => Space {
            side: 16,
            edge: 8,
            row_gap: 16,
            label_gap: 8,
        },
        _ => Space {
            side: 4,
            edge: 2,
            row_gap: 2,
            label_gap: 2,
        },
    }
}

/// A wrapped text that fills `area` (what [`screen`] returned), inset as
/// the text of a [`Message`] is: for a screen that has something to say
/// in place of its list.  (A text is in the middle of its widget's
/// height.)  On the touchscreen the text is centered, and its middle is a
/// quarter of the way down `area`: in the upper part of the screen, which
/// is read first, and not lost in the middle of the tall screen.
pub fn note(area: Rect, text: &str) -> Widget {
    let s = space();
    let m = ui::get_metrics();
    let inner = Rect::new(
        s.side as i16,
        area.y + s.edge as i16,
        area.width.saturating_sub(2 * s.side),
        area.height.saturating_sub(2 * s.edge),
    );
    let bounds = if m.keys {
        inner
    } else {
        // Room for the lines the text takes, about: a character is about
        // half as wide as a line is high.
        let per_line = (inner.width / (m.line_height / 2).max(1)).max(1) as usize;
        let lines = text.lines().map(|l| l.chars().count().div_ceil(per_line).max(1)).sum::<usize>() as u16;
        let height = (area.height / 4).max((lines + 1) * m.line_height).min(inner.height);
        let top = (area.height / 4).saturating_sub(height / 2);
        Rect::new(inner.x, area.y + top as i16, inner.width, height)
    };
    let w = ui::create(WidgetKind::Text, ROOT, bounds);
    ui::set_wrap(w, true);
    if !m.keys {
        ui::set_align(w, Align::Center);
    }
    ui::set_text(w, text);
    w
}

/// Gives a list the focus on a phone with keys, where the arrows move its
/// highlight.  On the touchscreen a list has no highlighted row until a
/// row is touched: [`screen`] gives its title the focus there, and this
/// leaves it.
pub fn focus_list(list: Widget) {
    if ui::get_metrics().keys {
        ui::set_focus(list);
    }
}

/// The softkeys a screen shows of `keys`: all of them on a phone with
/// keys, and on the touchscreen those whose `id` is not in `for_keys`.
/// The touchscreen does without two kinds of softkey: one that does what
/// `cancel` does on the screen ("Back", "Exit"), because the phone's back
/// button is `cancel`, and one that acts on the highlighted row of a list
/// ("Open", "Select"), because a touch on the row does that.
///
/// ```ignore
/// ui::set_softkeys(ROOT, &nav::softkeys(
///     &[Softkey::new(SK_REFRESH, "Refresh"), Softkey::new(SK_EXIT, "Exit")],
///     &[SK_EXIT],
/// ));
/// ```
pub fn softkeys(keys: &[Softkey], for_keys: &[u32]) -> Vec<Softkey> {
    let has_keys = ui::get_metrics().keys;
    keys.iter().filter(|k| has_keys || !for_keys.contains(&k.id)).cloned().collect()
}

/// A full-screen wrapped message (loading, error, confirmation text).
pub struct Message {
    pub text: Widget,
}

impl Message {
    /// Clears the window and shows `text`, with the given softkeys.
    pub fn show(title: &str, text: &str, keys: &[Softkey]) -> Message {
        let r = screen(title);
        let w = note(r, text);
        ui::set_softkeys(ROOT, keys);
        ui::set_focus(w);
        Message { text: w }
    }
}

/// Touch: a question before an action, on a screen of its own: the
/// question under the title, and across the bottom two buttons, "Cancel"
/// at the left and the action at the right, named for what it does
/// ("Delete", "Log out").  The back button is Cancel too.  (A phone with
/// keys asks with a [`Message`] and softkeys.)
pub struct Confirm {
    cancel: Button,
    act: Button,
}

impl Confirm {
    /// Clears the window and asks `question`; `look` is the action's
    /// button's ([`Look::Destructive`] for one that deletes).
    pub fn show(title: &str, question: &str, action: &str, look: Look) -> Confirm {
        let area = screen(title);
        let (rest, bottom) = controls::split_bottom(area);
        note(rest, question);
        let places = controls::columns(bottom, 2);
        Confirm {
            cancel: Button::new(ROOT, places[0], "Cancel", Look::Secondary),
            act: Button::new(ROOT, places[1], action, look),
        }
    }

    /// What a finger did on `w`: `Some(true)` when it touched the action,
    /// and `Some(false)` when it touched Cancel.
    pub fn touched(&mut self, w: Widget, t: &Touch) -> Option<bool> {
        if self.act.touched(w, t) {
            Some(true)
        } else if self.cancel.touched(w, t) {
            Some(false)
        } else {
            None
        }
    }
}

/// A vertical form of labeled inputs.
pub struct Form {
    pub inputs: Vec<Widget>,
}

impl Form {
    /// Clears the window and builds the form; focus goes to the first input.
    /// (On the touchscreen, touching an input focuses it.)
    pub fn show(title: &str, fields: &[(&str, InputMode, &str)], keys: &[Softkey]) -> Form {
        let r = screen(title);
        let h = row_height();
        let s = space();
        let label_w = r.width / 3;
        let input_x = s.side + label_w + s.label_gap;
        let mut inputs = Vec::new();
        for (i, (label, mode, value)) in fields.iter().enumerate() {
            let y = r.y + (i as u16 * (h + s.row_gap) + s.edge) as i16;
            let l = ui::create(WidgetKind::Text, ROOT, Rect::new(s.side as i16, y, label_w, h));
            ui::set_text(l, label);
            let inp = ui::create(
                WidgetKind::Input,
                ROOT,
                Rect::new(input_x as i16, y, r.width.saturating_sub(input_x + s.side), h),
            );
            ui::set_input_mode(inp, *mode);
            ui::set_value(inp, value);
            inputs.push(inp);
        }
        ui::set_softkeys(ROOT, keys);
        if let Some(first) = inputs.first() {
            ui::set_focus(*first);
        }
        Form { inputs }
    }

    pub fn value(&self, index: usize) -> String {
        self.inputs
            .get(index)
            .map(|w| ui::value(*w))
            .unwrap_or_default()
    }

    /// Moves focus to the next input after `current`; returns false when
    /// `current` was the last.
    pub fn focus_next(&self, current: Widget) -> bool {
        match self.inputs.iter().position(|w| *w == current) {
            Some(i) if i + 1 < self.inputs.len() => {
                ui::set_focus(self.inputs[i + 1]);
                true
            }
            _ => false,
        }
    }
}
