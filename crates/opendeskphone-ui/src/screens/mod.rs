//! Native screens and the generic building blocks they share (menu, text
//! entry, information list, confirmation, level adjuster).

use crate::appview::{Font, InputMode};
use crate::gfx::{Canvas, HAlign, IRect};
use crate::multitap::MultiTap;
use crate::skin::Theme;
use crate::strings as tr;
use crate::touch::Hit;
use crate::widgets::{self, Edit, ListState};
use crate::UiConfig;
use opendeskphone_hal::Lamp;
use opendeskphone_state::{Command, Key, LampColor, Model, State, Store};

pub(crate) mod alarm;
pub(crate) mod app;
pub(crate) mod bluetooth;
pub(crate) mod boot;
pub(crate) mod calls;
pub(crate) mod diag;
pub(crate) mod dialpad;
pub(crate) mod idle;
pub(crate) mod menus;
pub(crate) mod side;

/// What every screen sees.
pub(crate) struct Cx<'a> {
    pub store: &'a Store,
    pub st: &'a State,
    pub th: &'static Theme,
    pub model: Model,
    pub cfg: &'a UiConfig,
    pub now_ms: u64,
    pub unix: u64,
    pub off_hook: bool,
    /// The app view shown on the idle screen, if any.
    pub idle_view: Option<&'a crate::appview::AppView>,
}

impl Cx<'_> {
    /// Sends a command without waiting; failures are logged.
    pub fn post(&self, cmd: Command) {
        let what = format!("{cmd:?}");
        if !self.store.post(cmd) {
            log::warn!("no handler for {what}");
        }
    }

    pub fn civil(&self) -> crate::time::Civil {
        crate::time::Civil::from_unix(self.unix, self.cfg.utc_offset_secs)
    }
}

/// Navigation result of a key or softkey.
pub(crate) enum Nav {
    /// Not handled: the global key handler gets it.
    Ignored,
    /// Handled; stay.
    Stay,
    Push(Box<dyn Screen>),
    Pop,
    /// Back to the base screen.
    Home,
}

/// Kinds of screen the navigator treats specially.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    Plain,
    Call,
    /// A dialer; `off_hook` ones close when the handset goes down.
    Dialer,
    App,
    /// The ringing alarm.
    Alarm,
}

pub(crate) type Soft = (u32, String);

pub(crate) fn soft(id: u32, label: &str) -> Soft {
    (id, label.to_string())
}

/// A native screen.
pub(crate) trait Screen: Send {
    /// Name reported in the `Ui` state (`idle`, `dialer`, ...).
    fn name(&self) -> String;
    fn tag(&self) -> Tag {
        Tag::Plain
    }
    /// The softkey actions (any number; the navigator pages beyond four).
    fn softkeys(&self, _cx: &Cx) -> Vec<Soft> {
        Vec::new()
    }
    fn softkey(&mut self, _cx: &Cx, _id: u32) -> Nav {
        Nav::Ignored
    }
    /// Screens that lay out their own four softkeys (apps) return them
    /// here and receive `Key::Soft` through [`Screen::key`].
    fn raw_softkeys(&self, _cx: &Cx) -> Option<[String; 4]> {
        None
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav;
    /// Touch: a finger tapped a region this screen registered as it drew
    /// ([`crate::gfx::Canvas::hit`]) with [`Hit::Custom`] or [`Hit::Row`],
    /// at pixel (`x`, `y`) of the screen.  (The other kinds of region
    /// never come here: a [`Hit::Key`] comes as the key, a [`Hit::Soft`]
    /// as the softkey action.)  A row this returns [`Nav::Ignored`] for is
    /// selected in the screen's [`Screen::list_state`], if it has one, and
    /// then [`Key::Select`] is pressed.
    fn tap(&mut self, _cx: &Cx, _hit: Hit, _x: i32, _y: i32) -> Nav {
        Nav::Ignored
    }
    /// Touch: a finger came down on a [`Hit::Custom`] or [`Hit::Row`]
    /// region, at pixel (`x`, `y`) of the screen.  What the touch becomes
    /// (a tap, a long press, or a drag) comes afterward, as ever; this
    /// is for what cannot wait for the finger to be lifted.
    fn touch_down(&mut self, _cx: &Cx, _hit: Hit, _x: i32, _y: i32) {}
    /// Touch: a finger stayed on a [`Hit::Custom`] or [`Hit::Row`] region.
    /// [`Nav::Ignored`] makes it a tap.
    fn long_press(&mut self, _cx: &Cx, _hit: Hit, _x: i32, _y: i32) -> Nav {
        Nav::Ignored
    }
    /// Touch: a finger that went down on `from` (a region of this
    /// screen's, [`Hit::Custom`] or [`Hit::Row`], or none) moved `dx` and
    /// `dy` pixels (up is negative).  Called for every movement of the
    /// drag.  When this returns [`Nav::Ignored`], the screen's
    /// [`Screen::list_state`], if it has one, is scrolled by rows of the
    /// skin's height.
    fn drag(&mut self, _cx: &Cx, _from: Option<Hit>, _dx: i32, _dy: i32) -> Nav {
        Nav::Ignored
    }
    /// Touch: [`Screen::drag`] with the place the finger went down, pixel
    /// (`x`, `y`) of the screen, for a screen that has to know where in a
    /// region a drag began.  The UI calls this one.
    fn drag_at(&mut self, cx: &Cx, from: Option<Hit>, _x: i32, _y: i32, dx: i32, dy: i32) -> Nav {
        self.drag(cx, from, dx, dy)
    }
    /// Touch: the finger of a drag ([`Screen::drag`]) was lifted, or the
    /// touch was canceled.
    fn drag_end(&mut self, _cx: &Cx, _from: Option<Hit>) -> Nav {
        Nav::Ignored
    }
    /// Touch: the one list of a screen that draws it with
    /// [`widgets::list`] in rows of the skin's height (`row_h`), and the
    /// number of its rows.  With it, and nothing else, a tap on a row
    /// selects the row and presses [`Key::Select`], and a drag scrolls
    /// the list.
    fn list_state(&mut self, _cx: &Cx) -> Option<(&mut ListState, usize)> {
        None
    }
    /// The handset was lifted or replaced.
    fn hook(&mut self, _cx: &Cx, _off_hook: bool) -> Nav {
        Nav::Ignored
    }
    /// Paints the area above the softkey bar.
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect);
    /// Monotonic time at which the screen needs a repaint or a
    /// [`Screen::tick`] without any input (clocks, timers).
    fn deadline(&self, _cx: &Cx) -> Option<u64> {
        None
    }
    /// Called when the deadline passed.
    fn tick(&mut self, _cx: &Cx) -> Nav {
        Nav::Stay
    }
    /// The screen is being removed or covered.
    fn leave(&mut self, _cx: &Cx) {}
    /// For `Tag::App` screens: the app's name.
    fn app_name(&self) -> Option<&str> {
        None
    }
    /// True when the app view changed since the last draw.
    fn dirty(&self) -> bool {
        false
    }
    /// The screen became the top screen.
    fn enter(&mut self, _cx: &Cx) {}
    /// Digits typed so far, for the `Ui` state (dialers).
    fn dial_buffer(&self) -> Option<&str> {
        None
    }
    /// Paint the wallpaper behind this screen (idle and call screens)
    /// instead of the plain background.
    fn wallpaper(&self) -> bool {
        false
    }
    /// The wallpaper behind this screen moves, if it is one that does
    /// (the idle screen, and the theme selector, which shows it).
    fn motion(&self) -> bool {
        false
    }
    /// A built-in wallpaper whose picture and palette this screen is
    /// shown with, instead of the configured ones (color skin only).
    fn look(&self, _cx: &Cx) -> Option<&'static crate::wallpaper::Wallpaper> {
        None
    }
    /// The palette changed (color skin): `th` is the theme from now on.
    fn theme_changed(&mut self, _th: &'static Theme) {}
    /// Whether the softkey shapes are drawn below this screen.  A screen
    /// on which no key does anything returns false and gets the whole
    /// frame to draw in.
    fn softkey_bar(&self, _cx: &Cx) -> bool {
        true
    }
    /// Paints over the softkey bar too.
    fn full_screen(&self) -> bool {
        false
    }
    /// Receives every key, including the ones the phone normally keeps.
    fn captures_all_keys(&self) -> bool {
        false
    }
    /// Lamps that show these colors, in place of the phone's, while the
    /// screen is the top screen.
    fn lamps(&self) -> Vec<(Lamp, LampColor)> {
        Vec::new()
    }
}

