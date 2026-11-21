//! Alarms: the screens for setting the phone's alarm clock.  The alarm
//! clock itself is the phone's (the interface `alarms`): it keeps the
//! alarms, rings them, and shows the ringing alarm with Snooze and Dismiss,
//! whether or not this app runs.  Here the user adds, changes, enables,
//! disables, and deletes alarms.  An alarm's time, days, and tone are a
//! list of fields; Select on a field opens a dialog over the list in which
//! the field is changed.
//!
//! The touchscreen, which has no keys, has screens of its own
//! ([`touch`]): each alarm of the list has a switch that turns it on and
//! off, and touching the alarm opens it; its days are a row of buttons, one
//! for each day; and its time is typed on a number pad, with AM and PM
//! beside it.  The phone's back button is Cancel.

use opendeskphone_app_sdk::alarms::{self, Alarm};
use opendeskphone_app_sdk::canvas::{self, Op, Role};
use opendeskphone_app_sdk::controls::{Button, Switch};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{Align, Error, Font, nav};

mod text;
mod touch;

use text::{Typed, days_label, sentence};

const SK_NEW: u32 = 1;
const SK_EDIT: u32 = 2;
const SK_SWITCH: u32 = 3;
const SK_DELETE: u32 = 4;
const SK_DISMISS: u32 = 5;
const SK_EXIT: u32 = 6;
const SK_SAVE: u32 = 7;
const SK_CANCEL: u32 = 8;
const SK_OK: u32 = 9;
const SK_MERIDIEM: u32 = 10;
const SK_ERASE: u32 = 11;
const SK_PLAY: u32 = 12;

const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The rows of the screen that edits one alarm.
const ROW_TIME: u32 = 0;
const ROW_REPEAT: u32 = 1;
const ROW_TONE: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Screen {
    #[default]
    List,
    /// The fields of one alarm, with or without a dialog over them.
    Edit,
    Error,
}

/// The dialog that changes one field of the alarm.  (Touch: the screen
/// of its own that changes it.)
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    Time,
    Days,
    Tones,
}

/// What a control of the touchscreen does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    New,
    /// Switches the alarm on the day (0 is Sunday) or off.
    Day(u8),
    Dismiss,
    Delete,
    Save,
    Digit(u8),
    Erase,
    /// AM (`false`) or PM.
    Half(bool),
    SetTime,
}

/// The controls of the screen on the touchscreen.
#[derive(Default)]
struct Controls {
    buttons: Vec<(Button, Act)>,
    /// The switch of each alarm of the list, in the list's order.
    switches: Vec<Switch>,
    /// The time typed, on the time's screen; the days chosen, on the
    /// alarm's.
    shown: Option<Widget>,
}

#[derive(Default)]
struct Alarms {
    screen: Screen,
    /// The list of the screen: the alarms, or the fields of one.
    widget: Widget,
    alarms: Vec<Alarm>,
    /// The row highlighted in the list of alarms, and in the alarm's rows.
    row: u32,
    edit_row: u32,
    /// The alarm being added or changed.
    draft: Option<Alarm>,
    dialog: Option<Dialog>,
    /// The dialog's widget for the value: its list, or the time's text.
    field: Widget,
    /// The row highlighted in the dialog's list.
    dialog_row: u32,
    /// The days marked in the dialog, until it is confirmed.
    days: u8,
    typed: Typed,
    /// The phone shows times with AM and PM.
    meridiem: bool,
    /// Touch: the controls on the screen.
    touch: Controls,
}

/// The text of a time of day as the phone shows times.
fn time_text(hour: u8, minute: u8) -> String {
    // Any day will do: the text has no date.
    let unix = 86_400 * 20_000 + i64::from(hour) * 3600 + i64::from(minute) * 60;
    l10n::format_time((unix - i64::from(l10n::utc_offset())) as u64, TimeStyle::Time)
}

impl Alarms {
    fn list_widget(&mut self, title: &str, items: &[ListItem], selected: u32) {
        let area = screen(title);
        let list = ui::create(WidgetKind::TextList, ROOT, area);
        ui::set_items(list, items);
        ui::set_selected(list, selected.min(items.len().saturating_sub(1) as u32));
        ui::set_focus(list);
        self.widget = list;
    }

