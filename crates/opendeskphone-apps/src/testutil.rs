//! Fakes for tests and the simulator: an in-memory [`Shell`] whose widget
//! trees can be inspected and driven, a scripted PBX, and a fixed
//! configuration.

use crate::services::{
    AppAccount, Completion, ConfigSource, HttpClient, HttpReply, HttpRequest, NoStore,
    PbxRequest, PbxRequests, PbxStore, Services, StoreEntry, StoreOp, StoreReply, StoreScope,
    StoreSignal, StoreUser,
};
use crate::view::*;
use opendeskphone_state::Key;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Node {
    pub id: u32,
    pub kind: WidgetKind,
    pub parent: Option<u32>,
    pub bounds: Rect,
    pub visible: bool,
    pub text: String,
    pub font: Font,
    pub wrap: bool,
    pub highlight: bool,
    pub items: Vec<ListItem>,
    pub selected: u32,
    pub value: String,
    pub mode: InputMode,
    pub max_len: u32,
    pub image: String,
    pub softkeys: Vec<Softkey>,
    pub claims: Vec<Key>,
    /// The widget takes touches.
    pub touch: bool,
    /// What a canvas shows (`None`: not a canvas).
    pub drawing: Option<Vec<DrawOp>>,
}

#[derive(Default)]
pub struct Win {
    pub nodes: BTreeMap<u32, Node>,
    next: u32,
    pub focus: Option<u32>,
    pub softkeys: Vec<Softkey>,
    pub claims: Vec<Key>,
    /// The window takes the touches no widget takes.
    pub touch: bool,
    /// Package pictures handed over with `define_image`: name and size.
    pub images: Vec<(String, usize)>,
    /// How many times the app has drawn on a canvas.
    pub draws: u32,
    /// The host holds the window's changes back (a handler runs), and how
    /// many times it has begun to.
    pub held: bool,
    pub holds: u32,
    /// The window has the whole display.
    pub full_screen: bool,
    /// The window takes every key and hookswitch change.
    pub capture: bool,
    /// The lamps the app set, each with its color.
    pub lamps: BTreeMap<Lamp, opendeskphone_state::LampColor>,
    events: VecDeque<WindowEvent>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Win {
    fn push(&mut self, ev: WindowEvent) {
        self.events.push_back(ev);
        if let Some(w) = &self.wake {
            w();
        }
    }

    fn descendants(&self, w: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![w];
        while let Some(p) = stack.pop() {
            for n in self.nodes.values().filter(|n| n.parent == Some(p)) {
                out.push(n.id);
                stack.push(n.id);
            }
        }
        out
    }

    /// Softkeys in effect: those of the focused widget or its nearest
    /// ancestor with any, else the window's.
    pub fn effective_softkeys(&self) -> Vec<Softkey> {
        let mut cur = self.focus;
        while let Some(id) = cur {
            match self.nodes.get(&id) {
                Some(n) if !n.softkeys.is_empty() => return n.softkeys.clone(),
                Some(n) => cur = n.parent,
                None => break,
            }
        }
        self.softkeys.clone()
    }

    fn claimed(&self, key: Key) -> bool {
        let mut cur = self.focus;
        while let Some(id) = cur {
            match self.nodes.get(&id) {
                Some(n) if n.claims.contains(&key) => return true,
                Some(n) => cur = n.parent,
                None => break,
            }
        }
        self.claims.contains(&key)
    }

    /// Where a widget's top-left corner is in the window, if the widget
    /// and everything it is in are shown and (`x`, `y`) is in them all.
    fn origin_under(&self, id: u32, x: i32, y: i32) -> Option<(i32, i32)> {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            let n = self.nodes.get(&i)?;
            chain.push(n);
            cur = n.parent;
        }
        let (mut ox, mut oy) = (0, 0);
        for n in chain.iter().rev() {
            (ox, oy) = (ox + i32::from(n.bounds.x), oy + i32::from(n.bounds.y));
            let inside = x >= ox && y >= oy && x < ox + i32::from(n.bounds.width) && y < oy + i32::from(n.bounds.height);
            if !n.visible || !inside {
                return None;
            }
        }
        Some((ox, oy))
    }

    /// What a finger at (`x`, `y`) of the window is on, as the toolkit
    /// finds it: the last made of the widgets there that take touches or
    /// are a list, an input, or a scroll widget (with its top-left
    /// corner); else the window, if it takes touches (widget 0).
    fn touched(&self, x: i32, y: i32) -> Option<(u32, i32, i32)> {
        let mut found = self.touch.then_some((0, 0, 0));
        for n in self.nodes.values() {
            let stock = matches!(n.kind, WidgetKind::TextList | WidgetKind::Input | WidgetKind::Scroll) && n.drawing.is_none();
            if (n.touch || stock)
                && let Some((ox, oy)) = self.origin_under(n.id, x, y)
            {
                found = Some((n.id, ox, oy));
            }
        }
        found
    }

    fn takes_touches(&self, id: u32) -> bool {
        if id == 0 { self.touch } else { self.nodes.get(&id).is_some_and(|n| n.touch) }
    }

    /// A tap on a row of a list: the list is focused, and the row is
    /// highlighted and activated.
    fn tap_row(&mut self, list: u32, row: u32) {
        self.focus = Some(list);
        let n = self.nodes.get_mut(&list).expect("the list");
        assert!((row as usize) < n.items.len(), "the list has no row {row}");
        let changed = n.selected != row;
        n.selected = row;
        if changed {
            self.push(WindowEvent::Highlighted { widget: list, row });
        }
        self.push(WindowEvent::Activated { widget: list, row });
    }
}

