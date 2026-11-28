//! Timer: a countdown timer and a stopwatch, each on a screen of its own,
//! which run side by side and keep running while the app is off the
//! screen.  The time is drawn in large digits ([`digits`]).
//!
//! The timer's duration is set in hours, minutes, and seconds
//! ([`Fields`]): left and right choose one, which is highlighted, and up,
//! down, and the digits change it.  When the time is up, the app comes to
//! the front and rings the phone's first alarm tone for a minute, or until
//! dismissed.  As with the alarm clock, the timer does not ring into a
//! call: it rings when the call is over, and a call that begins silences
//! it.
//!
//! The stopwatch counts tenths of seconds and keeps laps, the last at the
//! top of the list.
//!
//! On the touchscreen, the two screens are two buttons under the title;
//! the duration is typed on a number pad as on a microwave oven
//! ([`Typed`]), and the actions are buttons across the bottom.

use opendeskphone_app_sdk::controls::{self, Button, GAP, Look};
use opendeskphone_app_sdk::prelude::*;
use opendeskphone_app_sdk::{Align, CallState, Font, alarms};
use std::ops::Range;

mod clock;
mod digits;

use clock::{Fields, Stopwatch, Typed, left, tenths};
use digits::Digits;

/// How long the timer rings, as long as an alarm does.
const RING_MS: u64 = 60_000;
/// An alarm tone plays for up to four seconds, so the tone starts again
/// this often.
const TONE_MS: u32 = 4_000;
/// How often a timer whose time came during a call looks for its end.
const WAIT_MS: u32 = 1_000;
/// How often the time on screen is drawn again.
const TICK_MS: u32 = 100;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Mode {
    #[default]
    Timer,
    Stopwatch,
}