    // ------------------------------------------------------------ the list

    /// The alarm of the highlighted row.
    fn selected(&self) -> Option<&Alarm> {
        self.alarms.get(self.row as usize)
    }

    fn list_softkeys(&self) {
        let mut keys = vec![Softkey::new(SK_NEW, "New")];
        if let Some(a) = self.selected() {
            keys.push(Softkey::new(SK_EDIT, "Edit"));
            keys.push(Softkey::new(SK_SWITCH, if a.enabled { "Disable" } else { "Enable" }));
            if alarms::snoozed().is_some_and(|(id, _)| id == a.id) {
                keys.push(Softkey::new(SK_DISMISS, "Dismiss"));
            }
            keys.push(Softkey::new(SK_DELETE, "Delete"));
        }
        keys.push(Softkey::new(SK_EXIT, "Exit"));
        ui::set_softkeys(ROOT, &keys);
    }

    fn show_list(&mut self) {
        self.screen = Screen::List;
        self.draft = None;
        self.dialog = None;
        self.alarms = alarms::list();
        if !has_keys() {
            return self.show_list_touch();
        }
        let snoozed = alarms::snoozed().map(|(id, _)| id);
        let items: Vec<ListItem> = self
            .alarms
            .iter()
            .map(|a| {
                let detail = if snoozed == Some(a.id) {
                    "Snoozed".to_string()
                } else if a.enabled {
                    days_label(a.days)
                } else {
                    "Disabled".to_string()
                };
                ListItem::with_detail(time_text(a.hour, a.minute), detail)
            })
            .collect();
        self.row = self.row.min(items.len().saturating_sub(1) as u32);
        self.list_widget("Alarms", &items, self.row);
        self.list_softkeys();
    }

    fn switch(&mut self) {
        let Some(a) = self.selected() else { return };
        let result = alarms::set(&Alarm {
            enabled: !a.enabled,
            ..a.clone()
        });
        self.after(result.map(|_| ()));
    }

    /// Shows the list again after a change, or why the change was not made.
    fn after(&mut self, result: Result<(), Error>) {
        match result {
            Ok(()) => self.show_list(),
            Err(e) => self.show_error(&e),
        }
    }

    fn show_error(&mut self, e: &Error) {
        self.screen = Screen::Error;
        self.touch = Controls::default();
        let (Error::Invalid(why) | Error::Failed(why)) = e;
        let why = sentence(why);
        Message::show("Alarms", &why, &[Softkey::new(SK_OK, "OK")]);
    }

    // ------------------------------------------------------------ one alarm

    fn new_alarm(&mut self) {
        // The next whole hour, once.
        let local = timers::wall_time() as i64 + i64::from(l10n::utc_offset());
        let hour = ((local.rem_euclid(86_400) / 3600 + 1) % 24) as u8;
        self.draft = Some(Alarm {
            id: 0,
            hour,
            minute: 0,
            days: 0,
            tone: alarms::tones().first().cloned().unwrap_or_default(),
            enabled: true,
        });
        self.edit_row = ROW_TIME;
        self.show_edit();
    }

    fn edit_alarm(&mut self) {
        if let Some(a) = self.selected() {
            self.draft = Some(a.clone());
            self.edit_row = ROW_TIME;
            self.show_edit();
        }
    }

    /// The alarm's fields, without a dialog.
    fn show_edit(&mut self) {
        let Some(d) = &self.draft else {
            return self.show_list();
        };
        self.screen = Screen::Edit;
        self.dialog = None;
        if !has_keys() {
            return self.show_edit_touch();
        }
        let items = [
            ListItem::with_detail("Time", time_text(d.hour, d.minute)),
            ListItem::with_detail("Repeat", days_label(d.days)),
            ListItem::with_detail("Tone", d.tone.clone()),
        ];
        let title = if d.id == 0 { "New Alarm" } else { "Edit Alarm" };
        self.list_widget(title, &items, self.edit_row);
        ui::set_softkeys(ROOT, &[Softkey::new(SK_SAVE, "Save"), Softkey::new(SK_CANCEL, "Cancel")]);
    }

    fn save(&mut self) {
        if let Some(d) = self.draft.take() {
            let result = alarms::set(&d);
            if result.is_ok() && d.id == 0 {
                // The new alarm is the last.
                self.row = self.alarms.len() as u32;
            }
            self.after(result.map(|_| ()));
        }
    }

