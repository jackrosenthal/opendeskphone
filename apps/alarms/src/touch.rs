//! The screens of the touchscreen, which has no keys.
//!
//! - The list: a row for each alarm, with a switch at its right that turns
//!   the alarm on and off, and touching the row opens the alarm.  "+ New
//!   alarm" is a button across the bottom.
//! - An alarm: its time and its tone are rows, each of which opens a
//!   screen; its days are a row of seven buttons, one for each day, each
//!   on while the alarm rings on that day.  "Save" is across the bottom,
//!   and "Delete alarm", in red, is well above it.
//! - The time: typed on a number pad as the hour and the minutes
//!   ("0630"), with AM and PM as two buttons where the phone shows them.
//!   "Set time" takes it.
//! - The tone: a list in which a touched tone is heard and becomes the
//!   alarm's, with a mark.
//!
//! The back button goes one screen back: from the time without taking it,
//! and from the alarm without saving it.

use super::*;
use opendeskphone_app_sdk::controls::{self, GAP, Look};

/// The width of the switch at the end of a row of the list.
const SWITCH_W: u16 = 160;

/// No widget: the list of a screen that has none.
const NONE: Widget = u32::MAX;

const DAY_LETTERS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

/// The keys of the time's number pad, by rows.
const PAD: [[Option<Act>; 3]; 4] = [
    [Some(Act::Digit(1)), Some(Act::Digit(2)), Some(Act::Digit(3))],
    [Some(Act::Digit(4)), Some(Act::Digit(5)), Some(Act::Digit(6))],
    [Some(Act::Digit(7)), Some(Act::Digit(8)), Some(Act::Digit(9))],
    [None, Some(Act::Digit(0)), Some(Act::Erase)],
];

/// The field of the alarm's row `row`: on the touchscreen the rows are the
/// time and the tone (the days are buttons).
pub(crate) fn field(row: u32) -> u32 {
    match (has_keys(), row) {
        (true, _) => row,
        (false, 0) => ROW_TIME,
        (false, _) => ROW_TONE,
    }
}

impl Alarms {
    pub(crate) fn show_list_touch(&mut self) {
        let snoozed = alarms::snoozed().map(|(id, _)| id);
        let items: Vec<ListItem> = self
            .alarms
            .iter()
            .map(|a| {
                let detail = if snoozed == Some(a.id) { "Snoozed".to_string() } else { days_label(a.days) };
                ListItem::with_detail(time_text(a.hour, a.minute), detail)
            })
            .collect();
        // As many alarms as the list shows (a switch was touched, or an
        // alarm changed elsewhere): the list keeps its place.
        if !items.is_empty() && self.touch.switches.len() == items.len() {
            ui::set_items(self.widget, &items);
            for (s, a) in self.touch.switches.iter_mut().zip(&self.alarms) {
                s.set(a.enabled);
            }
            return;
        }
        self.touch = Controls::default();
        let area = screen("Alarms");
        let (rest, bottom) = controls::split_bottom(area);
        self.touch.buttons.push((Button::new(ROOT, bottom, "+ New alarm", Look::Primary), Act::New));
        if items.is_empty() {
            self.widget = NONE;
            nav::note(rest, "There are no alarms.  Touch New alarm to set one.");
            return;
        }
        // The rows and their switches scroll together: the list is as
        // high as its rows, in a scroll widget, and a finger on a row
        // scrolls that.
        let m = ui::get_metrics();
        let h = m.row_height;
        let scroll = ui::create(WidgetKind::Scroll, ROOT, rest);
        // (The list's hairlines end where the switches' begin.)
        let width = m.width - SWITCH_W + GAP;
        let list = ui::create(WidgetKind::TextList, scroll, Rect::new(0, 0, width, items.len() as u16 * h));
        ui::set_items(list, &items);
        for (i, a) in self.alarms.iter().enumerate() {
            let at = Rect::new((m.width - SWITCH_W) as i16, (i as u16 * h) as i16, SWITCH_W, h);
            self.touch.switches.push(Switch::in_row(scroll, at, a.enabled));
        }
        self.widget = list;
    }