// ------------------------------------------------------------------ menu

pub(crate) type Action = Box<dyn Fn(&Cx) -> Nav + Send>;

pub(crate) struct Item {
    pub icon: Option<&'static str>,
    pub label: String,
    pub value: String,
    pub act: Action,
    /// Touch: the item is a setting that is on or off, and shows it with
    /// a switch at its right ([`Item::switch`]).
    pub switch: Option<bool>,
    /// The item destroys something ([`Item::destructive`]).
    pub destructive: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, act: impl Fn(&Cx) -> Nav + Send + 'static) -> Item {
        Item { icon: None, label: label.into(), value: String::new(), act: Box::new(act), switch: None, destructive: false }
    }
    /// A setting that is on or off.  On the touch skin the item has a
    /// switch at its right ([`widgets::switch`]) in place of its value,
    /// and a touch on the item does what the item does (which
    /// should be to switch the setting).  The label names the setting,
    /// not the state.  On the other skins the item is as it was: give it
    /// a value that says the state there.
    pub fn switch(mut self, on: bool) -> Item {
        self.switch = Some(on);
        self
    }
    /// An item that erases or deletes something: on the color and touch
    /// skins its label is in the error color.
    pub fn destructive(mut self) -> Item {
        self.destructive = true;
        self
    }
    pub fn value(mut self, v: impl Into<String>) -> Item {
        self.value = v.into();
        self
    }
    pub fn icon(mut self, name: &'static str) -> Item {
        self.icon = Some(name);
        self
    }
}

/// A titled list of items with numeric shortcuts (stock menu helper).  The
/// items are rebuilt from live state on every draw.
pub(crate) struct Menu {
    name: &'static str,
    title: String,
    build: Box<dyn Fn(&Cx) -> Vec<Item> + Send>,
    st: ListState,
    numbered: bool,
}

impl Menu {
    pub fn new(name: &'static str, title: impl Into<String>, build: impl Fn(&Cx) -> Vec<Item> + Send + 'static) -> Menu {
        Menu { name, title: title.into(), build: Box::new(build), st: ListState::default(), numbered: true }
    }
    pub fn boxed(name: &'static str, title: impl Into<String>, build: impl Fn(&Cx) -> Vec<Item> + Send + 'static) -> Box<dyn Screen> {
        Box::new(Self::new(name, title, build))
    }
    fn activate(&mut self, cx: &Cx, idx: usize) -> Nav {
        let items = (self.build)(cx);
        match items.get(idx) {
            Some(it) => {
                self.st.selected = idx;
                (it.act)(cx)
            }
            None => Nav::Stay,
        }
    }
}