    /// Deletes the alarm `id`, and shows the list.
    fn delete(&mut self, id: u32) {
        self.after(alarms::remove(id));
    }

    // ----------------------------------------------------------- the dialogs

    /// Opens the dialog that changes the field `row` of the alarm's rows
    /// (`ROW_TIME`, `ROW_REPEAT`, or `ROW_TONE`).
    fn open_dialog(&mut self, row: u32) {
        let Some(d) = &self.draft else { return };
        let dialog = match row {
            ROW_TIME => {
                self.meridiem = time_text(13, 0).contains('M');
                self.typed = Typed::of(d.hour, d.minute, self.meridiem);
                Dialog::Time
            }
            ROW_REPEAT => {
                self.days = d.days;
                self.dialog_row = 0;
                Dialog::Days
            }
            ROW_TONE => {
                self.dialog_row = alarms::tones().iter().position(|t| *t == d.tone).unwrap_or(0) as u32;
                Dialog::Tones
            }
            _ => return,
        };
        self.dialog = Some(dialog);
        self.draw_dialog();
    }

    /// Draws the open dialog as it stands: a box in the phone's manner
    /// over the middle of the fields, with the field's name and the value.
    /// The screen's title stays in view above it.  (Touch: a screen of its
    /// own.)
    fn draw_dialog(&mut self) {
        let Some(dialog) = self.dialog else { return };
        let m = ui::get_metrics();
        if !m.keys {
            return match dialog {
                Dialog::Time => self.show_time_touch(),
                Dialog::Tones => self.show_tones_touch(),
                // (The days are buttons on the alarm's screen.)
                Dialog::Days => {}
            };
        }
        // As the phone frames its own notices: two lines of the text's
        // color on the gray displays, a thin edge on the color one.
        // On the gray ones a line of the background is left inside the
        // frame, so that a highlighted row does not run into it.
        let (edge, pad) = if m.color { (1, 8) } else { (3, 2) };
        let width = if m.color { 300 } else { m.width * 3 / 4 };
        let head = m.line_height;
        let below = m.height.saturating_sub(m.title_height);
        let room = below.saturating_sub(2 * edge + head);
        let rows = |n: usize| (n as u16).min(room / m.row_height.max(1)) * m.row_height;
        let (title, body) = match dialog {
            Dialog::Time => ("Time", (m.line_height * 2 + m.line_height + pad).min(room)),
            Dialog::Days => ("Repeat", rows(DAY_NAMES.len())),
            Dialog::Tones => ("Tone", rows(alarms::tones().len().max(1))),
        };
        let height = 2 * edge + head + body;
        let top = m.title_height + (below - height) / 2;
        let bounds = Rect::new(((m.width - width) / 2) as i16, top as i16, width, height);
        let group = ui::create(WidgetKind::Group, ROOT, bounds);
        let back = canvas::create(group, Rect::new(0, 0, width, height));
        let line = canvas::theme_color(if m.color { Role::Border } else { Role::Foreground });
        let frame = edge.min(2);
        let inside = Rect::new(frame as i16, frame as i16, width - 2 * frame, height - 2 * frame);
        let _ = canvas::draw(
            back,
            &[
                Op::Fill((Rect::new(0, 0, width, height), line)),
                Op::Fill((inside, canvas::theme_color(Role::Background))),
            ],
        );
        let name = ui::create(WidgetKind::Text, group, Rect::new((edge + pad) as i16, edge as i16, width - 2 * (edge + pad), head));
        ui::set_font(name, Font::Bold);
        ui::set_text(name, title);
        let area = Rect::new(edge as i16, (edge + head) as i16, width - 2 * edge, body);
        match dialog {
            Dialog::Time => {
                let hint_h = m.line_height;
                let time = ui::create(WidgetKind::Text, group, Rect::new(area.x, area.y, area.width, body - hint_h - pad));
                ui::set_font(time, Font::Large);
                ui::set_align(time, Align::Center);
                let hint = ui::create(
                    WidgetKind::Text,
                    group,
                    Rect::new(area.x, area.y + (body - hint_h - pad) as i16, area.width, hint_h),
                );
                ui::set_font(hint, Font::Small);
                ui::set_align(hint, Align::Center);
                ui::set_text(hint, "Type the hour and the minutes.");
                // The dialog takes the keys that type the time.
                let mut claims = vec![Key::Cancel, Key::Select, Key::Left];
                claims.extend((0..=9).map(Key::Digit));
                ui::claim_keys(group, &claims);
                ui::set_focus(group);
                let mut softkeys = vec![Softkey::new(SK_OK, "OK")];
                if self.meridiem {
                    softkeys.push(Softkey::new(SK_MERIDIEM, "AM/PM"));
                }
                softkeys.push(Softkey::new(SK_ERASE, "Delete"));
                softkeys.push(Softkey::new(SK_CANCEL, "Cancel"));
                ui::set_softkeys(group, &softkeys);
                self.field = time;
                self.draw_time();
            }
            Dialog::Days | Dialog::Tones => {
                let list = ui::create(WidgetKind::TextList, group, area);
                ui::set_focus(list);
                let softkeys = match dialog {
                    Dialog::Tones => vec![
                        Softkey::new(SK_OK, "Select"),
                        Softkey::new(SK_PLAY, "Play"),
                        Softkey::new(SK_CANCEL, "Cancel"),
                    ],
                    _ => vec![Softkey::new(SK_OK, "OK"), Softkey::new(SK_CANCEL, "Cancel")],
                };
                ui::set_softkeys(list, &softkeys);
                self.field = list;
                self.draw_choices();
            }
        }
    }