fn point(x: i32, y: i32) -> Point {
    Point { x: x as i16, y: y as i16 }
}

#[derive(Default)]
struct UiState {
    windows: HashMap<(String, WindowKind), Win>,
    /// Navigation stack of app names; the last is on screen.
    stack: Vec<String>,
}

/// Handle to the fake screen, shared between the test and the manager.
#[derive(Clone, Default)]
pub struct FakeUi {
    state: Arc<Mutex<UiState>>,
    model: Option<opendeskphone_state::Model>,
}

fn metrics_for(model: opendeskphone_state::Model) -> Metrics {
    let area = model.app_content_area();
    let (skin, line, title, off, row, sb, color) = match model.skin() {
        opendeskphone_state::Skin::SmallGray => (Skin::SmallGray, 15, 15, 1, 15, 4, false),
        opendeskphone_state::Skin::LargeGray => (Skin::LargeGray, 17, 17, 1, 17, 4, false),
        opendeskphone_state::Skin::Color => (Skin::Color, 25, 40, 4, 32, 8, true),
        opendeskphone_state::Skin::Touch => (Skin::Touch, 44, 96, 8, 96, 12, true),
    };
    Metrics {
        skin,
        width: area.width,
        height: area.height,
        line_height: line,
        title_height: title,
        title_offset: off,
        row_height: row,
        scroll_bar_width: sb,
        color,
        keys: !model.is_touch(),
        back_width: if model.is_touch() { 96 } else { 0 },
    }
}

impl FakeUi {
    pub fn new(model: opendeskphone_state::Model) -> FakeUi {
        FakeUi {
            state: Default::default(),
            model: Some(model),
        }
    }

    pub fn shell(&self) -> Box<dyn Shell> {
        Box::new(self.clone())
    }

    /// The app on screen.
    pub fn top(&self) -> Option<String> {
        self.state.lock().unwrap().stack.last().cloned()
    }

    fn with<R>(&self, app: &str, f: impl FnOnce(&mut Win) -> R) -> R {
        let mut s = self.state.lock().unwrap();
        let win = s
            .windows
            .get_mut(&(app.to_string(), WindowKind::App))
            .unwrap_or_else(|| panic!("no window for app {app}"));
        f(win)
    }

    /// The widget that has the keyboard in the app's window.
    pub fn focused(&self, app: &str) -> Option<u32> {
        self.with(app, |w| w.focus)
    }

    /// Whether the host holds the window's changes back now, and how many
    /// times it has begun to.
    pub fn holds(&self, app: &str) -> (bool, u32) {
        self.with(app, |w| (w.held, w.holds))
    }

    pub fn has_window(&self, app: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .windows
            .contains_key(&(app.to_string(), WindowKind::App))
    }

    /// Texts of all visible text widgets, in creation order.
    pub fn texts(&self, app: &str) -> Vec<String> {
        self.with(app, |w| {
            w.nodes
                .values()
                .filter(|n| n.kind == WidgetKind::Text && n.visible)
                .map(|n| n.text.clone())
                .collect()
        })
    }

    /// What the app's canvases show, in creation order, each with its
    /// bounds.
    pub fn drawings(&self, app: &str) -> Vec<(Rect, Vec<DrawOp>)> {
        self.with(app, |w| {
            w.nodes
                .values()
                .filter(|n| n.visible)
                .filter_map(|n| n.drawing.clone().map(|d| (n.bounds, d)))
                .collect()
        })
    }

    /// Whether the app's window has the whole display.
    pub fn full_screen(&self, app: &str) -> bool {
        self.with(app, |w| w.full_screen)
    }

    /// How many times the app has drawn on a canvas.
    /// Whether the app's window takes every key and hookswitch change.
    pub fn captures(&self, app: &str) -> bool {
        self.with(app, |w| w.capture)
    }

    /// The lamps the app has set: (name, color), in the phone's order.
    pub fn lamps(&self, app: &str) -> Vec<(String, opendeskphone_state::LampColor)> {
        self.with(app, |w| w.lamps.iter().map(|(l, c)| (l.name(), *c)).collect())
    }

    /// Lifts the handset or puts it back; the app hears of it only while
    /// its window captures input.
    pub fn hook(&self, app: &str, off_hook: bool) {
        self.with(app, |w| {
            if w.capture {
                w.push(WindowEvent::Hook(off_hook));
            }
        })
    }

    pub fn draw_count(&self, app: &str) -> u32 {
        self.with(app, |w| w.draws)
    }

    /// Texts of the visible text widgets drawn highlighted.
    pub fn highlighted(&self, app: &str) -> Vec<String> {
        self.with(app, |w| {
            w.nodes
                .values()
                .filter(|n| n.kind == WidgetKind::Text && n.visible && n.highlight)
                .map(|n| n.text.clone())
                .collect()
        })
    }

    /// Texts of the app's idle-screen window.
    pub fn idle_texts(&self, app: &str) -> Vec<String> {
        let s = self.state.lock().unwrap();
        s.windows
            .get(&(app.to_string(), WindowKind::Idle))
            .map(|w| w.nodes.values().filter(|n| n.kind == WidgetKind::Text).map(|n| n.text.clone()).collect())
            .unwrap_or_default()
    }

