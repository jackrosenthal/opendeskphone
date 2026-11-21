//! Calendar: one month at a time, with today marked.  Left and Right (or
//! Up and Down) change the month; on the touchscreen, a swipe across the
//! month does, as the softkeys do.  The date is the phone's clock in local
//! time; the phones have no clock chip, so until the time has been set
//! there is no calendar to show.

use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{Align, Font};

/// Touch: how far a finger moves sideways, in pixels, to turn the month.
const SWIPE: i32 = 80;

const SK_PREV: u32 = 1;
const SK_TODAY: u32 = 2;
const SK_NEXT: u32 = 3;
const SK_EXIT: u32 = 4;

/// The softkeys the touchscreen does without: the back button is the way
/// out.
fn keys(keys: &[Softkey]) -> Vec<Softkey> {
    opendeskphone_app_sdk::nav::softkeys(keys, &[SK_EXIT])
}

/// Sets the window's softkeys: of `all`, those the phone needs.
fn set_keys(all: &[Softkey]) {
    ui::set_softkeys(ROOT, &keys(all));
}

/// A wall clock at or before this (September 2020) has not been set; the
/// idle screen's clock uses the same test.
const CLOCK_UNSET_UNTIL: u64 = 1_600_000_000;
/// How often the date is checked while the calendar is on screen.
const CHECK_MS: u32 = 5_000;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Days from 1970-01-01 to a date of the Gregorian calendar.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day counted from 1970-01-01.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((yoe + era * 400 + i64::from(month <= 2)) as i32, month, day)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (days_from_civil(ny, nm, 1) - days_from_civil(year, month, 1)) as u32
}

/// Weekday of the first of a month: 0 is Sunday.
fn first_weekday(year: i32, month: u32) -> u32 {
    (days_from_civil(year, month, 1) + 4).rem_euclid(7) as u32
}

/// Today's local date, or `None` while the clock has not been set.
fn today() -> Option<(i32, u32, u32)> {
    let now = timers::wall_time();
    if now <= CLOCK_UNSET_UNTIL {
        return None;
    }
    let local = now as i64 + i64::from(l10n::utc_offset());
    Some(civil_from_days(local.div_euclid(86_400)))
}

/// The month grid: a title and six weeks of day cells.
struct Grid {
    title: Widget,
    cells: Vec<Widget>,
    /// The cell marked as today.
    marked: Option<Widget>,
}

#[derive(Default)]
struct Calendar {
    grid: Option<Grid>,
    /// The month shown.
    year: i32,
    month: u32,
    today: Option<(i32, u32, u32)>,
    timer: Option<u32>,
}

impl Calendar {
    fn build() -> Grid {
        let m = ui::get_metrics();
        ui::clear(ROOT);
        ui::claim_keys(ROOT, &[Key::Left, Key::Right, Key::Up, Key::Down, Key::Cancel]);
        // Touch: a swipe turns the month, and the title is centered
        // between the back button and as much space at the right.
        ui::claim_touch(ROOT, !m.keys);
        let title = ui::create(
            WidgetKind::Text,
            ROOT,
            Rect::new(
                m.back_width as i16,
                0,
                m.width.saturating_sub(2 * m.back_width),
                m.title_height,
            ),
        );
        ui::set_font(title, Font::Title);
        ui::set_align(title, Align::Center);

        let top = m.title_height;
        let col_w = m.width / 7;
        let left = (m.width - col_w * 7) / 2;
        // The weekdays' row and the six rows of days share the height
        // under the title.  Touch: the upright screen has more height
        // than a month wants; the weekdays have a low row, and a row of
        // days is at most a quarter higher than a day is wide.
        let below = m.height.saturating_sub(top);
        let (head_h, row_h) = if m.keys {
            (below / 7, below / 7)
        } else {
            let head_h = m.line_height * 3 / 2;
            (head_h, ((below - head_h) / 6).min(col_w * 5 / 4))
        };
        // The mark on today is as wide as two digits need, not the
        // column.  Touch: a square in the middle of the day's place.
        let (cell_w, cell_h) = if m.keys {
            (col_w.min(row_h * 2), row_h)
        } else {
            let side = col_w.min(row_h) * 5 / 6;
            (side, side)
        };
        let (inset_x, inset_y) = ((col_w - cell_w) / 2, (row_h - cell_h) / 2);
        for (i, name) in WEEKDAYS.iter().enumerate() {
            let x = (left + col_w * i as u16) as i16;
            let w = ui::create(
                WidgetKind::Text,
                ROOT,
                Rect::new(x, top as i16, col_w, head_h),
            );
            ui::set_font(w, Font::Small);
            ui::set_align(w, Align::Center);
            ui::set_text(w, if col_w < 60 { &name[..2] } else { name });
        }
        let mut cells = Vec::with_capacity(42);
        for i in 0..42u16 {
            let x = (left + col_w * (i % 7) + inset_x) as i16;
            let y = (top + head_h + row_h * (i / 7) + inset_y) as i16;
            let w = ui::create(WidgetKind::Text, ROOT, Rect::new(x, y, cell_w, cell_h));
            ui::set_align(w, Align::Center);
            cells.push(w);
        }
        let mut keys = vec![
            Softkey::new(SK_PREV, "Prev"),
            Softkey::new(SK_TODAY, "Today"),
            Softkey::new(SK_NEXT, "Next"),
        ];
        // (Touch: the back button leaves.)
        if m.keys {
            keys.push(Softkey::new(SK_EXIT, "Exit"));
        }
        set_keys(&keys);
        Grid {
            title,
            cells,
            marked: None,
        }
    }

