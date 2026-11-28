//! The numbers of the Timer app: a duration as it is set, the stopwatch,
//! and times as the screen shows them.

use std::ops::Range;

/// The largest hours, minutes, and seconds.
const MAX: [u8; 3] = [99, 59, 59];

/// A duration set with keys: hours, minutes, and seconds, one of them
/// chosen.  The arrows change the chosen one by one, and choose another;
/// digits type it, and two digits, or one that can only begin a number
/// too large, go on to the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    values: [u8; 3],
    chosen: usize,
    /// The first digit typed into the chosen field.
    half: Option<u8>,
}

impl Default for Fields {
    fn default() -> Fields {
        Fields {
            values: [0; 3],
            chosen: 1,
            half: None,
        }
    }
}

impl Fields {
    pub fn step(&mut self, up: bool) {
        self.half = None;
        let (v, max) = (&mut self.values[self.chosen], MAX[self.chosen]);
        *v = match (up, *v) {
            (true, v) if v == max => 0,
            (true, v) => v + 1,
            (false, 0) => max,
            (false, v) => v - 1,
        };
    }

    pub fn choose(&mut self, right: bool) {
        self.half = None;
        self.chosen = if right {
            (self.chosen + 1).min(2)
        } else {
            self.chosen.saturating_sub(1)
        };
    }

    pub fn digit(&mut self, d: u8) {
        let max = MAX[self.chosen];
        let d = d.min(9);
        match self.half.take() {
            Some(first) => self.values[self.chosen] = (first * 10 + d).min(max),
            None => {
                self.values[self.chosen] = d;
                if d * 10 <= max {
                    self.half = Some(d);
                    return;
                }
            }
        }
        self.chosen = (self.chosen + 1).min(2);
    }

    pub fn secs(&self) -> u64 {
        let [h, m, s] = self.values.map(u64::from);
        h * 3600 + m * 60 + s
    }

    /// `00:05:00`.
    pub fn text(&self) -> String {
        let [h, m, s] = self.values;
        format!("{h:02}:{m:02}:{s:02}")
    }

    /// Where the chosen field is in [`Fields::text`].
    pub fn chosen(&self) -> Range<usize> {
        3 * self.chosen..3 * self.chosen + 2
    }
}

/// A duration being typed: up to six digits, which fill the seconds, then
/// the minutes, then the hours, from the right, as on a microwave oven.
/// "130" is 1 minute and 30 seconds, and "90" is 90 seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    digits: Vec<u8>,
    /// The digits are of the last timer: the next digit starts over.
    fresh: bool,
}

impl Typed {
    pub fn push(&mut self, digit: u8) {
        if std::mem::take(&mut self.fresh) {
            self.digits.clear();
        }
        if (self.digits.is_empty() && digit == 0) || self.digits.len() == 6 {
            return;
        }
        self.digits.push(digit.min(9));
    }

    pub fn erase(&mut self) {
        self.fresh = false;
        self.digits.pop();
    }

    /// Keeps the digits on screen for the next timer, until a digit is
    /// typed.
    pub fn keep(&mut self) {
        self.fresh = true;
    }

    fn fields(&self) -> [u64; 3] {
        let mut padded = [0u8; 6];
        padded[6 - self.digits.len()..].copy_from_slice(&self.digits);
        let pair = |i: usize| u64::from(padded[i] * 10 + padded[i + 1]);
        [pair(0), pair(2), pair(4)]
    }

    pub fn secs(&self) -> u64 {
        let [h, m, s] = self.fields();
        h * 3600 + m * 60 + s
    }

    /// `00:01:30`.
    pub fn text(&self) -> String {
        let [h, m, s] = self.fields();
        format!("{h:02}:{m:02}:{s:02}")
    }
}

fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Time left on a timer, in whole seconds rounded up, so that a timer
/// shows 0:00 only when its time is up: `4:59`, or `1:04:59`.
pub fn left(ms: u64) -> String {
    clock(ms.div_ceil(1000))
}

/// Time on the stopwatch, to the tenth of a second: `0:12.3`.
pub fn tenths(ms: u64) -> String {
    format!("{}.{}", clock(ms / 1000), ms / 100 % 10)
}

/// A stopwatch, on the phone's monotonic clock in milliseconds.
#[derive(Clone, Debug, Default)]
pub struct Stopwatch {
    /// When the stopwatch was started last, while it runs.
    since: Option<u64>,
    /// The time counted before `since`.
    banked: u64,
    /// The time on the stopwatch at each lap.
    laps: Vec<u64>,
}