    /// The switch of the alarm of row `i` was turned over.
    fn switched(&mut self, i: usize) {
        let (Some(a), Some(s)) = (self.alarms.get_mut(i), self.touch.switches.get(i)) else { return };
        a.enabled = s.is_on();
        let a = a.clone();
        // (The list follows when the alarm clock says it changed.)
        if let Err(e) = alarms::set(&a) {
            self.show_error(&e);
        }
    }

    pub(crate) fn show_edit_touch(&mut self) {
        let Some(d) = self.draft.clone() else { return };
        self.touch = Controls::default();
        let m = ui::get_metrics();
        let h = m.row_height;
        let area = screen(if d.id == 0 { "New Alarm" } else { "Edit Alarm" });
        let (_, bottom) = controls::split_bottom(area);
        let list = ui::create(WidgetKind::TextList, ROOT, Rect::new(0, area.y, area.width, 2 * h));
        ui::set_items(
            list,
            &[ListItem::with_detail("Time", time_text(d.hour, d.minute)), ListItem::with_detail("Tone", d.tone.clone())],
        );
        self.widget = list;

        // The days: a line that says which, and a button for each.
        let mut y = area.y + (2 * h + GAP) as i16;
        let line = m.line_height;
        let half = area.width / 2;
        let label = ui::create(WidgetKind::Text, ROOT, Rect::new(GAP as i16, y, half - GAP, line));
        ui::set_text(label, "Repeat");
        let shown = ui::create(WidgetKind::Text, ROOT, Rect::new(half as i16, y, half - GAP, line));
        ui::set_align(shown, Align::Right);
        ui::set_text(shown, &days_label(d.days));
        self.touch.shown = Some(shown);
        y += (line + GAP / 2) as i16;
        let row = Rect::new(GAP as i16, y, area.width - 2 * GAP, h);
        for (i, place) in controls::columns(row, 7).into_iter().enumerate() {
            let mut b = Button::new(ROOT, place, DAY_LETTERS[i], Look::Secondary);
            b.set_on(d.days & (1 << i) != 0);
            self.touch.buttons.push((b, Act::Day(i as u8)));
        }
        y += (h + 2 * GAP) as i16;

        let wide = |y: i16| Rect::new(GAP as i16, y, area.width - 2 * GAP, h);
        if d.id != 0 && alarms::snoozed().is_some_and(|(id, _)| id == d.id) {
            self.touch.buttons.push((Button::new(ROOT, wide(y), "Dismiss snooze", Look::Secondary), Act::Dismiss));
            y += (h + GAP) as i16;
        }
        // Well apart from "Save".
        if d.id != 0 {
            let delete = Button::new(ROOT, wide(y + GAP as i16), "Delete alarm", Look::Destructive);
            self.touch.buttons.push((delete, Act::Delete));
        }
        self.touch.buttons.push((Button::new(ROOT, bottom, "Save", Look::Primary), Act::Save));
    }

    /// The day `i` (0 is Sunday) was touched on the alarm's screen.
    fn day(&mut self, i: u8) {
        let Some(d) = &mut self.draft else { return };
        d.days ^= 1 << i;
        let days = d.days;
        for (b, act) in &mut self.touch.buttons {
            if *act == Act::Day(i) {
                b.set_on(days & (1 << i) != 0);
            }
        }
        if let Some(shown) = self.touch.shown {
            ui::set_text(shown, &days_label(days));
        }
    }