    /// The idle screen came on screen (`true`) or was covered, as the
    /// phone UI reports it to an app's idle window.
    pub fn idle_screen(&self, app: &str, shown: bool) {
        let mut s = self.state.lock().unwrap();
        if let Some(w) = s.windows.get_mut(&(app.to_string(), WindowKind::Idle)) {
            w.push(if shown { WindowEvent::Foreground } else { WindowEvent::Background });
        }
    }

    /// One of the phone's own screens went over the app's window
    /// (`true`), or left it on top again.
    pub fn cover(&self, app: &str, covered: bool) {
        self.with(app, |w| {
            w.push(if covered {
                WindowEvent::Background
            } else {
                WindowEvent::Foreground
            })
        })
    }

    /// Package pictures the app's window was handed: (name, file size).
    pub fn images(&self, app: &str) -> Vec<(String, usize)> {
        self.with(app, |w| w.images.clone())
    }

    /// The picture names of the app's image widgets, in creation order.
    pub fn image_names(&self, app: &str) -> Vec<String> {
        self.with(app, |w| w.nodes.values().filter(|n| n.kind == WidgetKind::Image).map(|n| n.image.clone()).collect())
    }

    /// True when any visible text contains `needle`.
    pub fn shows(&self, app: &str, needle: &str) -> bool {
        self.texts(app).iter().any(|t| t.contains(needle))
            || self
                .items(app)
                .iter()
                .any(|i| i.0.contains(needle) || i.1.contains(needle))
    }

    /// (text, detail) rows of the focused list, or the first list.
    pub fn items(&self, app: &str) -> Vec<(String, String)> {
        self.with(app, |w| {
            let list = w
                .focus
                .and_then(|f| w.nodes.get(&f))
                .filter(|n| n.kind == WidgetKind::TextList)
                .or_else(|| w.nodes.values().find(|n| n.kind == WidgetKind::TextList));
            list.map(|n| {
                n.items
                    .iter()
                    .map(|i| (i.text.clone(), i.detail.clone()))
                    .collect()
            })
            .unwrap_or_default()
        })
    }

    pub fn softkeys(&self, app: &str) -> Vec<String> {
        self.with(app, |w| {
            w.effective_softkeys()
                .into_iter()
                .map(|k| k.label)
                .collect()
        })
    }

    /// Values of the input widgets, in creation order.
    pub fn inputs(&self, app: &str) -> Vec<String> {
        self.with(app, |w| {
            w.nodes
                .values()
                .filter(|n| n.kind == WidgetKind::Input)
                .map(|n| n.value.clone())
                .collect()
        })
    }

    /// Presses the softkey with this label.  Panics when it is not shown.
    pub fn press_softkey(&self, app: &str, label: &str) {
        self.with(app, |w| {
            let keys = w.effective_softkeys();
            let k = keys.iter().find(|k| k.label == label).unwrap_or_else(|| {
                panic!(
                    "no softkey {label:?}; have {:?}",
                    keys.iter().map(|k| &k.label).collect::<Vec<_>>()
                )
            });
            w.push(WindowEvent::Softkey(k.id));
        })
    }

    /// Presses a key, with the toolkit's built-in behavior for unclaimed
    /// keys.
    pub fn press(&self, app: &str, key: Key) {
        self.with(app, |w| {
            if w.capture || w.claimed(key) {
                w.push(WindowEvent::Key(key));
                return;
            }
            let focused = w.focus.and_then(|f| w.nodes.get_mut(&f));
            match (key, focused) {
                (Key::Cancel, _) => w.push(WindowEvent::CloseRequested),
                (Key::Up | Key::Down, Some(n)) if n.kind == WidgetKind::TextList => {
                    let len = n.items.len() as u32;
                    if len == 0 {
                        return;
                    }
                    let row = if key == Key::Down {
                        (n.selected + 1).min(len - 1)
                    } else {
                        n.selected.saturating_sub(1)
                    };
                    if row != n.selected {
                        n.selected = row;
                        let widget = n.id;
                        w.push(WindowEvent::Highlighted { widget, row });
                    }
                }
                (Key::Select, Some(n)) if n.kind == WidgetKind::TextList => {
                    if !n.items.is_empty() {
                        let (widget, row) = (n.id, n.selected);
                        w.push(WindowEvent::Activated { widget, row });
                    }
                }
                (Key::Select, Some(n)) if n.kind == WidgetKind::Input => {
                    let (widget, value) = (n.id, n.value.clone());
                    w.push(WindowEvent::Submitted { widget, value });
                }
                (k, Some(n)) if n.kind == WidgetKind::Input && k.dial_char().is_some() => {
                    n.value.push(k.dial_char().unwrap());
                    let (widget, value) = (n.id, n.value.clone());
                    w.push(WindowEvent::Changed { widget, value });
                }
                _ => {}
            }
        })
    }

    /// Touch: a finger taps the window at (`x`, `y`), with the toolkit's
    /// built-in behavior on a widget that does not take touches: a row of
    /// a list is highlighted and activated (the fake's lists do not
    /// scroll: their rows are those from the first), and an input is
    /// focused.
    pub fn tap(&self, app: &str, x: i32, y: i32) {
        let row_h = i32::from(metrics_for(self.model.unwrap_or(opendeskphone_state::Model::D80)).row_height);
        self.with(app, |w| {
            let Some((id, ox, oy)) = w.touched(x, y) else { return };
            if w.takes_touches(id) {
                w.push(WindowEvent::Touch { widget: id, touch: Touch::Tap(point(x - ox, y - oy)) });
                return;
            }
            let n = &w.nodes[&id];
            match n.kind {
                WidgetKind::TextList => {
                    let row = ((y - oy) / row_h) as usize;
                    if row < n.items.len() {
                        w.tap_row(id, row as u32);
                    }
                }
                WidgetKind::Input => w.focus = Some(id),
                _ => {}
            }
        })
    }