    fn draw_time(&mut self) {
        if has_keys() {
            ui::set_text(self.field, &self.typed.text(self.meridiem));
        } else {
            self.draw_time_touch();
        }
    }

    /// The rows of the dialog's list: the days, with a mark on each the
    /// alarm repeats on, or the tones, with a mark on the alarm's.
    fn draw_choices(&self) {
        let marked = |text: String, on: bool| ListItem {
            text,
            detail: String::new(),
            icon: on.then_some(Image::Icon(Icon::Ok)),
        };
        let items: Vec<ListItem> = match self.dialog {
            Some(Dialog::Days) => DAY_NAMES
                .iter()
                .enumerate()
                .map(|(i, name)| marked(name.to_string(), self.days & (1 << i) != 0))
                .collect(),
            Some(Dialog::Tones) => {
                let tone = self.draft.as_ref().map(|d| d.tone.as_str());
                alarms::tones().into_iter().map(|t| marked(t.clone(), tone == Some(t.as_str()))).collect()
            }
            _ => return,
        };
        ui::set_items(self.field, &items);
        ui::set_selected(self.field, self.dialog_row.min(items.len().saturating_sub(1) as u32));
    }

    /// Select in a dialog: a day is switched; a time or a tone is
    /// taken.  (Touch: a tone is taken and heard, and the list stays.)
    fn dialog_select(&mut self) {
        match self.dialog {
            Some(Dialog::Days) => {
                self.days ^= 1 << self.dialog_row.min(6);
                self.draw_choices();
            }
            Some(Dialog::Tones) if !has_keys() => self.tone_touched(),
            Some(_) => self.dialog_done(),
            None => {}
        }
    }

    /// Takes the dialog's value into the alarm and closes the dialog.  A
    /// time that is not one keeps the dialog open.
    fn dialog_done(&mut self) {
        let (Some(dialog), Some(d)) = (self.dialog, &mut self.draft) else { return };
        match dialog {
            Dialog::Time => match self.typed.time(self.meridiem) {
                Some((hour, minute)) => (d.hour, d.minute) = (hour, minute),
                None => return,
            },
            Dialog::Days => d.days = self.days,
            Dialog::Tones => {
                alarms::preview(None);
                if let Some(tone) = alarms::tones().into_iter().nth(self.dialog_row as usize) {
                    d.tone = tone;
                }
            }
        }
        self.show_edit();
    }

    /// Closes the dialog and leaves the alarm as it was.  (Touch: the
    /// tone touched last is the alarm's.)
    fn dialog_cancel(&mut self) {
        if self.dialog == Some(Dialog::Tones) {
            alarms::preview(None);
        }
        self.show_edit();
    }

    // ------------------------------------------------------------- the keys