impl Screen for Menu {
    fn name(&self) -> String {
        self.name.into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: an item is touched, and the title bar has the way back.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::SELECT), soft(0, ""), soft(0, ""), soft(4, tr::BACK)]
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 => self.activate(cx, self.st.selected),
            4 => Nav::Pop,
            _ => Nav::Stay,
        }
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, _x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Row { row, .. } => self.activate(cx, row as usize),
            _ => Nav::Ignored,
        }
    }
    fn drag(&mut self, cx: &Cx, _from: Option<Hit>, _dx: i32, dy: i32) -> Nav {
        let len = (self.build)(cx).len();
        self.st.drag(dy, cx.th.row_h, len);
        Nav::Stay
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        let len = (self.build)(cx).len();
        match key {
            Key::Up => {
                self.st.step(-1, len, true);
                Nav::Stay
            }
            Key::Down => {
                self.st.step(1, len, true);
                Nav::Stay
            }
            Key::Select | Key::Right => self.activate(cx, self.st.selected),
            Key::Cancel | Key::Left => Nav::Pop,
            Key::Digit(d) if self.numbered => {
                let idx = if d == 0 { 9 } else { d as usize - 1 };
                if idx < len { self.activate(cx, idx) } else { Nav::Stay }
            }
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, &self.title);
        let items = (self.build)(cx);
        // Touch: no row is the selected one (there is no cursor), and the
        // rows have no numbers (there are no digit keys): an icon, or the
        // title's inset.
        let numbered = self.numbered && !th.touch();
        let num_w = if numbered {
            if th.gray { 15 } else { area.w / 10 }
        } else if th.touch() && items.iter().any(|it| it.icon.is_some()) {
            th.back_w
        } else if th.touch() {
            th.title_pad
        } else {
            th.pad
        };
        let f = th.font(Font::Normal);
        widgets::list(c, th, widgets::body_rows(th, body), items.len(), &mut self.st, th.row_h, !th.touch(), |c, r, i, sel, fg| {
            let it = &items[i];
            let (num, rest) = r.split_left(num_w);
            if let Some(icon) = it.icon.and_then(|n| th.icon(n)) {
                c.icon_in(icon, num, fg);
            } else if numbered && i < 10 {
                let n = if i == 9 { 0 } else { i + 1 };
                c.text_in(f, num, &n.to_string(), if sel { fg } else { th.secondary }, HAlign::Center);
            }
            let mut rest = IRect { w: rest.w - th.pad, ..rest };
            // Touch: the switch says the state, in place of the value.
            let switched = it.switch.filter(|_| th.touch());
            if let Some(on) = switched {
                let sw = widgets::switch(c, th, IRect { w: rest.w - th.pad, ..rest }, on);
                rest.w = sw.x - th.pad - rest.x;
            }
            let label_fg = if it.destructive && !th.gray && !sel { th.error } else { fg };
            if it.value.is_empty() || switched.is_some() {
                c.text_in(f, rest, &it.label, label_fg, HAlign::Left);
            } else {
                widgets::key_value(c, th, rest, &it.label, &it.value, label_fg, if sel { fg } else { th.secondary });
            }
        });
    }
}

// ------------------------------------------------------------ text entry

/// A one-field form.
///
/// Touch: the on-screen keyboard ([`crate::osk`]) at the bottom of the
/// screen, and the field right above it.  The keyboard's action key is
/// the one control that takes the text, labeled with what it does on
/// this screen ([`EditScreen::action`]; "Done" otherwise); the title
/// bar's back button is the way out without it.  A tap in the field puts
/// the cursor there.
pub(crate) struct EditScreen {
    name: &'static str,
    title: String,
    text: String,
    cursor: u16,
    mode: InputMode,
    max_len: usize,
    tap: MultiTap,
    done: Box<dyn Fn(&Cx, &str) -> Nav + Send>,
    /// Touch: the keyboard.
    kb: crate::osk::Keyboard,
    /// Touch: where the field was drawn.
    field: IRect,
}

/// Touch: the region of [`EditScreen`]'s field.
const EDIT_FIELD: u32 = 1;

impl EditScreen {
    pub fn boxed(
        name: &'static str,
        title: impl Into<String>,
        initial: &str,
        mode: InputMode,
        done: impl Fn(&Cx, &str) -> Nav + Send + 'static,
    ) -> Box<EditScreen> {
        Box::new(EditScreen {
            name,
            title: title.into(),
            text: initial.to_string(),
            cursor: initial.chars().count() as u16,
            mode,
            max_len: 127,
            tap: MultiTap::default(),
            done: Box::new(done),
            kb: crate::osk::Keyboard::new(mode),
            field: IRect::default(),
        })
    }

    /// Touch: what the keyboard's action key says: what taking the text
    /// does on this screen, in a word ("Next", "Connect").
    pub fn action(mut self: Box<Self>, label: &'static str) -> Box<EditScreen> {
        self.kb.set_action(label);
        self
    }

    /// Touch: the keyboard, and the field right above it.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, body: IRect) {
        let th = cx.th;
        let (top, keys) = body.split_bottom(crate::osk::Keyboard::height(th));
        let inset = th.pad * 2;
        self.field = IRect::new(top.x + inset, top.bottom() - inset - th.touch_min, top.w - 2 * inset, th.touch_min);
        let pending = self.tap.pending(cx.now_ms);
        widgets::text_field(c, th, self.field, &self.text, self.cursor as usize, self.mode, true, pending, "", Font::Normal);
        // (A touch in the field moves the cursor: nothing is pressed.)
        c.hit_quiet(self.field, Hit::Custom(EDIT_FIELD));
        self.kb.draw(c, th, keys);
    }
}

