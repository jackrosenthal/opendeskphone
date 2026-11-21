//! Touch (the touch skin, D80): where the screen can be touched, and what
//! a finger did.
//!
//! # Where the screen can be touched
//!
//! A screen says where it can be touched as it draws: for each thing a
//! finger can do something to, it registers the thing's rectangle and a
//! [`Hit`] with the canvas ([`crate::gfx::Canvas::hit`]).  The regions are
//! those of the picture on the screen: the UI forgets them all before
//! every drawing.  Where regions overlap, the one registered last is the
//! one touched (what is drawn later is on top).
//!
//! ```text
//! fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
//!     let button = IRect::new(area.x + 16, area.y + 16, 200, cx.th.touch_min);
//!     c.fill_round(button, 8, cx.th.sk_bg);
//!     c.hit(button, Hit::Custom(MUTE));        // -> Screen::tap
//!     c.hit(other, Hit::Key(Key::Line(2)));    // as if the key were pressed
//! }
//! fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
//!     match hit {
//!         Hit::Custom(MUTE) => { cx.post(Command::SetMute { on: true }); Nav::Stay }
//!         _ => Nav::Ignored,
//!     }
//! }
//! ```
//!
//! The widgets every screen shares register themselves on the touch skin:
//! the softkey buttons, the rows of [`crate::widgets::list`], the back and
//! home buttons of [`crate::widgets::title_bar`], the line tabs, and the
//! notices and the volume popup.  Only the touch skin's screens need
//! register anything ([`crate::skin::Theme::touch`]); on the other skins
//! nothing looks the regions up.
//!
//! # What a finger did
//!
//! [`Recognizer`] turns the touchscreen's events (one finger: down, moves,
//! up) into [`Gesture`]s: a tap, a long press, or a drag.  The UI
//! ([`crate::Ui::touch`]) sends a tap to the region under the place the
//! finger went down, and a drag to the top screen.  Before any gesture, the
//! top screen is told of a finger that came down on a region of its own
//! ([`crate::screens`]' `Screen::touch_down`), for what cannot wait for
//! the finger to be lifted (an app's game).
//!
//! # A finger on a control
//!
//! From when a finger comes down until it is lifted, or begins to drag,
//! the UI shows the region it came down on as pressed
//! ([`crate::gfx::Canvas::show_press`]): every button, key, and row, of
//! every screen, with nothing for the screen to do.  A region that is not
//! a control is registered with [`crate::gfx::Canvas::hit_quiet`] and shows
//! nothing; a screen that shows the finger in its own way asks where it
//! is ([`crate::gfx::Canvas::pressed_in`]).

use opendeskphone_state::{Key, TouchPhase};

/// What touching a region of the screen does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hit {
    /// What pressing this key does on a phone that has it: the key goes
    /// the way of every key (the screen's `key`, then the phone's own
    /// handling), down and up.  The lines are `Key::Line(n)`.
    Key(Key),
    /// The native softkey action with this id: the screen's `softkey`.
    Soft(u32),
    /// Row `row` of a list.  `list` tells the lists of one screen apart
    /// (0 where there is one; an app view's lists are its widgets' ids).
    /// Goes to the screen's `tap`; a screen that does not take it and has
    /// a [`crate::widgets::ListState`] of its own gets the row selected
    /// and then `Key::Select`.
    Row { list: u32, row: u32 },
    /// Something of the screen's own: goes to the screen's `tap` (and
    /// `long_press`) with this id.
    Custom(u32),
    /// Goes back to the first screen (the home screen), past every screen
    /// between, as the Line 1 key does for an app on a phone that has it;
    /// a call that is up stays on the screen.  The home button of
    /// [`crate::widgets::title_bar`].
    Home,
    /// Takes down the notice, or else the volume popup, that is shown, and
    /// does nothing else (the area around a dialog).
    Dismiss,
    /// Nothing: a region that keeps a touch from what is under it.
    Nothing,
}

/// What a finger did.  Positions are pixels of the main screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Down and up again, without moving farther than [`SLOP`], sooner
    /// than [`LONG_PRESS_MS`].  The place is where the finger went down.
    Tap { x: i32, y: i32 },
    /// Down, and still there, without having moved, after
    /// [`LONG_PRESS_MS`].  Nothing more comes of this touch.
    LongPress { x: i32, y: i32 },
    /// The finger moved, farther than [`SLOP`] from where it went down
    /// (`x`, `y`): `dx` and `dy` are how far it moved since the last
    /// `Drag` of this touch (the first has the whole way from where it
    /// went down).
    Drag { x: i32, y: i32, dx: i32, dy: i32 },
    /// The finger of a drag was lifted.
    DragEnd { x: i32, y: i32 },
}

/// How far a finger may move, in pixels, and still tap: farther is a drag.
/// (About 2 mm on the D80.)
pub const SLOP: i32 = 18;

/// How long a finger stays down, in milliseconds, for a long press.
pub const LONG_PRESS_MS: u64 = 600;

#[derive(Clone, Copy, Debug)]
struct Finger {
    /// Where it went down, and when.
    x0: i32,
    y0: i32,
    down_ms: u64,
    /// Where the last `Drag` left it.
    x: i32,
    y: i32,
    dragging: bool,
    /// The long press was reported: nothing more comes of this touch.
    done: bool,
}

/// Makes [`Gesture`]s of the events of one finger.
#[derive(Clone, Copy, Debug, Default)]
pub struct Recognizer {
    finger: Option<Finger>,
}

impl Recognizer {
    pub fn new() -> Recognizer {
        Recognizer::default()
    }

    /// Whether a finger is down.
    pub fn touching(&self) -> bool {
        self.finger.is_some()
    }