    /// Touch: a finger taps row `row` of the focused list, or the first
    /// list (wherever the list has scrolled to).
    pub fn tap_row(&self, app: &str, row: u32) {
        self.with(app, |w| {
            let list = w
                .focus
                .filter(|f| w.nodes.get(f).is_some_and(|n| n.kind == WidgetKind::TextList))
                .or_else(|| w.nodes.values().find(|n| n.kind == WidgetKind::TextList && n.visible).map(|n| n.id))
                .expect("a list");
            w.tap_row(list, row);
        })
    }

    /// Touch: a finger comes down at `from` of the window, moves to `to`,
    /// and is lifted.  A widget that takes touches gets the drag and its
    /// end; nothing else does anything with it here (the fake's lists do
    /// not scroll).
    pub fn drag(&self, app: &str, from: (i32, i32), to: (i32, i32)) {
        self.with(app, |w| {
            let Some((id, ox, oy)) = w.touched(from.0, from.1) else { return };
            if !w.takes_touches(id) {
                return;
            }
            let ends = (point(from.0 - ox, from.1 - oy), point(to.0 - ox, to.1 - oy));
            w.push(WindowEvent::Touch { widget: id, touch: Touch::Drag(ends) });
            w.push(WindowEvent::Touch { widget: id, touch: Touch::DragEnd(ends) });
        })
    }

    /// Whether a widget (0: the window) takes touches.
    pub fn takes_touches(&self, app: &str, widget: u32) -> bool {
        self.with(app, |w| w.takes_touches(widget))
    }

    /// Replaces the text of the focused input, as if typed.
    pub fn type_text(&self, app: &str, text: &str) {
        self.with(app, |w| {
            let n = w
                .focus
                .and_then(|f| w.nodes.get_mut(&f))
                .filter(|n| n.kind == WidgetKind::Input)
                .expect("an input has focus");
            n.value = text.to_string();
            let (widget, value) = (n.id, n.value.clone());
            w.push(WindowEvent::Changed { widget, value });
        })
    }

    /// Highlights a row of the focused list.
    pub fn highlight(&self, app: &str, row: u32) {
        self.with(app, |w| {
            let n = w
                .focus
                .and_then(|f| w.nodes.get_mut(&f))
                .filter(|n| n.kind == WidgetKind::TextList)
                .expect("a list has focus");
            n.selected = row;
            let widget = n.id;
            w.push(WindowEvent::Highlighted { widget, row });
        })
    }

    /// Number of live widgets (to check apps do not leak them).
    pub fn widget_count(&self, app: &str) -> usize {
        self.with(app, |w| w.nodes.len())
    }
}

struct FakeWindow {
    ui: FakeUi,
    key: (String, WindowKind),
}

impl FakeWindow {
    fn with<R>(&self, f: impl FnOnce(&mut Win) -> R) -> R {
        let mut s = self.ui.state.lock().unwrap();
        f(s.windows.entry(self.key.clone()).or_default())
    }
    fn node(&self, w: u32, f: impl FnOnce(&mut Node)) {
        self.with(|win| {
            if let Some(n) = win.nodes.get_mut(&w) {
                f(n)
            }
        })
    }
}

impl FakeWindow {
    fn add(&self, kind: WidgetKind, parent: Option<u32>, bounds: Rect, canvas: bool) -> u32 {
        self.with(|w| {
            w.next += 1;
            let id = w.next;
            w.nodes.insert(
                id,
                Node {
                    id,
                    kind,
                    parent,
                    bounds,
                    visible: true,
                    text: String::new(),
                    font: Font::Normal,
                    wrap: false,
                    highlight: false,
                    items: Vec::new(),
                    selected: 0,
                    value: String::new(),
                    mode: InputMode::Text,
                    max_len: 0,
                    image: String::new(),
                    softkeys: Vec::new(),
                    claims: Vec::new(),
                    touch: false,
                    drawing: canvas.then(Vec::new),
                },
            );
            id
        })
    }
}

