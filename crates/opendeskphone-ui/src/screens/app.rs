//! An app's view as a screen on the navigation stack.
//!
//! Touch: while an `Input` of the view has the focus, the on-screen
//! keyboard ([`crate::osk`]) is across the bottom of the screen, over the
//! softkey buttons and the lower part of the view, in the layout of the
//! input's mode.  The view keeps its size; if the input would be under
//! the keyboard, the view is drawn as far up as brings the input above
//! it.  The keyboard's done key submits the input (as [`Key::Select`]
//! does) and puts the keyboard away; so do the back button and a touch
//! on the view beside the input, without submitting.  A touch on the
//! input brings the keyboard back.

use super::*;
use crate::appview::{AppEvent, AppView, WidgetId};
use crate::appview_impl::{self, Dispatch};
use crate::osk::{Keyboard, Press};

/// Touch: the region of the view.
const VIEW: u32 = 0;
/// Touch: the region of the back button while the keyboard is up, which
/// puts the keyboard away.
const KEYBOARD_AWAY: u32 = 1;

pub(crate) struct AppScreen {
    pub view: AppView,
    name: String,
    foreground: bool,
    /// Where the view was last drawn: a touch is at a place of the view.
    area: IRect,
    /// Touch: the keyboard, and the input it was last up for.
    kb: Keyboard,
    kb_for: Option<WidgetId>,
    /// Touch: the focused input whose keyboard the user put away.
    put_away: Option<WidgetId>,
}

impl AppScreen {
    pub fn new(view: AppView) -> AppScreen {
        let name = view.name().to_string();
        AppScreen {
            view,
            name,
            foreground: false,
            area: IRect::default(),
            kb: Keyboard::new(crate::appview::InputMode::Text),
            kb_for: None,
            put_away: None,
        }
    }

    /// Touch: the input the keyboard is up for, and its mode.
    fn keyboard(&self) -> Option<(WidgetId, crate::appview::InputMode)> {
        self.view.keyboard_input().filter(|(id, _)| self.put_away != Some(*id))
    }

    /// Touch: where the view is drawn in `screen` (the whole screen) while
    /// the keyboard, in `keys`, is up for `input`: in its own place above
    /// the softkey buttons' row, or as far up as brings the input above
    /// the keyboard.
    fn view_area(&self, th: &Theme, screen: IRect, keys: IRect, input: WidgetId) -> IRect {
        let area = if self.full_screen() { screen } else { screen.split_bottom(th.softkey_h).0 };
        let Some(r) = appview_impl::widget_rect(&self.view.batch_quiet(), input) else { return area };
        // (No farther than the keyboard covers the view: nothing shows
        // between the two.)
        let up = (area.y + r.bottom() + th.pad - keys.y).clamp(0, (area.bottom() - keys.y).max(0));
        IRect { y: area.y - up, ..area }
    }

    /// Tells the app whether it is the top screen (once per change).
    pub fn set_foreground(&mut self, on: bool) {
        if self.foreground != on {
            self.foreground = on;
            self.view.push_event(if on { AppEvent::Foreground } else { AppEvent::Background });
        }
    }
}