    /// One event of the touchscreen, at monotonic time `now_ms`.
    pub fn touch(&mut self, phase: TouchPhase, x: i32, y: i32, now_ms: u64) -> Option<Gesture> {
        match phase {
            TouchPhase::Down => {
                self.finger = Some(Finger { x0: x, y0: y, down_ms: now_ms, x, y, dragging: false, done: false });
                None
            }
            TouchPhase::Move => {
                let f = self.finger.as_mut()?;
                if f.done {
                    return None;
                }
                if !f.dragging && (x - f.x0).abs().max((y - f.y0).abs()) <= SLOP {
                    return None;
                }
                f.dragging = true;
                let (dx, dy) = (x - f.x, y - f.y);
                (f.x, f.y) = (x, y);
                (dx != 0 || dy != 0).then_some(Gesture::Drag { x: f.x0, y: f.y0, dx, dy })
            }
            TouchPhase::Up => {
                let f = self.finger.take()?;
                if f.done {
                    None
                } else if f.dragging {
                    Some(Gesture::DragEnd { x: f.x0, y: f.y0 })
                } else if now_ms.saturating_sub(f.down_ms) >= LONG_PRESS_MS {
                    // (The time for it passed without a `tick`.)
                    Some(Gesture::LongPress { x: f.x0, y: f.y0 })
                } else {
                    Some(Gesture::Tap { x: f.x0, y: f.y0 })
                }
            }
            TouchPhase::Cancel => {
                let f = self.finger.take()?;
                (f.dragging && !f.done).then_some(Gesture::DragEnd { x: f.x0, y: f.y0 })
            }
        }
    }

    /// When [`Recognizer::tick`] is to be called (monotonic milliseconds):
    /// when the finger that is down, and has not moved, has been down long
    /// enough for a long press.
    pub fn deadline(&self) -> Option<u64> {
        self.finger.filter(|f| !f.dragging && !f.done).map(|f| f.down_ms + LONG_PRESS_MS)
    }

    /// The time of [`Recognizer::deadline`] has come.
    pub fn tick(&mut self, now_ms: u64) -> Option<Gesture> {
        let f = self.finger.as_mut()?;
        if f.dragging || f.done || now_ms < f.down_ms + LONG_PRESS_MS {
            return None;
        }
        f.done = true;
        Some(Gesture::LongPress { x: f.x0, y: f.y0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TouchPhase::*;

    #[test]
    fn down_and_up_is_a_tap_where_the_finger_went_down() {
        let mut r = Recognizer::new();
        assert_eq!(r.touch(Down, 100, 200, 1000), None);
        assert!(r.touching());
        // A little movement is still a tap.
        assert_eq!(r.touch(Move, 100 + SLOP, 200 - SLOP, 1010), None);
        assert_eq!(r.touch(Up, 105, 195, 1100), Some(Gesture::Tap { x: 100, y: 200 }));
        assert!(!r.touching());
        // Nothing comes of an up, or a move, without a down.
        assert_eq!(r.touch(Up, 1, 1, 1200), None);
        assert_eq!(r.touch(Move, 1, 1, 1200), None);
    }

    #[test]
    fn a_finger_that_moves_drags_and_does_not_tap() {
        let mut r = Recognizer::new();
        r.touch(Down, 100, 500, 0);
        // The first drag has the whole way, the next ones what was added.
        assert_eq!(r.touch(Move, 100, 500 - SLOP - 1, 10), Some(Gesture::Drag { x: 100, y: 500, dx: 0, dy: -SLOP - 1 }));
        assert_eq!(r.touch(Move, 104, 440, 20), Some(Gesture::Drag { x: 100, y: 500, dx: 4, dy: 440 - (500 - SLOP - 1) }));
        // Back within the slop: still the drag.
        assert_eq!(r.touch(Move, 100, 500, 30), Some(Gesture::Drag { x: 100, y: 500, dx: -4, dy: 60 }));
        assert_eq!(r.deadline(), None);
        assert_eq!(r.tick(10_000), None);
        assert_eq!(r.touch(Up, 100, 500, 40), Some(Gesture::DragEnd { x: 100, y: 500 }));
    }

    #[test]
    fn a_finger_that_stays_is_a_long_press_once() {
        let mut r = Recognizer::new();
        r.touch(Down, 10, 20, 1000);
        assert_eq!(r.deadline(), Some(1000 + LONG_PRESS_MS));
        assert_eq!(r.tick(1000 + LONG_PRESS_MS - 1), None);
        assert_eq!(r.tick(1000 + LONG_PRESS_MS), Some(Gesture::LongPress { x: 10, y: 20 }));
        assert_eq!(r.deadline(), None);
        assert_eq!(r.tick(5000), None);
        // Neither moving nor lifting it does anything more.
        assert_eq!(r.touch(Move, 300, 300, 5000), None);
        assert_eq!(r.touch(Up, 300, 300, 5001), None);
        // Without a tick, the up reports it.
        r.touch(Down, 10, 20, 6000);
        assert_eq!(r.touch(Up, 10, 20, 6000 + LONG_PRESS_MS), Some(Gesture::LongPress { x: 10, y: 20 }));
    }

    #[test]
    fn a_canceled_touch_does_nothing() {
        let mut r = Recognizer::new();
        r.touch(Down, 10, 20, 0);
        assert_eq!(r.touch(Cancel, 10, 20, 5), None);
        assert!(!r.touching());
        r.touch(Down, 10, 20, 10);
        r.touch(Move, 10, 200, 20);
        assert_eq!(r.touch(Cancel, 10, 200, 30), Some(Gesture::DragEnd { x: 10, y: 20 }));
    }
}