impl AppWindow for FakeWindow {
    fn metrics(&self) -> Metrics {
        let model = self.ui.model.unwrap_or(opendeskphone_state::Model::D60);
        let mut m = metrics_for(model);
        if self.with(|w| w.full_screen) {
            m.height = model.main_screen().height;
        }
        m
    }
    fn set_full_screen(&mut self, on: bool) {
        self.with(|w| w.full_screen = on)
    }
    fn capture_input(&mut self, on: bool) {
        self.with(|w| w.capture = on)
    }
    fn set_lamp(&mut self, lamp: Lamp, color: opendeskphone_state::LampColor) {
        self.with(|w| {
            w.lamps.insert(lamp, color);
        })
    }
    fn release_lamps(&mut self) {
        self.with(|w| w.lamps.clear())
    }
    fn create(&mut self, kind: WidgetKind, parent: Option<u32>, bounds: Rect) -> u32 {
        self.add(kind, parent, bounds, false)
    }
    /// A different color for every role: the role's number in each part.
    fn theme_color(&self, role: ColorRole) -> Color {
        let v = (role as u8 + 1) * 16;
        Color { r: v, g: v, b: v }
    }
    /// A size for every font, the same for any one character: small 5
    /// by 6, normal 6 by 8, bold 7 by 9, title 8 by 10, and large 12 by
    /// 16 pixels.
    fn text_size(&self, font: Font, text: &str) -> (u16, u16) {
        let (w, h) = match font {
            Font::Small => (5, 6),
            Font::Normal => (6, 8),
            Font::Bold => (7, 9),
            Font::Title => (8, 10),
            Font::Large => (12, 16),
        };
        (w * text.chars().count() as u16, h)
    }
    fn create_canvas(&mut self, parent: Option<u32>, bounds: Rect) -> u32 {
        self.add(WidgetKind::Group, parent, bounds, true)
    }
    fn draw(&mut self, w: u32, ops: Vec<DrawOp>) -> bool {
        self.with(|win| match win.nodes.get_mut(&w).filter(|n| n.drawing.is_some()) {
            Some(n) => {
                n.drawing = Some(ops);
                win.draws += 1;
                true
            }
            None => false,
        })
    }
    fn destroy(&mut self, id: u32) {
        self.with(|w| {
            for d in w.descendants(id) {
                w.nodes.remove(&d);
            }
            w.nodes.remove(&id);
            if w.focus.is_some_and(|f| !w.nodes.contains_key(&f)) {
                w.focus = None;
            }
        })
    }
    fn clear(&mut self, id: Option<u32>) {
        self.with(|w| match id {
            None => {
                w.nodes.clear();
                w.focus = None;
                w.softkeys.clear();
                w.claims.clear();
                w.touch = false;
            }
            Some(id) => {
                for d in w.descendants(id) {
                    w.nodes.remove(&d);
                }
                if w.focus.is_some_and(|f| !w.nodes.contains_key(&f)) {
                    w.focus = None;
                }
            }
        })
    }
    fn set_bounds(&mut self, w: u32, bounds: Rect) {
        self.node(w, |n| n.bounds = bounds)
    }
    fn set_visible(&mut self, w: u32, visible: bool) {
        self.node(w, |n| n.visible = visible);
        self.with(|win| {
            if !visible && win.focus.is_some_and(|f| f == w || win.descendants(w).contains(&f)) {
                win.focus = None;
            }
        })
    }
    fn hold_changes(&mut self, on: bool) {
        self.with(|w| {
            w.held = on;
            if on {
                w.holds += 1;
            }
        })
    }
    fn set_focus(&mut self, id: Option<u32>) {
        self.with(|w| w.focus = id.filter(|i| w.nodes.contains_key(i)))
    }
    fn set_text(&mut self, w: u32, text: &str) {
        self.node(w, |n| n.text = text.to_string())
    }
    fn set_align(&mut self, _w: u32, _align: Align) {}
    fn set_font(&mut self, w: u32, font: Font) {
        self.node(w, |n| n.font = font)
    }
    fn set_wrap(&mut self, w: u32, wrap: bool) {
        self.node(w, |n| n.wrap = wrap)
    }
    fn set_highlight(&mut self, w: u32, on: bool) {
        self.node(w, |n| n.highlight = on)
    }
    fn set_items(&mut self, w: u32, items: &[ListItem]) {
        self.node(w, |n| {
            n.items = items.to_vec();
            n.selected = n.selected.min(items.len().saturating_sub(1) as u32);
        })
    }
    fn set_selected(&mut self, w: u32, index: u32) {
        self.node(w, |n| {
            n.selected = index.min(n.items.len().saturating_sub(1) as u32)
        })
    }
    fn selected(&self, w: u32) -> Option<u32> {
        self.with(|win| {
            win.nodes
                .get(&w)
                .filter(|n| !n.items.is_empty())
                .map(|n| n.selected)
        })
    }
    fn set_value(&mut self, w: u32, value: &str) {
        self.node(w, |n| n.value = value.to_string())
    }
    fn value(&self, w: u32) -> String {
        self.with(|win| {
            win.nodes
                .get(&w)
                .map(|n| n.value.clone())
                .unwrap_or_default()
        })
    }
    fn set_input_mode(&mut self, w: u32, mode: InputMode) {
        self.node(w, |n| n.mode = mode)
    }
    fn set_max_length(&mut self, w: u32, max: u32) {
        self.node(w, |n| n.max_len = max)
    }
    fn set_image(&mut self, w: u32, image: &Image) {
        let name = match image {
            Image::Icon(icon) => icon_name(*icon).to_string(),
            Image::Picture(name) => name.clone(),
        };
        self.node(w, |n| n.image = name)
    }
    fn define_image(&mut self, name: &str, file: &[u8]) -> bool {
        if !file.starts_with(b"\x89PNG") {
            return false;
        }
        self.with(|w| w.images.push((name.to_string(), file.len())));
        true
    }
    fn set_softkeys(&mut self, id: Option<u32>, keys: &[Softkey]) {
        self.with(|w| match id {
            None => w.softkeys = keys.to_vec(),
            Some(id) => {
                if let Some(n) = w.nodes.get_mut(&id) {
                    n.softkeys = keys.to_vec()
                }
            }
        })
    }
    fn claim_keys(&mut self, id: Option<u32>, keys: &[Key]) {
        self.with(|w| match id {
            None => w.claims = keys.to_vec(),
            Some(id) => {
                if let Some(n) = w.nodes.get_mut(&id) {
                    n.claims = keys.to_vec()
                }
            }
        })
    }
    fn claim_touch(&mut self, id: Option<u32>, on: bool) {
        self.with(|w| match id {
            None => w.touch = on,
            Some(id) => {
                if let Some(n) = w.nodes.get_mut(&id) {
                    n.touch = on
                }
            }
        })
    }
    fn poll_event(&mut self) -> Option<WindowEvent> {
        self.with(|w| w.events.pop_front())
    }
}

