//! Small screen helpers shared by the built-in apps: a message screen and
//! a labeled-input form.

use crate::{InputMode, ROOT, Rect, Skin, Softkey, Widget, WidgetKind, row_height, screen, ui};

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
/// in place of its list.
pub fn note(area: Rect, text: &str) -> Widget {
    let s = space();
    let w = ui::create(
        WidgetKind::Text,
        ROOT,
        Rect::new(
            s.side as i16,
            area.y + s.edge as i16,
            area.width.saturating_sub(2 * s.side),
            area.height.saturating_sub(2 * s.edge),
        ),
    );
    ui::set_wrap(w, true);
    ui::set_text(w, text);
    w
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
