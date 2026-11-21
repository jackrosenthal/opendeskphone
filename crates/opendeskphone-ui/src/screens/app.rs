//! An app's view as a screen on the navigation stack.
//!
//! Touch: the top of a view that is not full-screen is the title bar
//! every screen has.  The phone paints the bar's ground and draws its
//! buttons, back at the left (the `Cancel` key, the app's if claimed) and
//! home at the right; the app puts its title between them
//! ([`appview_impl::render_titled`]).  A full-screen view, in which the
//! app draws what it likes, has only the back button, on a round button
//! of its own, which shows on anything.
//!
//! Touch: while an `Input` of the view has the focus, the on-screen
//! keyboard ([`crate::osk`]) is across the bottom of the screen, over the
//! softkey buttons and the lower part of the view, in the layout of the
//! input's mode.  The view keeps its size; if the input would be under
//! the keyboard, the view is drawn as far up as brings the input above
//! it, under the title bar, which stays where it is.  The keyboard's
//! action key goes on to the view's next input ("Next"); from the last
//! one ("Done") it submits the input (as [`Key::Select`] does) and puts
//! the keyboard away.  So do the back button and a touch on the view
//! beside the input, without submitting.  A touch on the input brings the
//! keyboard back.
//!
//! Touch: a view with more than four softkey actions shows three and
//! "More", which opens a menu of the others over the view.

use super::*;
use crate::appview::{AppEvent, AppView, WidgetId};
use crate::appview_impl::{self, Dispatch};
use crate::osk::{Keyboard, Press};

/// Touch: the region of the view.
const VIEW: u32 = 0;
/// Touch: the region of the back button while the keyboard is up, which
/// puts the keyboard away.
const KEYBOARD_AWAY: u32 = 1;
/// Touch: the region beside the menu of "More", which puts the menu away.
const MENU_AWAY: u32 = 2;
/// Touch: the regions of the menu's items, from this id up, in the order
/// of [`appview_impl::softkey_overflow`].
const MENU_ITEM: u32 = 0x100;
/// (The most items a menu shows.)
const MENU_MAX: u32 = 0x100;