impl Shell for FakeUi {
    fn open(
        &mut self,
        app: &str,
        kind: WindowKind,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Box<dyn AppWindow> {
        let key = (app.to_string(), kind);
        let mut s = self.state.lock().unwrap();
        s.windows.insert(
            key.clone(),
            Win {
                wake: Some(wake),
                ..Win::default()
            },
        );
        Box::new(FakeWindow {
            ui: self.clone(),
            key,
        })
    }
    fn show(&mut self, app: &str) {
        let mut s = self.state.lock().unwrap();
        if s.stack.last().map(String::as_str) == Some(app) {
            return;
        }
        if let Some(prev) = s.stack.last().cloned()
            && let Some(w) = s.windows.get_mut(&(prev, WindowKind::App))
        {
            w.push(WindowEvent::Background);
        }
        s.stack.retain(|a| a != app);
        s.stack.push(app.to_string());
        if let Some(w) = s.windows.get_mut(&(app.to_string(), WindowKind::App)) {
            w.push(WindowEvent::Foreground);
        }
    }
    fn hide(&mut self, app: &str) {
        let mut s = self.state.lock().unwrap();
        let was_top = s.stack.last().map(String::as_str) == Some(app);
        s.stack.retain(|a| a != app);
        if was_top {
            if let Some(w) = s.windows.get_mut(&(app.to_string(), WindowKind::App)) {
                w.push(WindowEvent::Background);
            }
            if let Some(next) = s.stack.last().cloned()
                && let Some(w) = s.windows.get_mut(&(next, WindowKind::App))
            {
                w.push(WindowEvent::Foreground);
            }
        }
    }
    fn close(&mut self, app: &str, kind: WindowKind) {
        let mut s = self.state.lock().unwrap();
        s.windows.remove(&(app.to_string(), kind));
        if kind == WindowKind::App {
            s.stack.retain(|a| a != app);
        }
    }
    fn idle_size(&self) -> Size {
        Size {
            width: 200,
            height: 40,
        }
    }
}

type Handler = Box<dyn Fn(&Value) -> Result<Value, Value> + Send + Sync>;

/// A scripted PBX: answers each method from a handler, at once.
#[derive(Clone, Default)]
pub struct FakePbx {
    handlers: Arc<Mutex<HashMap<String, Handler>>>,
    log: Arc<Mutex<Vec<(String, Value)>>>,
    seen: Arc<Mutex<Vec<PbxRequest>>>,
}

impl FakePbx {
    /// `f` gets the request parameters and returns `response.result`, or
    /// `Err` with `response.errors`.
    pub fn on(
        &self,
        method: &str,
        f: impl Fn(&Value) -> Result<Value, Value> + Send + Sync + 'static,
    ) {
        self.handlers
            .lock()
            .unwrap()
            .insert(method.to_string(), Box::new(f));
    }

    /// (method, parameters) of every request so far.
    pub fn requests(&self) -> Vec<(String, Value)> {
        self.log.lock().unwrap().clone()
    }

    pub fn raw_requests(&self) -> Vec<PbxRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// Parameters of the last request for a method.
    pub fn last(&self, method: &str) -> Option<Value> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
    }

    pub fn count(&self, method: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .count()
    }
}