impl Screen for EditScreen {
    fn name(&self) -> String {
        self.name.into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the keyboard has the key that takes the text and the one
        // that deletes, and the title bar the way out.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::OK), soft(0, ""), soft(3, tr::DELETE), soft(4, tr::CANCEL)]
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, x: i32, _y: i32) -> Nav {
        if hit == Hit::Custom(EDIT_FIELD) {
            let pending = self.tap.pending(cx.now_ms);
            self.tap.reset();
            let at = widgets::text_field_cursor_at(cx.th, self.field, &self.text, self.cursor as usize, self.mode, pending, Font::Normal, x);
            self.cursor = at as u16;
            return Nav::Stay;
        }
        match self.kb.edit(hit, &mut self.text, &mut self.cursor, self.max_len) {
            Some(crate::osk::Press::Done) => (self.done)(cx, &self.text),
            Some(_) => {
                self.tap.reset();
                Nav::Stay
            }
            None => Nav::Ignored,
        }
    }
    fn drag(&mut self, _cx: &Cx, _from: Option<Hit>, _dx: i32, _dy: i32) -> Nav {
        // A finger that slides on the keyboard types nothing and moves
        // nothing.
        Nav::Stay
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 => (self.done)(cx, &self.text),
            3 => {
                widgets::edit_backspace(&mut self.text, &mut self.cursor, &mut self.tap);
                Nav::Stay
            }
            4 => Nav::Pop,
            _ => Nav::Stay,
        }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match key {
            Key::Select => (self.done)(cx, &self.text),
            Key::Cancel => Nav::Pop,
            _ => match widgets::edit_key(&mut self.text, &mut self.cursor, self.mode, self.max_len, &mut self.tap, key, cx.now_ms) {
                Edit::Ignored => Nav::Ignored,
                _ => Nav::Stay,
            },
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, &self.title);
        if th.touch() {
            return self.draw_touch(cx, c, body);
        }
        let h = if th.gray { th.line_h + 4 } else { th.px(40) };
        let r = IRect::new(body.x + th.pad * 2, body.y + (body.h - h) / 2, body.w - th.pad * 4, h);
        widgets::text_field(c, th, r, &self.text, self.cursor as usize, self.mode, true, self.tap.pending(cx.now_ms), "", Font::Normal);
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        self.tap.deadline_ms().filter(|d| *d > cx.now_ms)
    }
}

// ------------------------------------------------------------------ info

/// A scrolling list of `label: value` rows (status pages).
pub(crate) struct Info {
    name: &'static str,
    title: String,
    build: Box<dyn Fn(&Cx) -> Vec<(String, String)> + Send>,
    top: usize,
    /// Touch: how far a finger has dragged the rows, short of a row.
    drag: i32,
}

impl Info {
    pub fn boxed(name: &'static str, title: impl Into<String>, build: impl Fn(&Cx) -> Vec<(String, String)> + Send + 'static) -> Box<dyn Screen> {
        Box::new(Info { name, title: title.into(), build: Box::new(build), top: 0, drag: 0 })
    }
}

impl Info {
    /// The height of a row: these rows are read, not touched, so the
    /// touch skin's are lower than its lists'.
    fn row_h(th: &Theme) -> i32 {
        if th.gray {
            th.line_h
        } else if th.touch() {
            th.row_h * 3 / 4
        } else {
            th.row_h
        }
    }
}

impl Screen for Info {
    fn name(&self) -> String {
        self.name.into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the title bar has the way back.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(0, ""), soft(0, ""), soft(0, ""), soft(4, tr::BACK)]
    }
    fn softkey(&mut self, _cx: &Cx, _id: u32) -> Nav {
        Nav::Pop
    }
    fn drag(&mut self, cx: &Cx, _from: Option<Hit>, _dx: i32, dy: i32) -> Nav {
        let row_h = Self::row_h(cx.th);
        self.drag -= dy;
        let rows = self.drag / row_h;
        self.drag -= rows * row_h;
        // (Drawing keeps it within the rows there are.)
        self.top = (self.top as i32 + rows).max(0) as usize;
        Nav::Stay
    }
    fn key(&mut self, _cx: &Cx, key: Key) -> Nav {
        match key {
            Key::Up => {
                self.top = self.top.saturating_sub(1);
                Nav::Stay
            }
            Key::Down => {
                self.top += 1;
                Nav::Stay
            }
            Key::Cancel | Key::Left | Key::Select => Nav::Pop,
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, &self.title);
        let rows = (self.build)(cx);
        // Stock (color): 32-pixel rows from y 42 with a hairline under
        // each, text inset 15 pixels.
        let (row_h, top, inset) = if th.gray { (th.line_h, 0, th.pad + 2) } else { (Self::row_h(th), 2, th.title_pad) };
        let visible = ((body.h - top) / row_h).max(1) as usize;
        self.top = self.top.min(rows.len().saturating_sub(visible));
        let bar = rows.len() > visible;
        let w = body.w - if bar { th.scroll_w } else { 0 } - 2 * inset;
        let f = th.font(Font::Normal);
        for (i, (k, v)) in rows.iter().skip(self.top).take(visible).enumerate() {
            let r = IRect::new(body.x + inset, body.y + top + i as i32 * row_h, w, row_h - if th.gray { 0 } else { 1 });
            if v.is_empty() {
                c.text_in(th.font(Font::Bold), r, k, th.fg, HAlign::Left);
            } else if k.is_empty() {
                c.text_in(f, r, v, th.fg, HAlign::Left);
            } else {
                widgets::key_value(c, th, r, k, v, th.fg, th.fg);
            }
            if !th.gray {
                c.hline(r.x, r.bottom(), r.w + th.px(4), th.border);
            }
        }
        widgets::scroll_bar(c, th, body, rows.len() as i32, visible as i32, self.top as i32);
    }
    fn deadline(&self, cx: &Cx) -> Option<u64> {
        Some(cx.now_ms + 1000)
    }
}