    /// Select, or OK, on what is highlighted.
    fn activate(&mut self) {
        match self.screen {
            Screen::List => self.edit_alarm(),
            Screen::Edit if self.dialog.is_some() => self.dialog_select(),
            Screen::Edit => self.open_dialog(self.edit_row),
            Screen::Error => self.show_list(),
        }
    }

    /// Cancel: a dialog is closed, a screen goes one back, and the list
    /// leaves the app.
    fn back(&mut self) {
        match self.screen {
            Screen::List => runtime::background(),
            Screen::Edit if self.dialog.is_some() => self.dialog_cancel(),
            Screen::Edit | Screen::Error => self.show_list(),
        }
    }

    fn key(&mut self, key: Key) {
        let typing = self.dialog == Some(Dialog::Time);
        match key {
            Key::Cancel => self.back(),
            Key::Select => self.activate(),
            Key::Left if typing => {
                self.typed.erase();
                self.draw_time();
            }
            Key::Digit(d) if typing => {
                self.typed.push(d);
                self.draw_time();
            }
            _ => {}
        }
    }

    fn softkey(&mut self, id: u32) {
        match id {
            SK_NEW => self.new_alarm(),
            SK_EDIT => self.edit_alarm(),
            SK_SWITCH => self.switch(),
            SK_DELETE => {
                if let Some(id) = self.selected().map(|a| a.id) {
                    self.delete(id);
                }
            }
            SK_DISMISS => self.after(alarms::dismiss()),
            SK_EXIT => runtime::background(),
            SK_SAVE => self.save(),
            SK_OK if self.dialog.is_some() => self.dialog_done(),
            SK_OK => self.activate(),
            SK_CANCEL => self.back(),
            SK_MERIDIEM => {
                self.typed.pm = !self.typed.pm;
                self.draw_time();
            }
            SK_ERASE => {
                self.typed.erase();
                self.draw_time();
            }
            SK_PLAY => alarms::preview(alarms::tones().get(self.dialog_row as usize).map(String::as_str)),
            _ => {}
        }
    }
}

impl App for Alarms {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => {
                // The phone's theme may have changed: what was on screen
                // is drawn again, from the start.
                self.touch = Controls::default();
                match self.screen {
                    Screen::Edit => {
                        // An open dialog stays open, as it stood.
                        let dialog = self.dialog;
                        self.show_edit();
                        self.dialog = dialog;
                        self.draw_dialog();
                    }
                    Screen::List | Screen::Error => self.show_list(),
                }
            }
            Event::Background | Event::Exit => {
                if self.dialog == Some(Dialog::Tones) {
                    alarms::preview(None);
                }
            }
            // The alarms changed elsewhere (the web interface, or an alarm
            // rang): the list follows; an alarm being edited is the user's.
            Event::AlarmsChanged if self.screen == Screen::List && runtime::in_foreground() => {
                self.show_list()
            }
            Event::Touch((w, t)) => self.touched(w, &t),
            Event::Widget((w, WidgetEvent::Highlighted(row))) if self.dialog.is_some() => {
                if w == self.field {
                    self.dialog_row = row;
                }
            }
            Event::Widget((w, WidgetEvent::Activated(row))) if self.dialog.is_some() => {
                if w == self.field {
                    self.dialog_row = row;
                    self.dialog_select();
                }
            }
            Event::Widget((w, WidgetEvent::Highlighted(row))) if w == self.widget => match self.screen {
                Screen::List => {
                    self.row = row;
                    self.list_softkeys();
                }
                Screen::Edit => self.edit_row = row,
                Screen::Error => {}
            },
            Event::Widget((w, WidgetEvent::Activated(row))) if w == self.widget => {
                match self.screen {
                    Screen::List => self.row = row,
                    // (Touch: the rows are fewer, and are not the keys'.)
                    Screen::Edit => self.edit_row = touch::field(row),
                    Screen::Error => {}
                }
                self.activate();
            }
            Event::Softkey(id) => self.softkey(id),
            Event::Key(k) => self.key(k),
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Alarms,
    name: "alarms",
    id: "org.opendeskphone.alarms",
    display_name: "Alarms",
    description: "Set the alarm clock",
    phone_icon: Some(Icon::AppAlarms),
}