impl PbxRequests for FakePbx {
    fn request(&self, request: PbxRequest, done: Completion<Result<Vec<u8>, String>>) {
        self.seen.lock().unwrap().push(request.clone());
        let v: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
        let method = v["request"]["method"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let params = v["request"]["parameters"].clone();
        self.log
            .lock()
            .unwrap()
            .push((method.clone(), params.clone()));
        let reply = match self.handlers.lock().unwrap().get(&method) {
            Some(h) => match h(&params) {
                Ok(result) => serde_json::json!({"response": {"method": method, "result": result}}),
                Err(errors) => {
                    serde_json::json!({"response": {"method": method, "errors": errors}})
                }
            },
            None => serde_json::json!({"response": {"method": method, "errors": {"error":
                {"code": "10001", "message": "Unknown method"}}}}),
        };
        done.complete(Ok(reply.to_string().into_bytes()));
    }
}

/// An HTTP client with no network.
pub struct NoHttp;

impl HttpClient for NoHttp {
    fn request(&self, _request: HttpRequest, done: Completion<Result<HttpReply, String>>) {
        done.complete(Err("no network in this test".into()));
    }
}

/// A fixed configuration.
#[derive(Clone, Default)]
pub struct FakeConfig {
    pub account: Option<AppAccount>,
    pub app_settings: HashMap<String, String>,
    pub contact_lists: Vec<(String, String)>,
    /// Built-in apps the configuration switches off.  A test may change
    /// the list while apps run.
    pub disabled: Arc<Mutex<Vec<String>>>,
    /// Local time's offset from UTC, in seconds.
    pub utc_offset: i32,
    /// The wall clock apps see, in seconds since the Unix epoch (`None`:
    /// the real one).  A test may change it while apps run.
    pub wall_time: Option<Arc<std::sync::atomic::AtomicU64>>,
}

impl FakeConfig {
    /// Every app shares one DPMA application account.
    pub fn dasp() -> FakeConfig {
        FakeConfig {
            account: Some(AppAccount {
                account_id: "100".into(),
                username: "100".into(),
                password: "secret".into(),
                appserver: "dasp://pbx.example/json".into(),
                apiformat: "json".into(),
            }),
            ..Default::default()
        }
    }
}

impl ConfigSource for FakeConfig {
    fn app_config_xml(&self, app: &str) -> String {
        format!("<appconfig id=\"{app}\"/>")
    }
    fn app_account(&self, _app: &str) -> Option<AppAccount> {
        self.account.clone()
    }
    fn app_setting(&self, _app: &str, name: &str) -> Option<String> {
        self.app_settings.get(name).cloned()
    }
    fn app_enabled(&self, app: &str) -> bool {
        !self.disabled.lock().unwrap().iter().any(|d| d == app)
    }
    fn utc_offset_secs(&self) -> i32 {
        self.utc_offset
    }
    fn wall_time(&self) -> u64 {
        match &self.wall_time {
            Some(t) => t.load(std::sync::atomic::Ordering::Relaxed),
            None => opendeskphone_state::clock::unix_secs(),
        }
    }
    fn setting(&self, _id: &str) -> Option<String> {
        None
    }
    fn mac_address(&self) -> String {
        "000fd3000001".into()
    }
    fn contact_lists(&self) -> Vec<(String, String)> {
        self.contact_lists.clone()
    }
}

type Subscriber = Box<dyn Fn(StoreSignal) + Send + Sync>;

/// One phone's side of a [`FakeStore`].
struct StoreSide {
    user: StoreUser,
    tenant: String,
    online: std::sync::atomic::AtomicBool,
    watch: Mutex<Vec<String>>,
    subscriber: Mutex<Option<Subscriber>>,
    log: Mutex<Vec<StoreOp>>,
}

#[derive(Default)]
struct StorePbx {
    backend: dpma_proto::appdata::store::MemoryBackend,
    sides: Vec<Arc<StoreSide>>,
    now: u64,
}

/// A PBX's store of app data, as one phone reaches it.  It keeps its keys
/// as the DPMA module does (the same code decides every answer), at once
/// and in memory.  [`FakeStore::phone`] is another phone of the same PBX,
/// for tests with two phones.
#[derive(Clone)]
pub struct FakeStore {
    pbx: Arc<Mutex<StorePbx>>,
    side: Arc<StoreSide>,
}

impl Default for FakeStore {
    /// A PBX with one phone, of the user `100` named "Desk" with the
    /// extension 100, in reach.
    fn default() -> FakeStore {
        let pbx = Arc::new(Mutex::new(StorePbx {
            now: 1_794_607_500,
            ..Default::default()
        }));
        FakeStore::join(pbx, "100", "Desk", "")
    }
}

impl FakeStore {
    fn join(pbx: Arc<Mutex<StorePbx>>, id: &str, name: &str, tenant: &str) -> FakeStore {
        let side = Arc::new(StoreSide {
            user: StoreUser {
                id: id.into(),
                name: name.into(),
                extension: id.into(),
            },
            tenant: tenant.into(),
            online: true.into(),
            watch: Mutex::new(Vec::new()),
            subscriber: Mutex::new(None),
            log: Mutex::new(Vec::new()),
        });
        pbx.lock().unwrap().sides.push(side.clone());
        FakeStore { pbx, side }
    }

    /// Another phone of the same PBX, of the user `id` (also the
    /// extension) named `name`.
    pub fn phone(&self, id: &str, name: &str) -> FakeStore {
        FakeStore::join(self.pbx.clone(), id, name, &self.side.tenant)
    }

    /// Another phone of the same PBX, of another tenant.
    pub fn phone_of_tenant(&self, id: &str, name: &str, tenant: &str) -> FakeStore {
        FakeStore::join(self.pbx.clone(), id, name, tenant)
    }

    /// Puts the PBX in or out of this phone's reach.  Coming into reach
    /// is signaled, as on the phone.
    pub fn set_online(&self, online: bool) {
        let was = self
            .side
            .online
            .swap(online, std::sync::atomic::Ordering::SeqCst);
        if online && !was {
            self.signal(&self.side, StoreSignal::Online);
        }
    }

    /// The PBX's clock, which stamps what is written.
    pub fn set_time(&self, unix: u64) {
        self.pbx.lock().unwrap().now = unix;
    }

    /// Every request this phone made, in order.
    pub fn ops(&self) -> Vec<StoreOp> {
        self.side.log.lock().unwrap().clone()
    }

    /// The apps this phone watches.
    pub fn watched(&self) -> Vec<String> {
        self.side.watch.lock().unwrap().clone()
    }

    fn signal(&self, side: &StoreSide, signal: StoreSignal) {
        if let Some(f) = side.subscriber.lock().unwrap().as_ref() {
            f(signal);
        }
    }