// --------------------------------------------------------------- confirm

/// A question before something is done.
///
/// With physical buttons: the question, and the softkeys "Yes" and "No".
///
/// Touch: a card in the middle of the screen with a title (if it has
/// one), the question (best a sentence that says what will happen), and
/// two buttons side by side: Cancel at the left, and at the right the
/// action, labeled with what it does ([`Confirm::action`]) and red when it
/// erases or deletes something ([`Confirm::destructive`]).
pub(crate) struct Confirm {
    title: String,
    text: String,
    action: String,
    destructive: bool,
    yes: Box<dyn Fn(&Cx) -> Nav + Send>,
}

impl Confirm {
    /// A question with no title, whose action is labeled "OK" on the
    /// touch skin.
    pub fn boxed(text: impl Into<String>, yes: impl Fn(&Cx) -> Nav + Send + 'static) -> Box<Confirm> {
        Box::new(Confirm { title: String::new(), text: text.into(), action: tr::OK.into(), destructive: false, yes: Box::new(yes) })
    }
    /// Touch: the title of the card ("Factory reset").
    pub fn title(mut self: Box<Self>, title: impl Into<String>) -> Box<Confirm> {
        self.title = title.into();
        self
    }
    /// Touch: the label of the button that does it: a verb, or a verb and
    /// its object ("Erase and restart", "Clear call log").
    pub fn action(mut self: Box<Self>, label: impl Into<String>) -> Box<Confirm> {
        self.action = label.into();
        self
    }
    /// Touch: the action erases or deletes something: its button is red.
    pub fn destructive(mut self: Box<Self>) -> Box<Confirm> {
        self.destructive = true;
        self
    }

    /// Touch: the card.
    fn draw_touch(&self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        let f = th.font(Font::Normal);
        let pad = th.pad * 2;
        let w = area.w - 2 * th.px(20);
        let inner_w = w - 2 * pad;
        let title_h = if self.title.is_empty() { 0 } else { th.line_h + th.pad };
        let lines = f.wrap(&self.text, inner_w).len().max(1) as i32;
        let text_h = lines * th.line_h;
        let button_h = th.row_h;
        let h = pad + title_h + text_h + pad + button_h + pad;
        let card = IRect::new(area.x + (area.w - w) / 2, area.y + ((area.h - h) / 2).max(0), w, h);
        c.fill_round_aa(card.inset(-th.scale), th.px(9), th.sk_edge);
        c.fill_round_aa(card, th.px(8), th.modal_bg);
        let inner = card.inset(pad);
        let mut y = inner.y;
        if !self.title.is_empty() {
            c.text_in(th.font(Font::Title), IRect::new(inner.x, y, inner.w, th.line_h), &self.title, th.fg, HAlign::Left);
            y += title_h;
        }
        c.text_wrapped(f, IRect::new(inner.x, y, inner.w, text_h), &self.text, th.fg, HAlign::Left, th.line_h);
        // Cancel at the left, the action at the right, and room between
        // them for a finger to miss.  (The action's region is the first,
        // as its softkey is on the other skins.)
        let gap = th.px(12);
        let bw = (inner.w - gap) / 2;
        let row = IRect::new(inner.x, inner.bottom() - button_h, bw, button_h);
        let style = if self.destructive { widgets::ButtonStyle::Destructive } else { widgets::ButtonStyle::Primary };
        widgets::button_styled(c, th, IRect { x: inner.right() - bw, ..row }, &self.action, style, Hit::Soft(1));
        widgets::button_styled(c, th, row, tr::CANCEL, widgets::ButtonStyle::Secondary, Hit::Soft(4));
    }
}

impl Screen for Confirm {
    fn name(&self) -> String {
        "confirm".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the buttons are in the card.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, tr::YES), soft(0, ""), soft(0, ""), soft(4, tr::NO)]
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        if id == 1 { (self.yes)(cx) } else { Nav::Pop }
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match key {
            Key::Select => (self.yes)(cx),
            Key::Cancel => Nav::Pop,
            _ => Nav::Ignored,
        }
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        if th.touch() {
            return self.draw_touch(cx, c, area);
        }
        let f = th.font(Font::Normal);
        let inner = area.inset(th.pad * 2);
        let lines = f.wrap(&self.text, inner.w).len() as i32;
        let y = inner.y + ((inner.h - lines * th.line_h) / 2).max(0);
        c.text_wrapped(f, IRect::new(inner.x, y, inner.w, inner.bottom() - y), &self.text, th.fg, HAlign::Center, th.line_h);
    }
}

// ----------------------------------------------------------------- level