    pub(crate) fn show_time_touch(&mut self) {
        self.touch = Controls::default();
        let m = ui::get_metrics();
        let area = screen("Time");
        let (rest, bottom) = controls::split_bottom(area);
        self.touch.buttons.push((Button::new(ROOT, bottom, "Set time", Look::Primary), Act::SetTime));

        // The pad: four rows of three keys, over the button.
        let key_h = m.row_height + GAP;
        let pad_h = 4 * key_h + 3 * GAP;
        let top = rest.y + rest.height.saturating_sub(pad_h + GAP) as i16;
        for (r, keys) in PAD.iter().enumerate() {
            let row = Rect::new(GAP as i16, top + (r as u16 * (key_h + GAP)) as i16, area.width - 2 * GAP, key_h);
            for (place, key) in controls::columns(row, 3).into_iter().zip(keys) {
                let b = match key {
                    Some(Act::Digit(d)) => Button::new(ROOT, place, &d.to_string(), Look::Secondary).with_font(Font::Large),
                    Some(Act::Erase) => Button::with_icon(ROOT, place, Icon::Backspace, Look::Secondary),
                    _ => continue,
                };
                self.touch.buttons.push((b, key.unwrap()));
            }
        }

        // Over the pad, in the middle of the space there: the time typed,
        // and AM and PM.
        let text_h = 2 * m.line_height;
        let block = if self.meridiem { text_h + GAP + m.row_height } else { text_h };
        let y = rest.y + ((top - rest.y) as u16).saturating_sub(block) as i16 / 2;
        let shown = ui::create(WidgetKind::Text, ROOT, Rect::new(0, y, area.width, text_h));
        ui::set_font(shown, Font::Large);
        ui::set_align(shown, Align::Center);
        self.touch.shown = Some(shown);
        if self.meridiem {
            let w = 2 * (area.width / 5) + GAP;
            let row = Rect::new(((area.width - w) / 2) as i16, y + (text_h + GAP) as i16, w, m.row_height);
            for (place, (label, pm)) in controls::columns(row, 2).into_iter().zip([("AM", false), ("PM", true)]) {
                self.touch.buttons.push((Button::new(ROOT, place, label, Look::Secondary), Act::Half(pm)));
            }
        }
        self.draw_time_touch();
    }

    /// The time typed, which of AM and PM is chosen, and whether the time
    /// is one that "Set time" takes.
    pub(crate) fn draw_time_touch(&mut self) {
        if let Some(shown) = self.touch.shown {
            ui::set_text(shown, &self.typed.text(false));
        }
        let (pm, whole) = (self.typed.pm, self.typed.time(self.meridiem).is_some());
        for (b, act) in &mut self.touch.buttons {
            match act {
                Act::Half(half) => b.set_on(*half == pm),
                Act::SetTime => b.set_enabled(whole),
                _ => {}
            }
        }
    }

    pub(crate) fn show_tones_touch(&mut self) {
        self.touch = Controls::default();
        let area = screen("Tone");
        self.field = ui::create(WidgetKind::TextList, ROOT, area);
        self.draw_choices();
    }

    /// A tone was touched: it is heard, and it is the alarm's.
    pub(crate) fn tone_touched(&mut self) {
        let tone = alarms::tones().into_iter().nth(self.dialog_row as usize);
        if let (Some(tone), Some(d)) = (tone, &mut self.draft) {
            alarms::preview(Some(&tone));
            d.tone = tone;
        }
        self.draw_choices();
    }

    /// What a finger did on a widget that takes touches.
    pub(crate) fn touched(&mut self, w: Widget, t: &Touch) {
        // Every control is told, so that the one under the finger looks
        // pressed while the finger is on it.
        let mut acted = None;
        for (b, act) in &mut self.touch.buttons {
            if b.touched(w, t) {
                acted = Some(*act);
            }
        }
        let mut switched = None;
        for (i, s) in self.touch.switches.iter_mut().enumerate() {
            if s.touched(w, t) {
                switched = Some(i);
            }
        }
        if let Some(i) = switched {
            self.switched(i);
        }
        let Some(act) = acted else { return };
        match act {
            Act::New => self.new_alarm(),
            Act::Day(i) => self.day(i),
            Act::Dismiss => self.after(alarms::dismiss()),
            Act::Delete => {
                if let Some(id) = self.draft.as_ref().map(|d| d.id) {
                    self.delete(id);
                }
            }
            Act::Save => self.save(),
            Act::Digit(d) => {
                self.typed.push(d);
                self.draw_time_touch();
            }
            Act::Erase => {
                self.typed.erase();
                self.draw_time_touch();
            }
            Act::Half(pm) => {
                self.typed.pm = pm;
                self.draw_time_touch();
            }
            Act::SetTime => self.dialog_done(),
        }
    }
}