impl Mode {
    /// The widest time the screen shows, which sets the digits' size.
    fn widest(self) -> &'static str {
        match self {
            Mode::Timer => "00:00:00",
            Mode::Stopwatch => "00:00.0",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ring {
    /// The time came during a call.
    Waiting,
    Ringing {
        since: u64,
    },
    Quiet,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Countdown {
    /// The duration is being set.
    #[default]
    Set,
    Running {
        end: u64,
    },
    Paused {
        left: u64,
    },
    Up(Ring),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    Show(Mode),
    Digit(u8),
    Erase,
    /// Up (`true`) or down on the chosen field.
    Step(bool),
    /// The field to the right (`true`) or left.
    Choose(bool),
    Start,
    Pause,
    Resume,
    Cancel,
    Dismiss,
    Run,
    Halt,
    Lap,
    Reset,
    Exit,
}

/// The actions of the softkeys: a softkey's ID is its place here, from 1.
const SOFT: [Act; 12] = [
    Act::Show(Mode::Timer),
    Act::Show(Mode::Stopwatch),
    Act::Start,
    Act::Pause,
    Act::Resume,
    Act::Cancel,
    Act::Dismiss,
    Act::Run,
    Act::Halt,
    Act::Lap,
    Act::Reset,
    Act::Exit,
];

fn softkey(act: Act, label: &str) -> Softkey {
    let id = SOFT
        .iter()
        .position(|a| *a == act)
        .map_or(0, |i| i as u32 + 1);
    Softkey::new(id, label)
}

/// The keys of the touchscreen's number pad, by rows.
const PAD: [[Option<Act>; 3]; 4] = [
    [
        Some(Act::Digit(1)),
        Some(Act::Digit(2)),
        Some(Act::Digit(3)),
    ],
    [
        Some(Act::Digit(4)),
        Some(Act::Digit(5)),
        Some(Act::Digit(6)),
    ],
    [
        Some(Act::Digit(7)),
        Some(Act::Digit(8)),
        Some(Act::Digit(9)),
    ],
    [None, Some(Act::Digit(0)), Some(Act::Erase)],
];

fn busy() -> bool {
    calls::list().iter().any(|c| c.state != CallState::Ended)
}

fn text(bounds: Rect, font: Font) -> Widget {
    let w = ui::create(WidgetKind::Text, ROOT, bounds);
    ui::set_font(w, font);
    ui::set_align(w, Align::Center);
    w
}

#[derive(Default)]
struct Timer {
    mode: Mode,
    /// The duration, as set with keys, and as typed on the touchscreen.
    fields: Fields,
    typed: Typed,
    countdown: Countdown,
    stopwatch: Stopwatch,
    /// The countdown's end, a one-shot timer.
    end_timer: u32,
    /// The tone's restarts, or the looks for a call's end.
    alert_timer: u32,
    /// The time on screen, while it changes.
    tick: u32,
    /// The time.
    face: Option<Digits>,
    /// The line under the time.
    note: Widget,
    laps: Widget,
    /// Touch: the buttons on the screen.
    buttons: Vec<(Button, Act)>,
}

impl Timer {
    // ------------------------------------------------------------ the timer

    fn duration(&self) -> u64 {
        if has_keys() {
            self.fields.secs()
        } else {
            self.typed.secs()
        }
    }

    fn run_for(&mut self, ms: u64) {
        self.end_timer = timers::set(ms as u32, false).unwrap_or(0);
        self.countdown = Countdown::Running {
            end: timers::now_ms() + ms,
        };
    }

    fn stop_countdown(&mut self) {
        timers::cancel(self.end_timer);
        timers::cancel(self.alert_timer);
        (self.end_timer, self.alert_timer) = (0, 0);
        if matches!(self.countdown, Countdown::Up(Ring::Ringing { .. })) {
            alarms::preview(None);
        }
        self.countdown = Countdown::Set;
        self.typed.keep();
    }

    fn time_up(&mut self) {
        self.end_timer = 0;
        self.countdown = Countdown::Up(Ring::Waiting);
        self.alert();
    }

    fn set_alert(&mut self, period: u32) {
        timers::cancel(self.alert_timer);
        self.alert_timer = timers::set(period, true).unwrap_or(0);
    }

    /// Rings, or goes on ringing, unless a call keeps it waiting.
    fn alert(&mut self) {
        let Countdown::Up(ring) = self.countdown else {
            return;
        };
        let now = timers::now_ms();
        let ring = match ring {
            Ring::Waiting if busy() => {
                if self.alert_timer == 0 {
                    self.set_alert(WAIT_MS);
                }
                Ring::Waiting
            }
            Ring::Waiting => {
                self.set_alert(TONE_MS);
                self.mode = Mode::Timer;
                if runtime::in_foreground() {
                    runtime::reset_idle_timer();
                } else {
                    let _ = runtime::show_app("timer");
                }
                self.tone();
                Ring::Ringing { since: now }
            }
            Ring::Ringing { since } if busy() || now.saturating_sub(since) >= RING_MS => {
                alarms::preview(None);
                timers::cancel(self.alert_timer);
                self.alert_timer = 0;
                Ring::Quiet
            }
            Ring::Ringing { since } => {
                self.tone();
                Ring::Ringing { since }
            }
            Ring::Quiet => Ring::Quiet,
        };
        self.countdown = Countdown::Up(ring);
    }

    fn tone(&self) {
        let tone = alarms::tones().into_iter().next().unwrap_or_default();
        alarms::preview(Some(&tone));
    }

    // ---------------------------------------------------------- the screens

    /// The time on screen, and which of its characters are highlighted.
    fn time_text(&self) -> (String, Range<usize>) {
        let now = timers::now_ms();
        let text = match (self.mode, self.countdown) {
            (Mode::Stopwatch, _) => tenths(self.stopwatch.elapsed(now)),
            (Mode::Timer, Countdown::Set) if has_keys() => {
                return (self.fields.text(), self.fields.chosen());
            }
            (Mode::Timer, Countdown::Set) => self.typed.text(),
            (Mode::Timer, Countdown::Running { end }) => left(end.saturating_sub(now)),
            (Mode::Timer, Countdown::Paused { left: ms }) => left(ms),
            (Mode::Timer, Countdown::Up(_)) => left(0),
        };
        (text, 0..0)
    }

    fn note_text(&self) -> &'static str {
        match (self.mode, self.countdown) {
            (Mode::Timer, Countdown::Paused { .. }) => "Paused",
            (Mode::Timer, Countdown::Up(_)) => "Time's up",
            _ => "",
        }
    }

    /// What Select does.
    fn primary(&self) -> Act {
        match (self.mode, self.countdown) {
            (Mode::Stopwatch, _) if self.stopwatch.running() => Act::Halt,
            (Mode::Stopwatch, _) => Act::Run,
            (Mode::Timer, Countdown::Set) => Act::Start,
            (Mode::Timer, Countdown::Running { .. }) => Act::Pause,
            (Mode::Timer, Countdown::Paused { .. }) => Act::Resume,
            (Mode::Timer, Countdown::Up(_)) => Act::Dismiss,
        }
    }

    fn softkeys(&self) -> Vec<Softkey> {
        let sw = &self.stopwatch;
        let mut keys = match (self.mode, self.countdown) {
            (Mode::Timer, Countdown::Up(_)) => return vec![softkey(Act::Dismiss, "Dismiss")],
            (Mode::Timer, Countdown::Set) => vec![softkey(Act::Start, "Start")],
            (Mode::Timer, Countdown::Running { .. }) => {
                vec![softkey(Act::Pause, "Pause"), softkey(Act::Cancel, "Cancel")]
            }
            (Mode::Timer, Countdown::Paused { .. }) => {
                vec![
                    softkey(Act::Resume, "Resume"),
                    softkey(Act::Cancel, "Cancel"),
                ]
            }
            (Mode::Stopwatch, _) if sw.running() => {
                vec![softkey(Act::Halt, "Stop"), softkey(Act::Lap, "Lap")]
            }
            (Mode::Stopwatch, _) if sw.at_rest() => vec![softkey(Act::Run, "Start")],
            (Mode::Stopwatch, _) => vec![softkey(Act::Run, "Start"), softkey(Act::Reset, "Reset")],
        };
        keys.push(match self.mode {
            Mode::Timer => softkey(Act::Show(Mode::Stopwatch), "Stopwatch"),
            Mode::Stopwatch => softkey(Act::Show(Mode::Timer), "Timer"),
        });
        keys.push(softkey(Act::Exit, "Exit"));
        keys
    }

    fn draw(&mut self) {
        self.buttons.clear();
        (self.face, self.note, self.laps) = (None, 0, 0);
        let area = screen(match self.mode {
            Mode::Timer => "Timer",
            Mode::Stopwatch => "Stopwatch",
        });
        let mut claims = vec![Key::Cancel, Key::Select];
        if self.mode == Mode::Timer {
            claims.extend([Key::Up, Key::Down, Key::Left, Key::Right]);
            claims.extend((0..=9).map(Key::Digit));
        }
        ui::claim_keys(ROOT, &claims);
        if has_keys() {
            self.draw_keys(area);
        } else {
            self.draw_touch(area);
        }
        self.draw_time();
        ui::set_text(self.note, self.note_text());
        if self.laps != 0 {
            let items: Vec<ListItem> = self
                .stopwatch
                .laps()
                .into_iter()
                .map(|(n, ms)| ListItem::with_detail(format!("Lap {n}"), tenths(ms)))
                .collect();
            ui::set_items(self.laps, &items);
        }
    }

    /// The time at `y`, across the window, at most `most` high: returns
    /// its height, which is as high as the digits need.
    fn place_face(&mut self, y: i16, most: u16) -> u16 {
        let widest = self.mode.widest();
        let height = Digits::height(widest, most);
        let width = ui::get_metrics().width;
        self.face = Some(Digits::new(Rect::new(0, y, width, height), widest));
        height
    }

    fn draw_time(&mut self) {
        let (text, lit) = self.time_text();
        if let Some(face) = &mut self.face {
            face.show(&text, lit);
        }
    }

    /// The time, and the line under it, in the middle of the room from
    /// `top` to `bottom`.
    fn centered(&mut self, top: i16, bottom: i16) {
        let m = ui::get_metrics();
        let room = (bottom - top).max(0) as u16;
        let height = Digits::height(self.mode.widest(), room.saturating_sub(m.line_height));
        let y = top + room.saturating_sub(height + m.line_height) as i16 / 2;
        self.place_face(y, height);
        self.note = text(
            Rect::new(0, y + height as i16, m.width, m.line_height),
            Font::Normal,
        );
    }

    fn draw_keys(&mut self, area: Rect) {
        let bottom = area.y + area.height as i16;
        match self.mode {
            Mode::Timer => self.centered(area.y, bottom),
            Mode::Stopwatch => {
                let height = self.place_face(area.y, area.height * 2 / 5);
                let list_y = area.y + height as i16;
                let list = Rect::new(0, list_y, area.width, (bottom - list_y).max(0) as u16);
                self.laps = ui::create(WidgetKind::TextList, ROOT, list);
            }
        }
        ui::set_softkeys(ROOT, &self.softkeys());
    }

    fn button(&mut self, bounds: Rect, label: &str, look: Look, act: Act) -> &mut Button {
        self.buttons
            .push((Button::new(ROOT, bounds, label, look), act));
        &mut self.buttons.last_mut().unwrap().0
    }

    fn draw_touch(&mut self, area: Rect) {
        let h = ui::get_metrics().row_height;
        let (rest, bottom) = controls::split_bottom(area);
        let floor = rest.y + rest.height as i16;
        let up = self.mode == Mode::Timer && matches!(self.countdown, Countdown::Up(_));
        let mut top = rest.y;
        if !up {
            let row = Rect::new(GAP as i16, rest.y + GAP as i16, rest.width - 2 * GAP, h);
            let modes = [("Timer", Mode::Timer), ("Stopwatch", Mode::Stopwatch)];
            for (place, (label, mode)) in controls::columns(row, 2).into_iter().zip(modes) {
                let on = mode == self.mode;
                self.button(place, label, Look::Secondary, Act::Show(mode))
                    .set_on(on);
            }
            top = row.y + (h + GAP) as i16;
        }
        let pair = controls::columns(bottom, 2);
        match (self.mode, self.countdown) {
            (Mode::Timer, Countdown::Set) => {
                let key_h = h + GAP;
                let pad_top = floor - (4 * key_h + 4 * GAP) as i16;
                for (r, keys) in PAD.iter().enumerate() {
                    let y = pad_top + (r as u16 * (key_h + GAP)) as i16;
                    let row = Rect::new(GAP as i16, y, area.width - 2 * GAP, key_h);
                    for (place, key) in controls::columns(row, 3).into_iter().zip(keys) {
                        let b = match key {
                            Some(Act::Digit(d)) => {
                                Button::new(ROOT, place, &d.to_string(), Look::Secondary)
                                    .with_font(Font::Large)
                            }
                            Some(Act::Erase) => {
                                Button::with_icon(ROOT, place, Icon::Backspace, Look::Secondary)
                            }
                            _ => continue,
                        };
                        self.buttons.push((b, key.unwrap()));
                    }
                }
                self.centered(top, pad_top);
                let set = self.typed.secs() > 0;
                self.button(bottom, "Start", Look::Primary, Act::Start)
                    .set_enabled(set);
            }
            (Mode::Timer, Countdown::Up(_)) => {
                self.centered(top, floor);
                self.button(bottom, "Dismiss", Look::Primary, Act::Dismiss);
            }
            (Mode::Timer, running) => {
                self.centered(top, floor);
                self.button(pair[0], "Cancel", Look::Secondary, Act::Cancel);
                match running {
                    Countdown::Paused { .. } => {
                        self.button(pair[1], "Resume", Look::Primary, Act::Resume)
                    }
                    _ => self.button(pair[1], "Pause", Look::Primary, Act::Pause),
                };
            }
            (Mode::Stopwatch, _) => {
                let height = self.place_face(top, ((floor - top).max(0) / 3) as u16);
                let list_y = top + height as i16;
                let list = Rect::new(0, list_y, area.width, (floor - list_y).max(0) as u16);
                self.laps = ui::create(WidgetKind::TextList, ROOT, list);
                let sw = &self.stopwatch;
                let (running, at_rest) = (sw.running(), sw.at_rest());
                if running || at_rest {
                    self.button(pair[0], "Lap", Look::Secondary, Act::Lap)
                        .set_enabled(running);
                } else {
                    self.button(pair[0], "Reset", Look::Secondary, Act::Reset);
                }
                if running {
                    self.button(pair[1], "Stop", Look::Primary, Act::Halt);
                } else {
                    self.button(pair[1], "Start", Look::Primary, Act::Run);
                }
            }
        }
    }

    // ------------------------------------------------------------ the input

    fn act(&mut self, act: Act) {
        let now = timers::now_ms();
        match act {
            Act::Show(mode) => self.mode = mode,
            Act::Digit(d) if has_keys() => self.fields.digit(d),
            Act::Digit(d) => self.typed.push(d),
            Act::Erase => self.typed.erase(),
            Act::Step(up) => self.fields.step(up),
            Act::Choose(right) => self.fields.choose(right),
            Act::Start => match self.duration() {
                0 => return,
                secs => self.run_for(secs * 1000),
            },
            Act::Pause => {
                if let Countdown::Running { end } = self.countdown {
                    timers::cancel(self.end_timer);
                    self.end_timer = 0;
                    self.countdown = Countdown::Paused {
                        left: end.saturating_sub(now),
                    };
                }
            }
            Act::Resume => {
                if let Countdown::Paused { left } = self.countdown {
                    self.run_for(left);
                }
            }
            Act::Cancel | Act::Dismiss => self.stop_countdown(),
            Act::Run => self.stopwatch.start(now),
            Act::Halt => self.stopwatch.stop(now),
            Act::Lap => self.stopwatch.lap(now),
            Act::Reset => self.stopwatch.reset(),
            Act::Exit => return runtime::background(),
        }
        self.draw();
    }

    fn key(&mut self, key: Key) {
        let setting = self.mode == Mode::Timer && self.countdown == Countdown::Set;
        let up = self.mode == Mode::Timer && matches!(self.countdown, Countdown::Up(_));
        let act = match key {
            Key::Cancel if up => Act::Dismiss,
            Key::Cancel => Act::Exit,
            Key::Select => self.primary(),
            Key::Digit(d) if setting => Act::Digit(d),
            Key::Up | Key::Down if setting => Act::Step(key == Key::Up),
            Key::Left | Key::Right if setting => Act::Choose(key == Key::Right),
            _ => return,
        };
        self.act(act);
    }

    /// The time on screen is drawn again while it changes there.
    fn sync_tick(&mut self) {
        let live = runtime::in_foreground()
            && match self.mode {
                Mode::Timer => matches!(self.countdown, Countdown::Running { .. }),
                Mode::Stopwatch => self.stopwatch.running(),
            };
        if live && self.tick == 0 {
            self.tick = timers::set(TICK_MS, true).unwrap_or(0);
        } else if !live && self.tick != 0 {
            timers::cancel(self.tick);
            self.tick = 0;
        }
    }
}

impl App for Timer {
    fn start(&mut self) {}

    fn handle(&mut self, event: Event) {
        match event {
            Event::Foreground => self.draw(),
            Event::Exit if matches!(self.countdown, Countdown::Up(Ring::Ringing { .. })) => {
                alarms::preview(None)
            }
            Event::Timer(id) if id == self.end_timer || id == self.alert_timer => {
                let before = (self.mode, self.countdown);
                if id == self.end_timer {
                    self.time_up();
                } else {
                    self.alert();
                }
                if runtime::in_foreground() && (self.mode, self.countdown) != before {
                    self.draw();
                }
            }
            Event::Timer(id) if id == self.tick => self.draw_time(),
            Event::Softkey(id) => {
                if let Some(act) = SOFT.get((id as usize).wrapping_sub(1)) {
                    self.act(*act);
                }
            }
            Event::Key(key) => self.key(key),
            Event::Touch((w, t)) => {
                // Every button is told, so that the one under the finger
                // looks pressed.
                let mut acted = None;
                for (b, act) in &mut self.buttons {
                    if b.touched(w, &t) {
                        acted = Some(*act);
                    }
                }
                if let Some(act) = acted {
                    self.act(act);
                }
            }
            _ => {}
        }
        self.sync_tick();
        let at_rest = self.countdown == Countdown::Set && self.stopwatch.at_rest();
        runtime::set_exit_after_background(at_rest);
    }
}

opendeskphone_app_sdk::export_app! {
    app: Timer,
    name: "timer",
    id: "org.opendeskphone.timer",
    display_name: "Timer",
    description: "A countdown timer and a stopwatch",
}