/// A level adjuster (brightness, contrast, volume) with Left/Right.
///
/// Touch: the level in large digits over a slider, with a minus button at
/// its left and a plus button at its right.  A tap on the slider sets the
/// level there; a finger dragged along it moves the level with it.
pub(crate) struct Level {
    title: String,
    max: u8,
    get: Box<dyn Fn(&Cx) -> u8 + Send>,
    set: Box<dyn Fn(&Cx, u8) + Send>,
    commit: Option<Box<dyn Fn(&Cx, u8) + Send>>,
    /// The level being shown, ahead of the state catching up.
    shown: Option<u8>,
    /// Touch: where the slider was drawn (from no level at its left end
    /// to the most at its right).
    track: IRect,
    /// Touch: how far a finger has dragged the slider, short of a step.
    drag: i32,
}

/// Touch: the regions of [`Level`].
const LEVEL_MINUS: u32 = 1;
const LEVEL_PLUS: u32 = 2;
const LEVEL_TRACK: u32 = 3;

impl Level {
    pub fn boxed(
        title: impl Into<String>,
        max: u8,
        get: impl Fn(&Cx) -> u8 + Send + 'static,
        set: impl Fn(&Cx, u8) + Send + 'static,
    ) -> Box<Level> {
        Box::new(Level {
            title: title.into(),
            max,
            get: Box::new(get),
            set: Box::new(set),
            commit: None,
            shown: None,
            track: IRect::default(),
            drag: 0,
        })
    }
    /// Runs once when the screen closes, with the final level (persist it).
    pub fn with_commit(mut self: Box<Self>, f: impl Fn(&Cx, u8) + Send + 'static) -> Box<Level> {
        self.commit = Some(Box::new(f));
        self
    }
    fn adjust(&mut self, cx: &Cx, d: i32) {
        let cur = self.shown.unwrap_or_else(|| (self.get)(cx)) as i32;
        let new = (cur + d).clamp(0, self.max as i32) as u8;
        self.shown = Some(new);
        (self.set)(cx, new);
    }

    /// Touch: the level at `x` along the slider.
    fn level_at(&self, x: i32) -> i32 {
        let w = self.track.w.max(1);
        ((x - self.track.x).clamp(0, w) * self.max as i32 + w / 2) / w
    }

    /// Touch: the level, the slider, and its buttons, in `body`.
    fn draw_touch(&mut self, cx: &Cx, c: &mut Canvas, body: IRect, level: u8) {
        let th = cx.th;
        let b = th.touch_min;
        let row = IRect::new(body.x + th.pad * 2, body.y + (body.h - b) / 2, body.w - th.pad * 4, b);
        let (minus, rest) = row.split_left(b);
        let (rest, plus) = rest.split_right(b);
        widgets::button_styled(c, th, minus, "\u{2013}", widgets::ButtonStyle::Secondary, Hit::Custom(LEVEL_MINUS));
        widgets::button_styled(c, th, plus, "+", widgets::ButtonStyle::Secondary, Hit::Custom(LEVEL_PLUS));
        // The slider: a bar with round ends, filled to the level, and a
        // round knob at the level; the knob's middle goes from one end of
        // the track to the other.
        let knob = th.px(22);
        let slider = rest.inset_xy(th.pad * 2, 0);
        self.track = slider.inset_xy(knob / 2, 0);
        let bar_h = th.px(6);
        let bar = IRect::new(slider.x, slider.y + (slider.h - bar_h) / 2, slider.w, bar_h);
        let at = self.track.x + self.track.w * level as i32 / self.max.max(1) as i32;
        c.fill_round_aa(bar, bar_h / 2, th.border);
        c.fill_round_aa(IRect { w: at - bar.x, ..bar }, bar_h / 2, th.sel_bg);
        let k = IRect::new(at - knob / 2, slider.y + (slider.h - knob) / 2, knob, knob);
        c.fill_round_aa(k, knob / 2, th.accent);
        // (The slider follows the finger: that is what it shows of it.)
        c.hit_quiet(rest, Hit::Custom(LEVEL_TRACK));
        // The level, large, above it.
        let text = IRect::new(body.x, row.y - th.pad * 2 - th.px(40), body.w, th.px(40));
        c.text_in(th.font(Font::Large), text, &level.to_string(), th.fg, HAlign::Center);
        c.text_in(
            th.font(Font::Normal),
            IRect::new(body.x, row.bottom() + th.pad, body.w, th.line_h),
            &format!("0 \u{2013} {}", self.max),
            th.secondary,
            HAlign::Center,
        );
    }
}