impl Screen for AppScreen {
    fn name(&self) -> String {
        format!("app:{}", self.name)
    }
    fn tag(&self) -> Tag {
        Tag::App
    }
    fn app_name(&self) -> Option<&str> {
        Some(&self.name)
    }
    /// The app's `theme-color` answers change.  An app in the foreground
    /// is told it is in the foreground again, on which apps draw anew;
    /// one in the background is told when it comes forward, as ever.
    fn theme_changed(&mut self, th: &'static Theme) {
        self.view.set_theme(th);
        if self.foreground {
            self.view.push_event(AppEvent::Foreground);
        }
    }
    fn raw_softkeys(&self, _cx: &Cx) -> Option<[String; 4]> {
        Some(appview_impl::softkey_labels(&self.view.batch_quiet()))
    }
    fn full_screen(&self) -> bool {
        self.view.batch_quiet().full_screen
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match appview_impl::dispatch(&self.view, key, cx.now_ms) {
            Dispatch::Consumed => Nav::Stay,
            Dispatch::Ignored => Nav::Ignored,
            // Stock: Line 1 backgrounds a nonprivileged app.
            Dispatch::Background => Nav::Home,
        }
    }
    // Touch.  The whole view is one region (`draw`): what a finger does
    // goes by the place in the view, to the widget that is there.
    fn tap(&mut self, cx: &Cx, hit: Hit, x: i32, y: i32) -> Nav {
        if hit == Hit::Custom(KEYBOARD_AWAY) {
            self.put_away = self.keyboard().map(|(id, _)| id);
            return Nav::Stay;
        }
        if let Some(press) = self.kb.tap(hit) {
            match press {
                Press::Char(ch) => {
                    self.view.type_text(ch.encode_utf8(&mut [0; 4]));
                }
                Press::Backspace => {
                    self.view.backspace();
                }
                // The input is submitted.  The keyboard goes away, and
                // comes up again if the app gives another input the
                // focus (the next field of a form).
                Press::Done => {
                    self.put_away = self.keyboard().map(|(id, _)| id);
                    return self.key(cx, Key::Select);
                }
                Press::Layout => {}
            }
            return Nav::Stay;
        }
        let up = self.keyboard().is_some();
        let (vx, vy) = (x - self.area.x, y - self.area.y);
        appview_impl::tap(&self.view, vx, vy);
        // On the focused input: its keyboard comes back.  Beside it: the
        // keyboard is put away.
        if let Some((id, _)) = self.view.keyboard_input() {
            let on_input = appview_impl::widget_rect(&self.view.batch_quiet(), id)
                .is_some_and(|r| vx >= r.x && vx < r.right() && vy >= r.y && vy < r.bottom());
            if on_input {
                self.put_away = None;
            } else if up {
                self.put_away = Some(id);
            }
        }
        Nav::Stay
    }
    fn drag_at(&mut self, _cx: &Cx, from: Option<Hit>, x: i32, y: i32, dx: i32, dy: i32) -> Nav {
        // A finger that slides on the keyboard types nothing and moves
        // nothing.
        if !from.is_some_and(Keyboard::owns) {
            appview_impl::drag(&self.view, x - self.area.x, y - self.area.y, dx, dy);
        }
        Nav::Stay
    }
    fn drag_end(&mut self, _cx: &Cx, _from: Option<Hit>) -> Nav {
        appview_impl::drag_end(&self.view);
        Nav::Stay
    }
    /// Touch: the keyboard is where the softkey buttons are.
    fn softkey_bar(&self, _cx: &Cx) -> bool {
        self.keyboard().is_none()
    }
    fn captures_all_keys(&self) -> bool {
        self.view.batch_quiet().capture
    }
    fn hook(&mut self, _cx: &Cx, off_hook: bool) -> Nav {
        if !self.captures_all_keys() {
            return Nav::Ignored;
        }
        self.view.push_event(AppEvent::Hook { off_hook });
        Nav::Stay
    }
    fn lamps(&self) -> Vec<(Lamp, LampColor)> {
        self.view.batch_quiet().lamps()
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        self.view.take_dirty();
        let th = cx.th;
        // (An input that lost the focus has its keyboard again when it
        // next gets it.)
        if self.put_away.is_some() && self.view.keyboard_input().map(|(id, _)| id) != self.put_away {
            self.put_away = None;
        }
        let Some((input, mode)) = self.keyboard() else {
            self.kb_for = None;
            self.area = area;
            let mut tree = self.view.batch_quiet();
            if th.touch {
                c.hit(area, Hit::Custom(VIEW));
            }
            appview_impl::render(&mut tree, c, th, area, cx.now_ms);
            if th.touch {
                // There is no X key: the way back is a button, over the
                // corner of the view that `Metrics::back_width` tells the
                // app to keep free.  It is the `Cancel` key, the app's if
                // claimed.
                widgets::back_button(c, th, IRect::new(area.x, area.y, th.back_w, th.title_h), th.fg);
            }
            return;
        };
        // The keyboard is up (`softkey_bar`: `area` is the whole screen),
        // on its first page for an input it was not up for before.
        if self.kb_for != Some(input) {
            self.kb = Keyboard::new(mode);
            self.kb_for = Some(input);
        }
        self.kb.set_mode(mode);
        let (_, keys) = area.split_bottom(Keyboard::height(th));
        self.area = self.view_area(th, area, keys, input);
        appview_impl::render(&mut self.view.batch_quiet(), c, th, self.area, cx.now_ms);
        // A `Scroll` that brought the input into itself as it was drawn
        // moved the input: once more, where the view is to be now.
        let moved = self.view_area(th, area, keys, input);
        if moved != self.area {
            self.area = moved;
            appview_impl::render(&mut self.view.batch_quiet(), c, th, self.area, cx.now_ms);
        }
        c.hit(self.area, Hit::Custom(VIEW));
        // The back button, where it always is, puts the keyboard away.
        // (Over a view drawn farther up it is not over the corner the
        // app keeps free: on a ground of its own.)
        let back = IRect::new(area.x, area.y, th.back_w, th.title_h);
        if self.area.y != area.y {
            c.fill(back, th.bg);
        }
        if let Some(icon) = th.icon("left") {
            c.icon_in(icon, back, th.fg);
        }
        c.hit(back, Hit::Custom(KEYBOARD_AWAY));
        // (Between the keys nothing is touched: not the view under them.)
        c.hit(keys, Hit::Nothing);
        self.kb.draw(c, th, keys);
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        self.view.batch_quiet().multitap.deadline_ms().filter(|d| *d > cx.now_ms)
    }
    fn dirty(&self) -> bool {
        self.view.is_dirty()
    }
    fn leave(&mut self, _cx: &Cx) {
        // A drag does not outlast the screen.
        appview_impl::drag_end(&self.view);
        self.set_foreground(false);
    }
    fn enter(&mut self, _cx: &Cx) {
        self.set_foreground(true);
    }
}