    /// Does a request as this phone and returns its answer, as
    /// [`PbxStore::request`] does through its completion.
    pub fn run(&self, op: StoreOp) -> StoreReply {
        use dpma_proto::appdata::store::{Who, apply};
        use dpma_proto::appdata::{Entry, Op, Outcome, Scope, User};
        self.side.log.lock().unwrap().push(op.clone());
        if !self.side.online.load(std::sync::atomic::Ordering::SeqCst) {
            return StoreReply::Unreachable("not attached to a DPMA server".into());
        }
        let scope = |s| match s {
            StoreScope::Shared => Scope::Shared,
            StoreScope::User => Scope::User,
        };
        let op = match op {
            StoreOp::Get { app, scope: s, key } => Op::Get {
                app,
                scope: scope(s),
                key,
            },
            StoreOp::List {
                app,
                scope: s,
                prefix,
                after,
            } => Op::List {
                app,
                scope: scope(s),
                prefix,
                after,
            },
            StoreOp::Set {
                app,
                scope: s,
                key,
                value,
                expect,
            } => Op::Set {
                app,
                scope: scope(s),
                key,
                value,
                expect,
            },
            StoreOp::Remove {
                app,
                scope: s,
                key,
                expect,
            } => Op::Remove {
                app,
                scope: scope(s),
                key,
                expect,
            },
        };
        // Through the documents, so that what the server would refuse to
        // read is refused here.
        let op = match Op::decode(&op.encode()) {
            Ok(op) => op,
            Err(why) => return StoreReply::Refused(why),
        };
        let user = User {
            id: self.side.user.id.clone(),
            name: self.side.user.name.clone(),
            extension: self.side.user.extension.clone(),
        };
        let who = Who {
            tenant: self.side.tenant.as_bytes(),
            phone: user.id.as_bytes(),
            user: &user,
        };
        let (outcome, change, others) = {
            let mut pbx = self.pbx.lock().unwrap();
            let now = pbx.now;
            let (outcome, change) = apply(&mut pbx.backend, &who, &op, now);
            (outcome, change, pbx.sides.clone())
        };
        if let Some(change) = change {
            for other in others {
                let hears = !Arc::ptr_eq(&other, &self.side)
                    && other.tenant == self.side.tenant
                    && other.online.load(std::sync::atomic::Ordering::SeqCst)
                    && other.watch.lock().unwrap().contains(&change.app);
                if hears {
                    self.signal(
                        &other,
                        StoreSignal::Changed {
                            app: change.app.clone(),
                            scope: StoreScope::Shared,
                            key: change.key.clone(),
                        },
                    );
                }
            }
        }
        let entry = |e: Entry| StoreEntry {
            key: e.key,
            value: e.value,
            version: e.version,
            modified: e.modified,
            author_id: e.author_id,
            author_name: e.author_name,
        };
        match outcome {
            Outcome::Hello | Outcome::Removed => StoreReply::Removed,
            Outcome::Entry(e) => StoreReply::Entry(e.map(entry)),
            Outcome::Entries { entries, more } => {
                StoreReply::Entries(entries.into_iter().map(entry).collect(), more)
            }
            Outcome::Stored(e) => StoreReply::Stored(entry(e)),
            Outcome::Conflict(e) => StoreReply::Conflict(e.map(entry)),
            Outcome::Error { message, .. } => StoreReply::Refused(message),
        }
    }

    /// The value of a key of `app`, as this phone reads it.
    pub fn value(&self, app: &str, scope: StoreScope, key: &str) -> Option<Vec<u8>> {
        let op = StoreOp::Get {
            app: app.into(),
            scope,
            key: key.into(),
        };
        // Not a request of the phone's: it is not in the log.
        let log = self.side.log.lock().unwrap().len();
        let reply = self.run(op);
        self.side.log.lock().unwrap().truncate(log);
        match reply {
            StoreReply::Entry(e) => e.map(|e| e.value),
            _ => None,
        }
    }
}

impl PbxStore for FakeStore {
    fn request(&self, op: StoreOp, done: Completion<StoreReply>) {
        done.complete(self.run(op));
    }
    fn user(&self) -> Option<StoreUser> {
        // The phone has reached this PBX before: it knows its user.
        Some(self.side.user.clone())
    }
    fn online(&self) -> bool {
        self.side.online.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn watch(&self, apps: &[String]) {
        *self.side.watch.lock().unwrap() = apps.to_vec();
    }
    fn subscribe(&self, f: Box<dyn Fn(StoreSignal) + Send + Sync>) {
        *self.side.subscriber.lock().unwrap() = Some(f);
    }
}

/// The value of an app's `local` key, as the phone keeps it in `dir`
/// (the app's directory under `appdata`).
pub fn local_value(dir: &std::path::Path, key: &str) -> Option<Vec<u8>> {
    match crate::localstore::LocalStore::new(dir.to_path_buf()).get(key) {
        StoreReply::Entry(e) => e.map(|e| e.value),
        _ => None,
    }
}

/// Sets an app's `local` key in `dir` (the app's directory under
/// `appdata`), as the app would.
pub fn set_local(dir: &std::path::Path, key: &str, value: &[u8]) {
    let reply = crate::localstore::LocalStore::new(dir.to_path_buf()).set(key, value, None);
    assert!(matches!(reply, StoreReply::Stored(_)), "{key}: {reply:?}");
}

/// Services without a PBX store.
pub fn services(pbx: &FakePbx, config: FakeConfig) -> Services {
    Services {
        pbx: Arc::new(pbx.clone()),
        store: Arc::new(NoStore),
        http: Arc::new(NoHttp),
        config: Arc::new(config),
    }
}

/// Services with `store` as the PBX's store of app data.
pub fn services_with_store(pbx: &FakePbx, config: FakeConfig, store: &FakeStore) -> Services {
    Services {
        store: Arc::new(store.clone()),
        ..services(pbx, config)
    }
}