impl Screen for Level {
    fn name(&self) -> String {
        "level".into()
    }
    fn softkeys(&self, cx: &Cx) -> Vec<Soft> {
        // Touch: the buttons are by the slider, and the title bar has the
        // way back.
        if cx.th.touch() {
            return Vec::new();
        }
        vec![soft(1, "\u{2013}"), soft(2, "+"), soft(0, ""), soft(4, tr::BACK)]
    }
    fn tap(&mut self, cx: &Cx, hit: Hit, x: i32, _y: i32) -> Nav {
        match hit {
            Hit::Custom(LEVEL_MINUS) => self.adjust(cx, -1),
            Hit::Custom(LEVEL_PLUS) => self.adjust(cx, 1),
            Hit::Custom(LEVEL_TRACK) => {
                let cur = self.shown.unwrap_or_else(|| (self.get)(cx)) as i32;
                self.adjust(cx, self.level_at(x) - cur);
            }
            _ => return Nav::Ignored,
        }
        Nav::Stay
    }
    fn drag(&mut self, cx: &Cx, from: Option<Hit>, dx: i32, _dy: i32) -> Nav {
        if from != Some(Hit::Custom(LEVEL_TRACK)) {
            return Nav::Stay;
        }
        // A step for each part of the track a level takes.
        let step = (self.track.w / self.max.max(1) as i32).max(1);
        self.drag += dx;
        let steps = self.drag / step;
        self.drag -= steps * step;
        if steps != 0 {
            self.adjust(cx, steps);
        }
        Nav::Stay
    }
    fn softkey(&mut self, cx: &Cx, id: u32) -> Nav {
        match id {
            1 => self.adjust(cx, -1),
            2 => self.adjust(cx, 1),
            _ => return Nav::Pop,
        }
        Nav::Stay
    }
    fn key(&mut self, cx: &Cx, key: Key) -> Nav {
        match key {
            Key::Left | Key::Down => self.adjust(cx, -1),
            Key::Right | Key::Up => self.adjust(cx, 1),
            Key::Cancel | Key::Select => return Nav::Pop,
            _ => return Nav::Ignored,
        }
        Nav::Stay
    }
    fn draw(&mut self, cx: &Cx, c: &mut Canvas, area: IRect) {
        let th = cx.th;
        c.fill(area, th.bg);
        let (title, body) = area.split_top(th.title_h);
        widgets::title_bar(c, th, title, &self.title);
        let level = self.shown.unwrap_or_else(|| (self.get)(cx));
        if th.touch() {
            return self.draw_touch(cx, c, body, level);
        }
        let h = if th.gray { 10 } else { th.px(22) };
        let r = IRect::new(body.x + th.pad * 3, body.y + (body.h - h) / 2, body.w - th.pad * 6, h);
        widgets::progress(c, th, r, level as u32 * 100 / self.max.max(1) as u32);
        c.text_in(
            th.font(Font::Normal),
            IRect::new(body.x, r.bottom() + 2, body.w, th.line_h),
            &format!("{level} / {}", self.max),
            th.secondary,
            HAlign::Center,
        );
    }
    fn leave(&mut self, cx: &Cx) {
        if let (Some(f), Some(v)) = (&self.commit, self.shown.take()) {
            f(cx, v);
        }
    }
}

/// A full-area centered message with an optional progress bar
/// (provisioning progress, update).
pub(crate) fn splash(c: &mut Canvas, th: &Theme, area: IRect, title: &str, text: &str, percent: Option<u32>) {
    let mut y = area.y + (area.h - 3 * th.line_h) / 2;
    if !title.is_empty() {
        c.text_in(th.font(Font::Title), IRect::new(area.x + th.pad, y, area.w - 2 * th.pad, th.line_h), title, th.fg, HAlign::Center);
        y += th.line_h;
    }
    let used = c.text_wrapped(
        th.font(Font::Normal),
        IRect::new(area.x + th.pad, y, area.w - 2 * th.pad, (area.bottom() - y).max(th.line_h)),
        text,
        if th.gray { th.fg } else { th.secondary },
        HAlign::Center,
        th.line_h,
    );
    y += used + 2;
    if let Some(p) = percent {
        let h = if th.gray { 8 } else { th.px(14) };
        if y + h <= area.bottom() {
            widgets::progress(c, th, IRect::new(area.x + area.w / 6, y, area.w * 2 / 3, h), p);
        }
    }
}

#[cfg(all(test, feature = "touch"))]
mod tests {
    use super::*;
    use crate::osk::{Cap, Keyboard};
    use std::sync::{Arc, Mutex};

    /// Runs `f` with what a screen sees on a D80, and a canvas that says
    /// there is a screen to go back to.
    fn with_cx(f: impl FnOnce(&Cx, &mut Canvas, IRect)) {
        let store = Store::new(Model::D80);
        let st = store.snapshot();
        let cfg = UiConfig::default();
        let th = Theme::for_model(Model::D80);
        let cx = Cx { store: &store, st: &st, th, model: Model::D80, cfg: &cfg, now_ms: 0, unix: 0, off_hook: false, idle_view: None };
        let mut c = Canvas::new(th.width as u16, th.height as u16, false);
        c.set_nav(crate::gfx::NavCx { back: true });
        let area = c.bounds();
        f(&cx, &mut c, area);
    }

    fn redraw(s: &mut dyn Screen, cx: &Cx, c: &mut Canvas, area: IRect) {
        c.clear_hits();
        s.draw(cx, c, area);
    }

    #[test]
    fn text_entry_has_one_control_that_takes_the_text() {
        with_cx(|cx, c, area| {
            let th = cx.th;
            let got = Arc::new(Mutex::new(None));
            let sink = got.clone();
            let mut s = EditScreen::boxed("edit", "Server address", "ab", InputMode::Text, move |_, text| {
                *sink.lock().unwrap() = Some(text.to_string());
                Nav::Pop
            })
            .action("Connect");
            // No softkeys, and no buttons of its own: the keyboard's
            // action key is the one control, and says what it does.
            assert!(s.softkeys(cx).is_empty());
            redraw(&mut *s, cx, c, area);
            assert!(c.hits().iter().all(|(_, h)| !matches!(h, Hit::Soft(_))));
            assert_eq!(s.kb.action(), "Connect");
            // The field is right above the keyboard: no farther from it
            // than from the sides of the screen.
            let keys_y = area.bottom() - Keyboard::height(th);
            let field = c.hit_rect(Hit::Custom(EDIT_FIELD)).unwrap();
            assert!(field.bottom() <= keys_y && keys_y - field.bottom() <= field.x, "{field:?}");
            assert!(field.h >= th.touch_min);
            // The title bar's back button is the way out without the text.
            assert_eq!(c.hit_rect(Hit::Key(Key::Cancel)), Some(IRect::new(0, 0, th.back_w, th.title_h)));
            assert!(matches!(s.key(cx, Key::Cancel), Nav::Pop));
            assert_eq!(*got.lock().unwrap(), None);
            // Typed, and taken by the action key.
            assert!(matches!(s.tap(cx, Cap::Char('c').hit(), 0, 0), Nav::Stay));
            assert!(matches!(s.tap(cx, Cap::Done.hit(), 0, 0), Nav::Pop));
            assert_eq!(got.lock().unwrap().as_deref(), Some("abc"));
        });
    }