    /// Shows the month `self.year`/`self.month`, or that the date is not
    /// set.
    fn show(&mut self) {
        if self.today.is_none() {
            self.grid = None;
            Message::show(
                "Calendar",
                "The date is not set.",
                &keys(&[Softkey::new(SK_EXIT, "Exit")]),
            );
            return;
        }
        let grid = self.grid.get_or_insert_with(Self::build);
        ui::set_text(
            grid.title,
            &format!("{} {}", MONTHS[(self.month - 1) as usize], self.year),
        );
        let first = first_weekday(self.year, self.month);
        let days = days_in_month(self.year, self.month);
        let today = self
            .today
            .filter(|t| (t.0, t.1) == (self.year, self.month))
            .map(|t| t.2);
        if let Some(w) = grid.marked.take() {
            ui::set_highlight(w, false);
        }
        for (i, w) in grid.cells.iter().enumerate() {
            let day = (i as u32 + 1).wrapping_sub(first);
            if (1..=days).contains(&day) {
                ui::set_text(*w, &day.to_string());
                if today == Some(day) {
                    ui::set_highlight(*w, true);
                    grid.marked = Some(*w);
                }
            } else {
                ui::set_text(*w, "");
            }
        }
    }

    fn show_today(&mut self) {
        if let Some((y, m, _)) = self.today {
            (self.year, self.month) = (y, m);
        }
        self.show();
    }

    /// Moves by `step` months, within the years 1 to 9999.
    fn turn(&mut self, step: i32) {
        if self.today.is_none() {
            return;
        }
        let n = self.year * 12 + self.month as i32 - 1 + step;
        let (year, month) = (n.div_euclid(12), n.rem_euclid(12) as u32 + 1);
        if (1..=9999).contains(&year) {
            (self.year, self.month) = (year, month);
            self.show();
        }
    }

    /// The date may have changed: midnight passed, or the clock was set.
    fn check_date(&mut self) {
        let now = today();
        if now == self.today {
            return;
        }
        // A calendar showing the old current month follows the date.
        let follows = match self.today {
            Some((y, m, _)) => (y, m) == (self.year, self.month),
            None => true,
        };
        self.today = now;
        if follows {
            self.show_today();
        } else {
            self.show();
        }
    }
}

impl App for Calendar {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => {
                self.today = today();
                self.show_today();
                if self.timer.is_none() {
                    self.timer = timers::set(CHECK_MS, true).ok();
                }
            }
            Event::Background => {
                if let Some(t) = self.timer.take() {
                    timers::cancel(t);
                }
            }
            Event::Timer(_) => self.check_date(),
            Event::Softkey(SK_PREV) => self.turn(-1),
            Event::Softkey(SK_NEXT) => self.turn(1),
            Event::Softkey(SK_TODAY) => self.show_today(),
            Event::Softkey(SK_EXIT) => runtime::background(),
            // A swipe to the left brings the next month in from the
            // right, as a page is turned.
            Event::Touch((_, Touch::DragEnd((from, to)))) => {
                let (dx, dy) = (i32::from(to.x) - i32::from(from.x), i32::from(to.y) - i32::from(from.y));
                if dx.abs() >= SWIPE && dx.abs() > dy.abs() {
                    self.turn(if dx < 0 { 1 } else { -1 });
                }
            }
            Event::Key(k) => match k {
                Key::Left | Key::Up => self.turn(-1),
                Key::Right | Key::Down => self.turn(1),
                Key::Cancel => runtime::background(),
                _ => {}
            },
            _ => {}
        }
    }
}

opendeskphone_app_sdk::export_app! {
    app: Calendar,
    name: "calendar",
    id: "org.opendeskphone.calendar",
    display_name: "Calendar",
    description: "Month calendar",
    phone_icon: Some(Icon::AppCalendar),
}