impl Stopwatch {
    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    /// At zero, and stopped.
    pub fn at_rest(&self) -> bool {
        self.since.is_none() && self.banked == 0
    }

    pub fn elapsed(&self, now: u64) -> u64 {
        self.banked + self.since.map_or(0, |s| now.saturating_sub(s))
    }

    pub fn start(&mut self, now: u64) {
        self.since.get_or_insert(now);
    }

    pub fn stop(&mut self, now: u64) {
        if let Some(s) = self.since.take() {
            self.banked += now.saturating_sub(s);
        }
    }

    pub fn lap(&mut self, now: u64) {
        if self.running() {
            self.laps.push(self.elapsed(now));
        }
    }

    pub fn reset(&mut self) {
        *self = Stopwatch::default();
    }

    /// The laps, the last first: each lap's number, from 1, and its own
    /// time.
    pub fn laps(&self) -> Vec<(usize, u64)> {
        let mut before = 0;
        let mut laps: Vec<(usize, u64)> = self
            .laps
            .iter()
            .enumerate()
            .map(|(i, &at)| (i + 1, at - std::mem::replace(&mut before, at)))
            .collect();
        laps.reverse();
        laps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_choose_a_field_and_change_it_around_its_range() {
        let mut f = Fields::default();
        f.step(false);
        assert_eq!(f.text(), "00:59:00");
        f.step(true);
        f.step(true);
        f.choose(false);
        f.choose(false);
        f.step(false);
        assert_eq!(f.text(), "99:01:00");
        f.choose(true);
        f.choose(true);
        f.choose(true);
        f.step(true);
        assert_eq!((f.text(), f.chosen()), ("99:01:01".into(), 6..8));
    }

    #[test]
    fn digits_fill_the_chosen_field_and_go_on_to_the_next() {
        let mut f = Fields::default();
        f.digit(1);
        assert_eq!((f.text(), f.chosen()), ("00:01:00".into(), 3..5));
        f.digit(5);
        f.digit(7);
        assert_eq!(
            (f.text(), f.chosen()),
            ("00:15:07".into(), 6..8),
            "7 cannot begin seconds"
        );
        f.digit(3);
        f.digit(0);
        f.digit(4);
        assert_eq!(
            (f.text(), f.secs()),
            ("00:15:04".into(), 904),
            "the seconds start over"
        );
    }

    fn typed(digits: &str) -> Typed {
        let mut t = Typed::default();
        digits.bytes().for_each(|d| t.push(d - b'0'));
        t
    }

    #[test]
    fn digits_fill_from_the_right() {
        assert_eq!(
            (typed("130").text(), typed("130").secs()),
            ("00:01:30".into(), 90)
        );
        assert_eq!(typed("90").secs(), 90);
        assert_eq!(typed("0012").text(), "00:00:12", "leading zeros");
        assert_eq!(typed("1234567").text(), "12:34:56", "six digits at most");
        let mut t = typed("15");
        t.erase();
        assert_eq!(t.secs(), 1);
    }

    #[test]
    fn a_kept_duration_is_replaced_by_the_next_digit_and_not_by_erase() {
        let mut t = typed("500");
        t.keep();
        t.push(2);
        assert_eq!(t.secs(), 2);
        let mut t = typed("500");
        t.keep();
        t.erase();
        t.push(1);
        assert_eq!(t.text(), "00:05:01");
    }

    #[test]
    fn times_show_hours_only_when_there_are_some() {
        assert_eq!(left(299_001), "5:00");
        assert_eq!(left(1), "0:01");
        assert_eq!(left(0), "0:00");
        assert_eq!(left(3_600_000), "1:00:00");
        assert_eq!(tenths(12_345), "0:12.3");
        assert_eq!(tenths(3_725_960), "1:02:05.9");
    }

    #[test]
    fn the_stopwatch_counts_only_while_it_runs() {
        let mut s = Stopwatch::default();
        assert!(s.at_rest());
        s.start(1_000);
        s.lap(4_000);
        s.stop(6_000);
        s.lap(9_000);
        assert_eq!(s.elapsed(9_000), 5_000);
        s.start(10_000);
        s.start(11_000);
        s.lap(12_000);
        assert_eq!(s.elapsed(12_000), 7_000);
        assert_eq!(s.laps(), [(2, 4_000), (1, 3_000)]);
        s.reset();
        assert!(s.at_rest() && s.laps().is_empty());
    }
}
