//! An app's view as a screen on the navigation stack.

use super::*;
use crate::appview::{AppEvent, AppView};
use crate::appview_impl::{self, Dispatch};

pub(crate) struct AppScreen {
    pub view: AppView,
    name: String,
    foreground: bool,
    /// Where the view was last drawn: a touch is at a place of the view.
    area: IRect,
}

impl AppScreen {
    pub fn new(view: AppView) -> AppScreen {
        let name = view.name().to_string();
        AppScreen { view, name, foreground: false, area: IRect::default() }
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
    fn tap(&mut self, _cx: &Cx, _hit: Hit, x: i32, y: i32) -> Nav {
        appview_impl::tap(&self.view, x - self.area.x, y - self.area.y);
        Nav::Stay
    }
    fn drag_at(&mut self, _cx: &Cx, _from: Option<Hit>, x: i32, y: i32, dx: i32, dy: i32) -> Nav {
        appview_impl::drag(&self.view, x - self.area.x, y - self.area.y, dx, dy);
        Nav::Stay
    }
    fn drag_end(&mut self, _cx: &Cx, _from: Option<Hit>) -> Nav {
        appview_impl::drag_end(&self.view);
        Nav::Stay
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
        self.area = area;
        let mut tree = self.view.batch_quiet();
        if cx.th.touch {
            c.hit(area, Hit::Custom(0));
        }
        appview_impl::render(&mut tree, c, cx.th, area, cx.now_ms);
        if cx.th.touch {
            // There is no X key: the way back is a button, over the
            // corner of the view that `Metrics::back_width` tells the app
            // to keep free.  It is the `Cancel` key, the app's if claimed.
            widgets::back_button(c, cx.th, IRect::new(area.x, area.y, cx.th.back_w, cx.th.title_h), cx.th.fg);
            // HOOK(D80 on-screen keyboard): this is where a focused app
            // `Input` on the touch skin should bring up the on-screen
            // keyboard (`crate::osk`).  `tree.keyboard_input()` is the
            // input and its mode while one has the focus (a tap on an
            // input focuses it), and `None` when the keyboard should go
            // away.  What the keyboard types goes in with
            // `self.view.type_text` and `self.view.backspace`, its Enter
            // key is `Key::Select` to `Screen::key` (which submits the
            // input), and it should cover the softkey buttons, not the
            // view (or the view's `Scroll` cannot bring the input out
            // from under it).
            let _ = tree.keyboard_input();
        }
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