    #[test]
    fn a_whole_number_has_a_pad_without_a_point_and_is_shown() {
        with_cx(|cx, c, area| {
            let mut s = EditScreen::boxed("edit", "Timeout", "60", InputMode::Digits, |_, _| Nav::Pop);
            redraw(&mut *s, cx, c, area);
            assert!(c.hit_rect(Cap::Char('5').hit()).is_some());
            assert!(c.hit_rect(Cap::Char('.').hit()).is_none());
            // The keys of a phone that has them: digits, and nothing for
            // the star (a decimal point in a `Numeric` field).
            assert!(matches!(s.key(cx, Key::Star), Nav::Stay));
            assert!(matches!(s.key(cx, Key::Digit(5)), Nav::Stay));
            assert_eq!(s.text, "605");
        });
        // It is shown as it is, where a PIN is dots.
        let th = Theme::for_model(Model::D80);
        let field = IRect::new(0, 0, 400, 96);
        let draw = |mode: InputMode| {
            let mut c = Canvas::new(400, 96, false);
            widgets::text_field(&mut c, th, field, "60", 2, mode, true, false, "", Font::Normal);
            c.pixels().to_vec()
        };
        assert!(draw(InputMode::Digits) == draw(InputMode::Numeric));
        assert!(draw(InputMode::Digits) != draw(InputMode::Pin));
    }

    #[test]
    fn a_question_says_what_it_does_and_red_is_for_what_destroys() {
        with_cx(|cx, c, area| {
            let th = cx.th;
            let mut plain = Confirm::boxed("Restart the phone?", |_| Nav::Home);
            redraw(&mut *plain, cx, c, area);
            let action = c.hit_rect(Hit::Soft(1)).unwrap();
            assert_eq!(c.pixel(action.x + 12, action.y + 12), th.sel_bg);
            let without_title = c.pixels().to_vec();
            let mut s = Confirm::boxed("This erases every setting, and the phone starts again.", |_| Nav::Home)
                .title("Factory reset")
                .action("Erase and restart")
                .destructive();
            redraw(&mut *s, cx, c, area);
            let (action, cancel) = (c.hit_rect(Hit::Soft(1)).unwrap(), c.hit_rect(Hit::Soft(4)).unwrap());
            // The action is red, at the right; Cancel is not.
            assert_eq!(c.pixel(action.x + 12, action.y + 12), th.sk_stop);
            assert_eq!(c.pixel(cancel.x + 12, cancel.y + 12), th.sk_bg);
            assert!(cancel.right() < action.x);
            assert!(c.pixels() != &without_title[..]);
            // The buttons do what the softkeys do on the other phones.
            assert!(matches!(s.softkey(cx, 4), Nav::Pop));
            assert!(matches!(s.softkey(cx, 1), Nav::Home));
        });
    }

    #[test]
    fn a_menu_item_can_be_a_switch_and_can_be_red() {
        with_cx(|cx, c, area| {
            let th = cx.th;
            let on = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let state = on.clone();
            let mut m = Menu::new("m", "Settings", move |_| {
                let flip = state.clone();
                vec![
                    Item::new("Dim when idle", move |_| {
                        flip.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                        Nav::Stay
                    })
                    .switch(state.load(std::sync::atomic::Ordering::Relaxed)),
                    Item::new("Plain", |_| Nav::Stay),
                    Item::new("Factory reset", |_| Nav::Stay).destructive(),
                ]
            });
            redraw(&mut m, cx, c, area);
            let row = |i: u32| Hit::Row { list: 0, row: i };
            let r0 = c.hit_rect(row(0)).unwrap();
            // The switch is at the right of its row; off, its knob is at
            // the left.
            let (w, h) = widgets::switch_size(th);
            let track = IRect::new(r0.right() - 2 * th.pad - w, r0.y + (r0.h - h) / 2, w, h);
            assert_eq!(c.pixel(track.x + h / 2, track.y + h / 2), th.secondary);
            // A touch on the row does what the item does; the switch
            // shows the state.
            assert!(matches!(m.tap(cx, row(0), 0, 0), Nav::Stay));
            redraw(&mut m, cx, c, area);
            assert_eq!(c.pixel(track.x + h / 2, track.y + h / 2), th.fg);
            assert_eq!(c.pixel(track.right() - h / 2, track.y + h / 2), th.bg);
            // A plain row has nothing there, and the row that destroys
            // has its label in the error color.
            let r1 = c.hit_rect(row(1)).unwrap();
            assert_eq!(c.pixel(track.x + h / 2, r1.y + r1.h / 2), th.bg);
            let r2 = c.hit_rect(row(2)).unwrap();
            let has = |r: IRect, color| (r.y..r.bottom()).any(|y| (r.x..r.x + 300).any(|x| c.pixel(x, y) == color));
            assert!(has(r2, th.error) && !has(r1, th.error));
        });
    }
}