/// Touch: the back button's arrow in `back`, on a round button in the
/// colors of the softkey buttons (a full-screen view's).
fn back_disc(c: &mut Canvas, th: &Theme, back: IRect) {
    let d = back.w.min(back.h) - 2 * th.px(6);
    let disc = IRect::new(back.x + (back.w - d) / 2, back.y + (back.h - d) / 2, d, d);
    c.fill_round_aa(disc, d / 2, th.sk_edge);
    c.fill_round_aa(disc.inset(2), d / 2 - 2, th.sk_bg);
    if let Some(icon) = th.icon("left") {
        c.icon_in(icon, back, th.sk_fg);
    }
}

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
    /// Touch: the menu of "More" is open.
    menu: bool,
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
            menu: false,
        }
    }

    /// Touch: the input the keyboard is up for, and its mode.
    fn keyboard(&self) -> Option<(WidgetId, crate::appview::InputMode)> {
        self.view.keyboard_input().filter(|(id, _)| self.put_away != Some(*id))
    }

    /// Touch: the view's own place in `screen` (the whole screen): all of
    /// it, or what is above the softkey buttons' row.
    fn view_home(&self, th: &Theme, screen: IRect) -> IRect {
        if self.full_screen() { screen } else { screen.split_bottom(th.softkey_h).0 }
    }

    /// Touch: registers what looks pressed under a finger on the view, if
    /// anything does ([`appview_impl::press_rect`]): a region of the
    /// view's own, over the view's, so that a touch there is still one on
    /// the view.
    fn press(&self, c: &mut Canvas, tree: &crate::appview::Tree) {
        if let Some((x, y)) = c.pressed()
            && let Some(r) = appview_impl::press_rect(tree, x - self.area.x, y - self.area.y)
        {
            c.hit(IRect { x: r.x + self.area.x, y: r.y + self.area.y, ..r }, Hit::Custom(VIEW));
        }
    }

    /// Touch: the menu of "More" over the view in `area`: a button for
    /// each of `items`, one above the other at the bottom right, over the
    /// "More" button.  A touch beside it puts it away.
    fn draw_menu(&self, c: &mut Canvas, th: &Theme, area: IRect, items: &[(u32, String)]) {
        c.hit_quiet(area, Hit::Custom(MENU_AWAY));
        let gap = th.px(4);
        let pitch = th.row_h + gap;
        let n = (items.len() as i32).min((area.h - th.title_h) / pitch).min(MENU_MAX as i32);
        let w = area.w / 2;
        let card = IRect::new(area.right() - th.soft_x0 - w, area.bottom() - gap - (n * pitch + gap), w, n * pitch + gap);
        c.fill_round_aa(card.inset(-th.scale), th.px(7), th.sk_edge);
        c.fill_round_aa(card, th.px(6), th.modal_bg);
        c.hit(card, Hit::Nothing);
        for (i, (_, label)) in items.iter().take(n as usize).enumerate() {
            let r = IRect::new(card.x + gap, card.y + gap + i as i32 * pitch, card.w - 2 * gap, th.row_h);
            widgets::button_styled(c, th, r, label, widgets::ButtonStyle::Secondary, Hit::Custom(MENU_ITEM + i as u32));
        }
    }

    /// Touch: where the view is drawn in `screen` (the whole screen) while
    /// the keyboard, in `keys`, is up for `input`: in its own place above
    /// the softkey buttons' row, or as far up as brings the input above
    /// the keyboard.
    fn view_area(&self, th: &Theme, screen: IRect, keys: IRect, input: WidgetId) -> IRect {
        let area = self.view_home(th, screen);
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
        // Touch: "More" opens and closes the menu of the actions the
        // softkey buttons have no room for; anything else closes it.
        let more = cx.th.touch()
            && key == Key::Soft(4)
            && !self.full_screen()
            && !appview_impl::softkey_overflow(&self.view.batch_quiet()).is_empty();
        self.menu = more && !self.menu;
        if more {
            return Nav::Stay;
        }
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
        // The menu of "More": an item does what its softkey does, and a
        // touch beside the menu only closes it.
        if std::mem::take(&mut self.menu) {
            if let Hit::Custom(id @ MENU_ITEM..) = hit
                && id < MENU_ITEM + MENU_MAX
                && let Some((id, _)) = appview_impl::softkey_overflow(&self.view.batch_quiet()).get((id - MENU_ITEM) as usize)
            {
                self.view.push_event(AppEvent::Softkey { id: *id });
            }
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
                // On to the next input, if the view has one.  From the
                // last, the input is submitted: the keyboard goes away,
                // and comes up again if the app gives another input the
                // focus.
                Press::Done => {
                    if appview_impl::focus_next_input(&self.view) {
                        return Nav::Stay;
                    }
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
    fn touch_down(&mut self, _cx: &Cx, hit: Hit, x: i32, y: i32) {
        // (Not on the keyboard, the menu, or a button of the phone's.)
        if hit == Hit::Custom(VIEW) {
            appview_impl::down(&self.view, x - self.area.x, y - self.area.y);
        }
    }
    fn drag_at(&mut self, _cx: &Cx, from: Option<Hit>, x: i32, y: i32, dx: i32, dy: i32) -> Nav {
        // A finger that slides on the keyboard, or on the menu, types
        // nothing and moves nothing.
        if matches!(from, None | Some(Hit::Custom(VIEW))) {
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
        let full = self.full_screen();
        let Some((input, mode)) = self.keyboard() else {
            self.kb_for = None;
            self.area = area;
            let mut tree = self.view.batch_quiet();
            if !th.touch() {
                return appview_impl::render(&mut tree, c, th, area, cx.now_ms);
            }
            if full {
                appview_impl::render(&mut tree, c, th, area, cx.now_ms);
            } else {
                // The title bar's ground is the phone's, and the title on
                // it is the app's.
                appview_impl::render_titled(&mut tree, c, th, area, cx.now_ms);
            }
            // The whole view is one region (over the regions its lists
            // registered as they were drawn), which shows nothing under a
            // finger itself: the row of a list under the finger does.
            c.hit_quiet(area, Hit::Custom(VIEW));
            self.press(c, &tree);
            if full {
                // There is no X key: the way back is a button, over the
                // corner of the view that `Metrics::back_width` tells the
                // app to keep free.  It is the `Cancel` key, the app's if
                // claimed.
                let back = IRect::new(area.x, area.y, th.back_w, th.title_h);
                back_disc(c, th, back);
                c.hit(back, Hit::Key(Key::Cancel));
                return;
            }
            // The title bar's buttons are the phone's.
            widgets::title_buttons(c, th, IRect { h: th.title_h, ..area }, th.title_fg);
            // (The menu goes with the softkeys it was opened for.)
            let items = appview_impl::softkey_overflow(&tree);
            self.menu &= !items.is_empty();
            if self.menu {
                self.draw_menu(c, th, area, &items);
            }
            return;
        };
        self.menu = false;
        // The keyboard is up (`softkey_bar`: `area` is the whole screen),
        // on its first page for an input it was not up for before.
        if self.kb_for != Some(input) {
            self.kb = Keyboard::new(mode);
            self.kb_for = Some(input);
        }
        self.kb.set_mode(mode);
        let next = appview_impl::next_input(&self.view.batch_quiet()).is_some();
        self.kb.set_action(if next { tr::NEXT } else { tr::DONE });
        let (_, keys) = area.split_bottom(Keyboard::height(th));
        // The title bar stays where it is, and the view is drawn under it.
        let bar = (!full).then_some(IRect { h: th.title_h, ..area });
        let below = match bar {
            Some(bar) => IRect { y: bar.bottom(), h: keys.y - bar.bottom(), ..area },
            None => area,
        };
        let render = |tree: &mut crate::appview::Tree, c: &mut Canvas, at: IRect| {
            if full {
                appview_impl::render(tree, c, th, at, cx.now_ms)
            } else {
                appview_impl::render_titled(tree, c, th, at, cx.now_ms)
            }
        };
        let old = c.push_clip(below);
        self.area = self.view_area(th, area, keys, input);
        render(&mut self.view.batch_quiet(), c, self.area);
        // A `Scroll` that brought the input into itself as it was drawn
        // moved the input: once more, where the view is to be now.
        let moved = self.view_area(th, area, keys, input);
        if moved != self.area {
            self.area = moved;
            render(&mut self.view.batch_quiet(), c, self.area);
        }
        c.restore_clip(old);
        // (The region is the view's, as far as the screen goes: the title
        // bar and the keyboard are over it.  What looks pressed is within
        // what shows of the view.)
        c.hit_quiet(self.area, Hit::Custom(VIEW));
        let old = c.push_clip(below);
        self.press(c, &self.view.batch_quiet());
        c.restore_clip(old);
        // The back button, where it always is, puts the keyboard away.
        let back = IRect::new(area.x, area.y, th.back_w, th.title_h);
        match bar {
            Some(bar) => {
                // The top of the view, as it is without the keyboard.
                let home = self.view_home(th, area);
                let old = c.push_clip(bar);
                render(&mut self.view.batch_quiet(), c, home);
                c.restore_clip(old);
                // (A touch on the title is not one on the view under it.)
                c.hit(bar, Hit::Nothing);
                widgets::title_buttons(c, th, bar, th.title_fg);
            }
            // (Over a view drawn farther up the arrow is not over the
            // corner the app keeps free: on its own button, as always in
            // a full-screen view.)
            None => back_disc(c, th, back),
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
