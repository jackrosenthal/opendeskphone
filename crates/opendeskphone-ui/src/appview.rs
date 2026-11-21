//! The app view: the widget tree an app (the WASM host in `opendeskphone-apps`)
//! drives, and the events it gets back.
//!
//! # Contract for the app host
//!
//! One [`AppView`] per running app.  Create it with [`AppView::new`], which
//! also returns the [`AppEvents`] receiver.  Both are `Send`; the view is
//! `Clone + Sync` and may be driven from any thread (the UI thread only
//! reads it under the same lock when it renders or dispatches a key).
//!
//! ```text
//! let (view, events) = AppView::new("voicemail", model);
//! let title = view.create(WidgetKind::Text, None, Rect::new(0, 0, m.width, m.title_height));
//! view.set_text(title, "Voicemail");
//! let list = view.create(WidgetKind::List, None, Rect::new(0, m.title_height, m.width, m.height - m.title_height));
//! view.list_set_columns(list, &[Column::px(40), Column::rest()]);
//! view.list_set_rows(list, vec![vec![Cell::icon("voicemail"), Cell::text("Alice")]]);
//! view.focus(list);
//! view.set_softkeys(None, vec![Softkey::new(1, "Play"), Softkey::new(2, "Delete")]);
//! ui_handle.open_app(view.clone());            // push onto the navigation stack
//! while let Some(ev) = events.recv() { ... }   // or try_recv() + set_waker()
//! ui_handle.close_app("voicemail");
//! ```
//!
//! **Geometry.**  Widget rectangles are in pixels, relative to the parent
//! widget; top-level widgets (`parent == None`) are relative to the app
//! content area, whose size and stock text metrics are in
//! [`AppView::metrics`] (240x102, 320x140, 480x243, or 800x1160).  The
//! softkey bar below the content area belongs to the toolkit.
//!
//! **Widgets** mirror the stock set: [`WidgetKind::Group`], `Text`, `List`,
//! `Scroll`, `Input`, `Image`, and add `Canvas`, which shows a list of
//! drawing operations ([`AppView::canvas_set_ops`]).  They are addressed by [`WidgetId`] handles
//! which are never reused within a view.  Destroying a widget destroys its
//! children.  Calls on a dead handle are ignored (getters return `None`).
//!
//! **Focus.**  [`AppView::focus`] gives one widget the keyboard.  Built-in
//! key behavior of the focused widget:
//! - `List`: Up/Down move the selection ([`AppEvent::ListSelected`]);
//!   Select activates the row ([`AppEvent::ListActivated`]); digits 1-9,0
//!   activate rows 0..9 when [`AppView::list_set_shortcuts`] is on.
//! - `Input`: digits, `*`, `#` enter text (multi-tap in
//!   [`InputMode::Text`]), Left/Right move the cursor, softkey 3 deletes
//!   (the toolkit shows a "Delete" label there while the input has focus
//!   and softkey 3 is otherwise unset), Select submits
//!   ([`AppEvent::InputSubmitted`]); every edit sends
//!   [`AppEvent::InputChanged`].  Up/Down move focus to the previous/next
//!   focusable sibling `Input` or `List`.
//! - `Scroll`: Up/Down scroll when the focused widget is the scroll itself.
//!
//! **Key dispatch** follows the stock rule (walk up from the focused
//! widget), adapted to a host that cannot answer synchronously: a widget
//! *claims* keys with [`AppView::set_key_claims`].  For each key press the
//! toolkit walks from the focused widget up through its parents, then the
//! window claims ([`AppView::set_window_key_claims`]); the first claimant
//! gets [`AppEvent::Key`] and the key is consumed.  Unclaimed keys get the
//! built-in behavior above; an unclaimed `Cancel` sends
//! [`AppEvent::CloseRequested`] (the host decides whether to background or
//! exit).  Keys a view does not get this way: `Line(1)` backgrounds the app
//! (stock rule for nonprivileged apps), and the audio keys, hook, and
//! volume keys go to the phone.
//!
//! **Capture.**  With [`AppView::set_capture`] the view gets every key
//! press as [`AppEvent::Key`] (those keys too, and the softkeys) and every
//! hookswitch change as [`AppEvent::Hook`] while it is the top screen, and
//! the phone does nothing of its own with them.  `Cancel` three times in a
//! row still backgrounds the app.
//!
//! **Lamps.**  A lamp set with [`AppView::set_lamp`] shows that color in
//! place of the phone's while the view is the top screen.
//!
//! **Softkeys** are owned by widgets.  The bar shows the set of the nearest
//! widget, walking up from the focused one, that has softkeys set
//! ([`AppView::set_softkeys`] with `Some(widget)`), falling back to the
//! window's set (`None`).  A set may have any number of actions: with more
//! than four, the toolkit shows three and a "More" key that pages.  A press
//! sends [`AppEvent::Softkey`] with the action's `id`.
//!
//! **Full screen.**  [`AppView::set_full_screen`] gives the view the whole
//! main screen: no softkey bar is drawn, [`AppView::metrics`] reports the
//! screen's size, and the softkey keys go the way of every other key (to
//! a claimant, or nowhere).
//!
//! **Touch** (the touch skin, which has no keys).  The toolkit handles a
//! finger for the stock widgets: a tap on a list's row focuses the list,
//! selects the row ([`AppEvent::ListSelected`], if it was not selected),
//! and activates it ([`AppEvent::ListActivated`]); a drag scrolls a list
//! or a `Scroll`; a tap on an `Input` focuses it.  While an `Input` has
//! the focus, the on-screen keyboard is up in the layout of the input's
//! mode, over the softkey buttons and the lower part of the view (drawn
//! farther up, if the input would be under it); its done key is `Select`
//! (the input is submitted), and the back button, or a tap beside the
//! input, puts it away for the softkeys.  A widget (or the
//! window) that *takes touches* ([`AppView::set_touch_claim`]) gets
//! [`AppEvent::Touch`] instead, with places relative to itself: the
//! finger coming down ([`Touch::Down`], at once), and then the tap, or
//! the drag and its end, that the touch becomes.  A touch
//! goes to the topmost widget under the finger that takes touches or is a
//! `List`, an `Input`, or a `Scroll`; the window is under them all.  The
//! softkeys are buttons below the content area, and the toolkit draws a
//! back button over the top-left corner of the view
//! ([`Metrics::back_width`] wide, the title's height), which is the
//! `Cancel` key: claimed, it is the app's; unclaimed, it sends
//! [`AppEvent::CloseRequested`].  Lists have rows of the height a finger
//! needs, and a list scrolls to its selection only when the host selects
//! a row ([`AppView::list_select`]).
//!
//! **Lifecycle.**  [`AppEvent::Foreground`] / [`AppEvent::Background`]
//! report when the view becomes or stops being the top screen.  Changes to
//! a view that is not on screen are kept and shown later.
//!
//! **Batching.**  Each call locks, mutates, and marks the view dirty; the
//! UI repaints at most once per frame, so a burst of calls costs one
//! repaint.  [`AppView::batch`] holds the lock across several calls so the
//! UI never paints a half-built screen.  [`AppView::hold_changes`] keeps
//! the mark back without the lock, for a host whose calls come one by one
//! from an app's handler.

pub use opendeskphone_hal::Lamp;
use opendeskphone_state::{Key, LampColor, Model, Skin};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Handle of a widget within one [`AppView`].  Never reused; `0` is never
/// a valid handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetId(pub u32);

/// The stock widget set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    /// A container; draws its background (if set) and its children.
    Group,
    /// A label: one line, or wrapped lines with [`AppView::set_wrap`].
    Text,
    /// Rows of cells in columns with one selected row.
    List,
    /// A container that scrolls its children vertically and shows a
    /// scroll bar; it follows the focused child.
    Scroll,
    /// A one-line text entry.
    Input,
    /// A bitmap.
    Image,
    /// Free drawing: a list of [`DrawOp`]s, painted in order.
    Canvas,
}

/// A rectangle relative to the parent widget (or the app content area).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: i16, y: i16, w: u16, h: u16) -> Self {
        Rect { x, y, w, h }
    }
}

/// Sizes an app lays out with: the content area and the stock text metrics
/// of the skin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub skin: Skin,
    /// App content area.
    pub width: u16,
    pub height: u16,
    /// `Text.LINE_HEIGHT`: 15, 17, 25.
    pub line_height: u16,
    /// `Text.TITLE_HEIGHT`: 40 on color; the line height on gray.
    pub title_height: u16,
    /// `Text.TITLE_OFFSET`: 4 on color; 1 on gray.
    pub title_offset: u16,
    /// `Text.ROW_HEIGHT`: 32 on color; the line height on gray.
    pub row_height: u16,
    /// `Text.SCROLL_BAR_WIDTH`: 8 on color; 4 on gray.
    pub scroll_bar_width: u16,
    /// True for the color skin (theme colors and RGB are honored; gray
    /// skins map every color to one of four levels).
    pub color: bool,
    /// The phone has keys.  False on the touch skin: the view is used by
    /// touch alone.
    pub keys: bool,
    /// The width of the back button the toolkit draws over the top-left
    /// corner of the view on the touch skin (`title_height` high); 0 on
    /// the skins of phones with keys.
    pub back_width: u16,
}

impl Metrics {
    pub const fn for_model(model: Model) -> Metrics {
        let area = model.app_content_area();
        let skin = model.skin();
        let (line, title, off, row, sb, color, back) = match skin {
            Skin::SmallGray => (15, 15, 1, 15, 4, false, 0),
            Skin::LargeGray => (17, 17, 1, 17, 4, false, 0),
            Skin::Color => (25, 40, 4, 32, 8, true, 0),
            // (`Theme::back_w`, and the touch skin's other sizes.)
            Skin::Touch => (44, 96, 8, 96, 12, true, 96),
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
            back_width: back,
        }
    }
}

/// Theme color tokens (the stock names, minus the `color` suffix).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeColor {
    /// `bgcolor`
    Background,
    /// `fgcolor`
    Foreground,
    /// `titlebgcolor`
    TitleBackground,
    /// `titlefgcolor`
    TitleForeground,
    /// `selectbgcolor`
    SelectBackground,
    /// `selectfgcolor`
    SelectForeground,
    /// `tablfg1color`: secondary text.
    Secondary,
    /// `tablbdrcolor`: separators and borders.
    Border,
    /// `modalbgcolor`
    ModalBackground,
    /// Softkey bar background and label.
    SoftkeyBackground,
    SoftkeyForeground,
    /// Accent colors for status (good / warning / error).
    Good,
    Warning,
    Error,
}

/// A color.  Gray skins reduce everything to four levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// The widget kind's default for this role.
    #[default]
    Default,
    /// No fill (backgrounds only).
    Transparent,
    Theme(ThemeColor),
    Rgb(u8, u8, u8),
}

/// Font roles.  The faces and pixel sizes are the skin's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Font {
    #[default]
    Normal,
    Bold,
    /// Smaller secondary text.
    Small,
    /// The title font.
    Title,
    /// Large digits and names (dialer, clock).
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Width of a list column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnWidth {
    Px(u16),
    /// Percent of the list width (stock menus use 10% for the number).
    Percent(u8),
    /// Whatever is left, shared equally among `Rest` columns.
    Rest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Column {
    pub width: ColumnWidth,
    pub align: Align,
    pub font: Font,
}

impl Column {
    pub const fn px(w: u16) -> Column {
        Column { width: ColumnWidth::Px(w), align: Align::Left, font: Font::Normal }
    }
    pub const fn percent(p: u8) -> Column {
        Column { width: ColumnWidth::Percent(p), align: Align::Left, font: Font::Normal }
    }
    pub const fn rest() -> Column {
        Column { width: ColumnWidth::Rest, align: Align::Left, font: Font::Normal }
    }
    pub const fn align(mut self, a: Align) -> Column {
        self.align = a;
        self
    }
    pub const fn font(mut self, f: Font) -> Column {
        self.font = f;
        self
    }
}

/// One cell of a list row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Text(String),
    /// A built-in icon by name (see [`crate::icons::names`]); unknown names
    /// draw nothing.
    Icon(String),
    /// A picture (an app's own icon), drawn centered at its own size.
    Image(crate::image::Image),
}

impl Cell {
    pub fn text(s: impl Into<String>) -> Cell {
        Cell::Text(s.into())
    }
    pub fn icon(s: impl Into<String>) -> Cell {
        Cell::Icon(s.into())
    }
}

/// What an [`WidgetKind::Image`] shows.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ImageSource {
    #[default]
    None,
    /// A built-in icon by name.
    Icon(String),
    /// Raw pixels, row-major RGBA, `width * height * 4` bytes.  Gray skins
    /// convert to four levels by luminance.
    Rgba { width: u16, height: u16, pixels: Arc<[u8]> },
}

/// One drawing operation of a [`WidgetKind::Canvas`].  Coordinates are
/// relative to the canvas; nothing is drawn outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawOp {
    /// A filled rectangle.
    Fill { rect: Rect, color: Color },
    /// A one-color picture: one bit per pixel, rows from the top, each
    /// padded to whole bytes, the leftmost pixel in the highest bit.  Each
    /// set bit is drawn as a `scale` x `scale` square; clear bits are not
    /// drawn.  A picture with too few `bits` for its size draws nothing.
    Bitmap { x: i16, y: i16, width: u16, height: u16, scale: u8, color: Color, bits: Arc<[u8]> },
    /// One line of text in a font of the skin.  What its glyphs paint is
    /// centered in `rect`, both ways (not the line: a digit sits in the
    /// middle of a square), and clipped to it.
    Text { rect: Rect, font: Font, color: Color, text: String },
}

/// Text entry modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum InputMode {
    /// Multi-tap letters, as on stock (`2` -> a b c 2 A B C).
    #[default]
    Text,
    /// Digits, `*` enters `.`, `#` enters `#`... see `InputMode::Dial` for
    /// dial strings.  Here: digits only, `*` is a decimal point.
    Numeric,
    /// Dial string: digits, `*`, `#`.
    Dial,
    /// As `Text`, shown as dots except the character being entered.
    Password,
    /// As `Numeric`, shown as dots.
    Pin,
}

/// One softkey action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Softkey {
    /// Reported back in [`AppEvent::Softkey`].
    pub id: u32,
    pub label: String,
    /// Optional built-in icon shown instead of the label.
    pub icon: Option<String>,
}

impl Softkey {
    pub fn new(id: u32, label: impl Into<String>) -> Softkey {
        Softkey { id, label: label.into(), icon: None }
    }
}

/// A set of keys a widget claims.  See the module docs.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyClaims {
    pub keys: Vec<Key>,
    /// Claims all ten digits (`onkeynum`).
    pub digits: bool,
    /// Claims every key an app may see (`onkey`).
    pub all: bool,
}

impl KeyClaims {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn keys(keys: impl IntoIterator<Item = Key>) -> Self {
        KeyClaims { keys: keys.into_iter().collect(), digits: false, all: false }
    }
    pub fn claims(&self, key: Key) -> bool {
        self.all || (self.digits && matches!(key, Key::Digit(_))) || self.keys.contains(&key)
    }
}

/// What a finger did on a widget that takes touches
/// ([`AppView::set_touch_claim`]).  Places are in pixels from the widget's
/// top-left corner (the view's, for the window); a finger that left the
/// widget is at a place outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touch {
    /// The finger came down.  The first of every touch: a `Tap` follows,
    /// or `Drag`s and a `DragEnd`, or (a touch that was canceled) nothing.
    Down { x: i16, y: i16 },
    /// Up again in the place it came down, or still there after
    /// [`crate::touch::LONG_PRESS_MS`].
    Tap { x: i16, y: i16 },
    /// The finger, having come down at (`x0`, `y0`), is now at (`x`,
    /// `y`).  Sent for every movement.
    Drag { x0: i16, y0: i16, x: i16, y: i16 },
    /// The finger of a drag was lifted at (`x`, `y`).
    DragEnd { x0: i16, y0: i16, x: i16, y: i16 },
}

/// What the toolkit tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// The view became the top screen.
    Foreground,
    /// The view stopped being the top screen (another screen covers it, or
    /// the user pressed `Line(1)`).
    Background,
    /// A claimed key was pressed.  `widget` is the claimant (`None`: the
    /// window).
    Key { widget: Option<WidgetId>, key: Key },
    /// The handset was lifted or put back (under capture).
    Hook { off_hook: bool },
    /// A softkey action was pressed.
    Softkey { id: u32 },
    /// The list selection moved (by the user).
    ListSelected { widget: WidgetId, row: u32 },
    /// Select (or a digit shortcut) on a list row.
    ListActivated { widget: WidgetId, row: u32 },
    /// The input's text changed (by the user).
    InputChanged { widget: WidgetId, value: String },
    /// Select on an input.
    InputSubmitted { widget: WidgetId, value: String },
    /// Focus moved (by the user) to this widget.
    Focused { widget: WidgetId },
    /// A finger on a widget that takes touches (`None`: the window).
    Touch { widget: Option<WidgetId>, touch: Touch },
    /// Unclaimed `Cancel`.
    CloseRequested,
}

// ------------------------------------------------------------------ tree

#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub id: WidgetId,
    pub kind: WidgetKind,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    pub rect: Rect,
    pub visible: bool,
    pub fg: Color,
    pub bg: Color,
    pub border: bool,
    pub claims: KeyClaims,
    /// The widget takes touches.
    pub touch: bool,
    pub softkeys: Option<Vec<Softkey>>,
    // Text / Input
    pub text: String,
    pub font: Font,
    pub align: Align,
    pub valign: VAlign,
    pub wrap: bool,
    // List
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    pub selected: u32,
    pub top_row: u32,
    /// Touch: how far a finger has dragged the list, short of a row.
    pub drag: i32,
    pub row_height: u16,
    pub shortcuts: bool,
    pub wrap_selection: bool,
    // Scroll
    pub scroll_y: u16,
    pub content_h: u16,
    // Input
    pub mode: InputMode,
    pub max_len: u16,
    pub cursor: u16,
    pub placeholder: String,
    // Image
    pub image: ImageSource,
    // Canvas (shared: the renderer works on a copy of the node)
    pub ops: Arc<[DrawOp]>,
}

impl Node {
    fn new(id: WidgetId, kind: WidgetKind, parent: Option<WidgetId>, rect: Rect) -> Node {
        Node {
            id,
            kind,
            parent,
            children: Vec::new(),
            rect,
            visible: true,
            fg: Color::Default,
            bg: Color::Default,
            border: false,
            claims: KeyClaims::default(),
            touch: false,
            softkeys: None,
            text: String::new(),
            font: Font::Normal,
            align: Align::Left,
            valign: VAlign::Middle,
            wrap: false,
            columns: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            top_row: 0,
            drag: 0,
            row_height: 0,
            shortcuts: false,
            wrap_selection: true,
            scroll_y: 0,
            content_h: 0,
            mode: InputMode::Text,
            max_len: 255,
            cursor: 0,
            placeholder: String::new(),
            image: ImageSource::None,
            ops: Arc::new([]),
        }
    }
}

/// The widget tree.  Obtained with [`AppView::batch`]; every [`AppView`]
/// mutator is also available here.
pub struct Tree {
    pub(crate) model: Model,
    pub(crate) metrics: Metrics,
    pub(crate) nodes: Vec<Node>,
    pub(crate) roots: Vec<WidgetId>,
    pub(crate) next_id: u32,
    pub(crate) focus: Option<WidgetId>,
    pub(crate) window_softkeys: Vec<Softkey>,
    pub(crate) window_claims: KeyClaims,
    pub(crate) softkey_page: u8,
    pub(crate) background: Color,
    /// The view has the whole screen; there is no softkey bar.
    pub(crate) full_screen: bool,
    /// The view takes every key and hookswitch change.
    pub(crate) capture: bool,
    /// Presses of `Cancel` in a row under capture.
    pub(crate) cancels: u8,
    /// The lamps the view sets, each with its color.
    pub(crate) lamps: Vec<(Lamp, LampColor)>,
    /// Multi-tap state of the focused input (see `crate::input`).
    pub(crate) multitap: crate::multitap::MultiTap,
    /// The window takes the touches no widget takes.
    pub(crate) window_touch: bool,
    /// Touch: the drag under way (`crate::appview_impl`).
    pub(crate) dragging: Option<crate::appview_impl::Dragging>,
    /// Touch: the focus moved, and a `Scroll` has yet to bring the
    /// focused widget into view (on the other skins it always does).
    pub(crate) follow_focus: bool,
}

impl Tree {
    pub(crate) fn new(model: Model) -> Tree {
        Tree {
            model,
            metrics: Metrics::for_model(model),
            nodes: Vec::new(),
            roots: Vec::new(),
            next_id: 1,
            focus: None,
            window_softkeys: Vec::new(),
            window_claims: KeyClaims::default(),
            softkey_page: 0,
            background: Color::Default,
            full_screen: false,
            capture: false,
            cancels: 0,
            lamps: Vec::new(),
            multitap: Default::default(),
            window_touch: false,
            dragging: None,
            follow_focus: false,
        }
    }

    pub(crate) fn node(&self, id: WidgetId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub(crate) fn node_mut(&mut self, id: WidgetId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// Creates a widget.  A dead `parent` makes a top-level widget.
    pub fn create(&mut self, kind: WidgetKind, parent: Option<WidgetId>, rect: Rect) -> WidgetId {
        let id = WidgetId(self.next_id);
        self.next_id += 1;
        let parent = parent.filter(|p| self.node(*p).is_some());
        let mut node = Node::new(id, kind, parent, rect);
        if kind == WidgetKind::List {
            node.row_height = self.metrics.row_height;
        }
        self.nodes.push(node);
        match parent {
            Some(p) => self.node_mut(p).unwrap().children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    /// Destroys a widget and its children.
    pub fn destroy(&mut self, id: WidgetId) {
        let Some(node) = self.node(id) else { return };
        let parent = node.parent;
        let mut doomed = vec![id];
        let mut i = 0;
        while i < doomed.len() {
            if let Some(n) = self.node(doomed[i]) {
                doomed.extend(n.children.iter().copied());
            }
            i += 1;
        }
        self.nodes.retain(|n| !doomed.contains(&n.id));
        match parent {
            Some(p) => {
                if let Some(pn) = self.node_mut(p) {
                    pn.children.retain(|c| *c != id);
                }
            }
            None => self.roots.retain(|c| *c != id),
        }
        if self.focus.is_some_and(|f| doomed.contains(&f)) {
            self.focus = None;
            self.softkey_page = 0;
        }
    }

    /// Destroys the children of a widget, keeping the widget.
    pub fn clear_children(&mut self, id: WidgetId) {
        let children = self.node(id).map(|n| n.children.clone()).unwrap_or_default();
        for c in children {
            self.destroy(c);
        }
    }

    /// Destroys every widget and clears the window softkeys, key claims,
    /// and touch claim.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        self.focus = None;
        self.window_softkeys.clear();
        self.window_claims = KeyClaims::default();
        self.window_touch = false;
        self.softkey_page = 0;
    }

    pub fn exists(&self, id: WidgetId) -> bool {
        self.node(id).is_some()
    }

    pub fn set_rect(&mut self, id: WidgetId, rect: Rect) {
        if let Some(n) = self.node_mut(id) {
            n.rect = rect;
        }
    }
    /// Shows or hides a widget and its children.  Hiding the focused
    /// widget, or one the focused widget is in, leaves nothing focused: a
    /// widget that is not shown takes no keys.
    pub fn set_visible(&mut self, id: WidgetId, visible: bool) {
        let Some(n) = self.node_mut(id) else { return };
        n.visible = visible;
        if visible {
            return;
        }
        let mut cur = self.focus;
        while let Some(c) = cur {
            if c == id {
                self.focus(None);
                break;
            }
            cur = self.node(c).and_then(|n| n.parent);
        }
    }
    /// Foreground (text) and background colors.
    pub fn set_colors(&mut self, id: WidgetId, fg: Color, bg: Color) {
        if let Some(n) = self.node_mut(id) {
            n.fg = fg;
            n.bg = bg;
        }
    }
    /// Draws a one-pixel border in the theme's border color.
    pub fn set_border(&mut self, id: WidgetId, on: bool) {
        if let Some(n) = self.node_mut(id) {
            n.border = on;
        }
    }
    /// Gives the view the whole main screen (no softkey bar), or the app
    /// content area again.  [`Tree::metrics`] follows.
    pub fn set_full_screen(&mut self, on: bool) {
        self.full_screen = on;
        self.metrics.height = if on { self.model.main_screen().height } else { self.model.app_content_area().height };
    }
    pub fn full_screen(&self) -> bool {
        self.full_screen
    }
    /// Gives the view every key and hookswitch change while it is the top
    /// screen, or the usual key dispatch again.
    pub fn set_capture(&mut self, on: bool) {
        self.capture = on;
        self.cancels = 0;
    }
    pub fn capture(&self) -> bool {
        self.capture
    }
    /// Shows a color on a lamp, in place of the phone's, while the view is
    /// the top screen.
    pub fn set_lamp(&mut self, lamp: Lamp, color: LampColor) {
        match self.lamps.iter_mut().find(|(l, _)| *l == lamp) {
            Some(e) => e.1 = color,
            None => self.lamps.push((lamp, color)),
        }
    }
    /// Gives every lamp back to the phone.
    pub fn release_lamps(&mut self) {
        self.lamps.clear();
    }
    /// The lamps the view sets.
    pub fn lamps(&self) -> Vec<(Lamp, LampColor)> {
        self.lamps.clone()
    }
    /// Background of the whole content area.
    pub fn set_background(&mut self, color: Color) {
        self.background = color;
    }

    /// Text of a `Text`, or the value of an `Input` (cursor goes to the
    /// end; no `InputChanged` is sent for programmatic changes).
    pub fn set_text(&mut self, id: WidgetId, text: &str) {
        if let Some(n) = self.node_mut(id) {
            if n.text != text {
                n.text = text.to_string();
            }
            n.cursor = n.text.chars().count() as u16;
        }
        if self.focus == Some(id) {
            self.multitap.reset();
        }
    }
    /// Text of a `Text` or value of an `Input`.
    pub fn text(&self, id: WidgetId) -> Option<String> {
        self.node(id).map(|n| n.text.clone())
    }
    pub fn set_font(&mut self, id: WidgetId, font: Font) {
        if let Some(n) = self.node_mut(id) {
            n.font = font;
        }
    }
    pub fn set_align(&mut self, id: WidgetId, align: Align, valign: VAlign) {
        if let Some(n) = self.node_mut(id) {
            n.align = align;
            n.valign = valign;
        }
    }
    /// Word-wraps a `Text` within its rectangle.
    pub fn set_wrap(&mut self, id: WidgetId, wrap: bool) {
        if let Some(n) = self.node_mut(id) {
            n.wrap = wrap;
        }
    }

    pub fn list_set_columns(&mut self, id: WidgetId, columns: &[Column]) {
        if let Some(n) = self.node_mut(id) {
            n.columns = columns.to_vec();
        }
    }
    /// Replaces all rows.  The selection is kept if still in range.
    pub fn list_set_rows(&mut self, id: WidgetId, rows: Vec<Vec<Cell>>) {
        if let Some(n) = self.node_mut(id) {
            n.rows = rows;
            let max = n.rows.len().saturating_sub(1) as u32;
            n.selected = n.selected.min(max);
            n.top_row = n.top_row.min(max);
        }
    }
    /// Replaces one row (appends if `row == len`).
    pub fn list_set_row(&mut self, id: WidgetId, row: u32, cells: Vec<Cell>) {
        if let Some(n) = self.node_mut(id) {
            let r = row as usize;
            if r < n.rows.len() {
                n.rows[r] = cells;
            } else if r == n.rows.len() {
                n.rows.push(cells);
            }
        }
    }
    pub fn list_remove_row(&mut self, id: WidgetId, row: u32) {
        if let Some(n) = self.node_mut(id)
            && (row as usize) < n.rows.len() {
                n.rows.remove(row as usize);
                let max = n.rows.len().saturating_sub(1) as u32;
                n.selected = n.selected.min(max);
                n.top_row = n.top_row.min(max);
            }
    }
    pub fn list_len(&self, id: WidgetId) -> Option<u32> {
        self.node(id).map(|n| n.rows.len() as u32)
    }
    /// Selects a row.  (On the touch skin, where a list does not follow
    /// its selection as it is drawn, this scrolls the row into view.)
    pub fn list_select(&mut self, id: WidgetId, row: u32) {
        let touch = crate::skin::is_touch(self.model);
        if let Some(n) = self.node_mut(id) {
            n.selected = row.min(n.rows.len().saturating_sub(1) as u32);
            if touch {
                let visible = (n.rect.h / n.row_height.max(1)).max(1) as u32;
                if n.selected < n.top_row {
                    n.top_row = n.selected;
                    n.drag = 0;
                } else if n.selected >= n.top_row + visible {
                    n.top_row = n.selected + 1 - visible;
                    n.drag = 0;
                }
            }
        }
    }
    pub fn list_selected(&self, id: WidgetId) -> Option<u32> {
        self.node(id).filter(|n| !n.rows.is_empty()).map(|n| n.selected)
    }
    /// Row height in pixels (default: the skin's row height).
    pub fn list_set_row_height(&mut self, id: WidgetId, h: u16) {
        if let Some(n) = self.node_mut(id) {
            n.row_height = h.max(1);
        }
    }
    /// Digits 1-9, 0 activate rows 0..9 (stock menu helper).
    pub fn list_set_shortcuts(&mut self, id: WidgetId, on: bool) {
        if let Some(n) = self.node_mut(id) {
            n.shortcuts = on;
        }
    }
    /// Whether Up on the first row wraps to the last and vice versa
    /// (default on).
    pub fn list_set_wrap(&mut self, id: WidgetId, on: bool) {
        if let Some(n) = self.node_mut(id) {
            n.wrap_selection = on;
        }
    }

    /// Scroll position in pixels.
    pub fn scroll_to(&mut self, id: WidgetId, y: u16) {
        if let Some(n) = self.node_mut(id) {
            n.scroll_y = y;
        }
    }

    pub fn input_set_mode(&mut self, id: WidgetId, mode: InputMode) {
        if let Some(n) = self.node_mut(id) {
            n.mode = mode;
        }
    }
    pub fn input_set_max_len(&mut self, id: WidgetId, max: u16) {
        if let Some(n) = self.node_mut(id) {
            n.max_len = max;
        }
    }
    /// Dimmed text shown while the input is empty.
    pub fn input_set_placeholder(&mut self, id: WidgetId, text: &str) {
        if let Some(n) = self.node_mut(id) {
            n.placeholder = text.to_string();
        }
    }

    pub fn set_image(&mut self, id: WidgetId, image: ImageSource) {
        if let Some(n) = self.node_mut(id) {
            n.image = image;
        }
    }

    /// Replaces what a `Canvas` shows.  Returns whether `id` is a canvas;
    /// any other widget is left as it was.
    pub fn canvas_set_ops(&mut self, id: WidgetId, ops: Vec<DrawOp>) -> bool {
        match self.node_mut(id).filter(|n| n.kind == WidgetKind::Canvas) {
            Some(n) => {
                n.ops = ops.into();
                true
            }
            None => false,
        }
    }

    /// Gives a widget the keyboard (`None`: nothing focused).
    pub fn focus(&mut self, id: Option<WidgetId>) {
        let id = id.filter(|i| self.node(*i).is_some());
        if self.focus != id {
            self.focus = id;
            self.softkey_page = 0;
            self.multitap.reset();
            self.follow_focus = true;
        }
    }
    pub fn focused(&self) -> Option<WidgetId> {
        self.focus
    }

    /// Sets the softkeys owned by a widget (`None`: the window).  An empty
    /// list on a widget removes its set, so lookup continues upward.
    pub fn set_softkeys(&mut self, owner: Option<WidgetId>, keys: Vec<Softkey>) {
        match owner {
            None => self.window_softkeys = keys,
            Some(id) => {
                if let Some(n) = self.node_mut(id) {
                    n.softkeys = if keys.is_empty() { None } else { Some(keys) };
                }
            }
        }
        self.softkey_page = 0;
    }

    /// Keys a widget claims (see the module docs).
    pub fn set_key_claims(&mut self, id: WidgetId, claims: KeyClaims) {
        if let Some(n) = self.node_mut(id) {
            n.claims = claims;
        }
    }
    /// Keys the window claims when no widget on the focus path does.
    pub fn set_window_key_claims(&mut self, claims: KeyClaims) {
        self.window_claims = claims;
    }

    /// Whether a widget (`None`: the window) takes touches: what a finger
    /// does on it is sent as [`AppEvent::Touch`] in place of the built-in
    /// behavior (see the module docs).
    pub fn set_touch_claim(&mut self, owner: Option<WidgetId>, on: bool) {
        match owner {
            None => self.window_touch = on,
            Some(id) => {
                if let Some(n) = self.node_mut(id) {
                    n.touch = on;
                }
            }
        }
    }

    /// The focused `Input` and its mode, on the touch skin: the widget an
    /// on-screen keyboard types into ([`AppView::type_text`],
    /// [`AppView::backspace`]).
    pub fn keyboard_input(&self) -> Option<(WidgetId, InputMode)> {
        if !crate::skin::is_touch(self.model) {
            return None;
        }
        let n = self.node(self.focus?)?;
        (n.kind == WidgetKind::Input && n.visible).then_some((n.id, n.mode))
    }
}

// ------------------------------------------------------------------ view

type Waker = Box<dyn Fn() + Send + Sync>;

/// A theme that can be changed under those who use it.
pub(crate) type ThemeCell = Arc<Mutex<&'static crate::skin::Theme>>;

pub(crate) struct Inner {
    pub name: String,
    pub model: Model,
    /// The theme whose colors `theme_color` answers with.  A view made
    /// by a UI ([`crate::UiHandle::new_view`]) has that UI's theme, as it
    /// is at any time; a view made without one has the model's skin
    /// until it is on a phone's screen.
    theme: ThemeCell,
    pub tree: Mutex<Tree>,
    pub dirty: AtomicBool,
    /// Changes are held back ([`AppView::hold_changes`]), and whether
    /// there are any.
    held: AtomicBool,
    held_dirty: AtomicBool,
    queue: Mutex<VecDeque<AppEvent>>,
    cond: Condvar,
    closed: AtomicBool,
    event_waker: Mutex<Option<Waker>>,
    ui_waker: Mutex<Option<Waker>>,
}

/// One app's screen.  See the module docs.
#[derive(Clone)]
pub struct AppView {
    pub(crate) inner: Arc<Inner>,
}

/// A locked [`Tree`]; the view is marked dirty when this is dropped.
pub struct Batch<'a> {
    tree: MutexGuard<'a, Tree>,
    view: &'a AppView,
}

impl std::ops::Deref for Batch<'_> {
    type Target = Tree;
    fn deref(&self) -> &Tree {
        &self.tree
    }
}
impl std::ops::DerefMut for Batch<'_> {
    fn deref_mut(&mut self) -> &mut Tree {
        &mut self.tree
    }
}
impl Drop for Batch<'_> {
    fn drop(&mut self) {
        self.view.mark_dirty();
    }
}

macro_rules! forward {
    ($( $(#[$m:meta])* fn $name:ident(&self $(, $arg:ident : $ty:ty)*) $(-> $ret:ty)?; )*) => {
        $( $(#[$m])* pub fn $name(&self $(, $arg: $ty)*) $(-> $ret)? {
            self.batch().$name($($arg),*)
        } )*
    };
}

impl AppView {
    /// Creates the view for app `name` (the manifest name) and its event
    /// receiver.
    pub fn new(name: &str, model: Model) -> (AppView, AppEvents) {
        AppView::with_theme(name, model, Arc::new(Mutex::new(crate::skin::Theme::for_model(model))))
    }

    /// A view whose theme is the one in `theme`, whatever is put there.
    pub(crate) fn with_theme(name: &str, model: Model, theme: ThemeCell) -> (AppView, AppEvents) {
        let inner = Arc::new(Inner {
            name: name.to_string(),
            model,
            theme,
            tree: Mutex::new(Tree::new(model)),
            dirty: AtomicBool::new(true),
            held: AtomicBool::new(false),
            held_dirty: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            cond: Condvar::new(),
            closed: AtomicBool::new(false),
            event_waker: Mutex::new(None),
            ui_waker: Mutex::new(None),
        });
        (AppView { inner: inner.clone() }, AppEvents { inner })
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }
    pub fn model(&self) -> Model {
        self.inner.model
    }
    /// The sizes the app lays out with; the height is the whole screen's
    /// while the view is full-screen.
    pub fn metrics(&self) -> Metrics {
        self.batch_quiet().metrics
    }

    /// Locks the tree for several changes painted together.
    pub fn batch(&self) -> Batch<'_> {
        Batch { tree: self.inner.tree.lock().unwrap_or_else(|e| e.into_inner()), view: self }
    }

    forward! {
        /// See [`Tree::create`].
        fn create(&self, kind: WidgetKind, parent: Option<WidgetId>, rect: Rect) -> WidgetId;
        fn destroy(&self, id: WidgetId);
        fn clear_children(&self, id: WidgetId);
        fn clear(&self);
        fn exists(&self, id: WidgetId) -> bool;
        fn set_rect(&self, id: WidgetId, rect: Rect);
        fn set_visible(&self, id: WidgetId, visible: bool);
        fn set_colors(&self, id: WidgetId, fg: Color, bg: Color);
        fn set_border(&self, id: WidgetId, on: bool);
        fn set_full_screen(&self, on: bool);
        fn set_capture(&self, on: bool);
        fn set_lamp(&self, lamp: Lamp, color: LampColor);
        fn release_lamps(&self);
        fn set_background(&self, color: Color);
        fn set_text(&self, id: WidgetId, text: &str);
        fn text(&self, id: WidgetId) -> Option<String>;
        fn set_font(&self, id: WidgetId, font: Font);
        fn set_align(&self, id: WidgetId, align: Align, valign: VAlign);
        fn set_wrap(&self, id: WidgetId, wrap: bool);
        fn list_set_columns(&self, id: WidgetId, columns: &[Column]);
        fn list_set_rows(&self, id: WidgetId, rows: Vec<Vec<Cell>>);
        fn list_set_row(&self, id: WidgetId, row: u32, cells: Vec<Cell>);
        fn list_remove_row(&self, id: WidgetId, row: u32);
        fn list_len(&self, id: WidgetId) -> Option<u32>;
        fn list_select(&self, id: WidgetId, row: u32);
        fn list_selected(&self, id: WidgetId) -> Option<u32>;
        fn list_set_row_height(&self, id: WidgetId, h: u16);
        fn list_set_shortcuts(&self, id: WidgetId, on: bool);
        fn list_set_wrap(&self, id: WidgetId, on: bool);
        fn scroll_to(&self, id: WidgetId, y: u16);
        fn input_set_mode(&self, id: WidgetId, mode: InputMode);
        fn input_set_max_len(&self, id: WidgetId, max: u16);
        fn input_set_placeholder(&self, id: WidgetId, text: &str);
        fn set_image(&self, id: WidgetId, image: ImageSource);
        fn canvas_set_ops(&self, id: WidgetId, ops: Vec<DrawOp>) -> bool;
        fn focus(&self, id: Option<WidgetId>);
        fn focused(&self) -> Option<WidgetId>;
        fn set_softkeys(&self, owner: Option<WidgetId>, keys: Vec<Softkey>);
        fn set_key_claims(&self, id: WidgetId, claims: KeyClaims);
        fn set_window_key_claims(&self, claims: KeyClaims);
        fn set_touch_claim(&self, owner: Option<WidgetId>, on: bool);
    }

    /// See [`Tree::keyboard_input`].
    pub fn keyboard_input(&self) -> Option<(WidgetId, InputMode)> {
        self.batch_quiet().keyboard_input()
    }

    /// The width and height of what `text` paints in a font of this skin:
    /// the pixels of its glyphs, as [`DrawOp::Text`] centers them.
    pub fn text_ink(&self, font: Font, text: &str) -> (u16, u16) {
        let (_, _, w, h) = crate::skin::Theme::for_model(self.inner.model).font(font).ink(text);
        (w.clamp(0, u16::MAX as i32) as u16, h.clamp(0, u16::MAX as i32) as u16)
    }

    /// Pixel width of `text` in a font of this skin, for layout.
    pub fn text_width(&self, font: Font, text: &str) -> u16 {
        crate::skin::Theme::for_model(self.inner.model).text_width(font, text)
    }

    /// A color of the theme in use (this model's skin, in the palette of
    /// the selected wallpaper on a color model), as red, green, and blue.
    /// The palette can change while the app runs; an app in the foreground
    /// then gets [`AppEvent::Foreground`] again.
    pub fn theme_color(&self, color: ThemeColor) -> (u8, u8, u8) {
        let c = self.inner.theme.lock().unwrap_or_else(|p| p.into_inner()).theme_color(color);
        ((c >> 16) as u8, (c >> 8) as u8, c as u8)
    }

    pub(crate) fn set_theme(&self, th: &'static crate::skin::Theme) {
        *self.inner.theme.lock().unwrap_or_else(|p| p.into_inner()) = th;
    }

    /// With `true`, the changes made to the view from now on do not mark
    /// the view as changed; with `false`, they mark it once, if there are
    /// any.  A host holds the changes while the app's handler runs, so
    /// that what one handler changes is painted together.  The tree is
    /// not locked meanwhile: a repaint for another reason (the view comes
    /// on screen, the theme changes) shows the tree as it is then.
    pub fn hold_changes(&self, on: bool) {
        self.inner.held.store(on, Ordering::Release);
        if !on && self.inner.held_dirty.swap(false, Ordering::AcqRel) {
            self.mark_dirty();
        }
    }

    pub(crate) fn mark_dirty(&self) {
        if self.inner.held.load(Ordering::Acquire) {
            self.inner.held_dirty.store(true, Ordering::Release);
            // Released between the two: the mark is not lost.
            if self.inner.held.load(Ordering::Acquire) || !self.inner.held_dirty.swap(false, Ordering::AcqRel) {
                return;
            }
        }
        self.inner.dirty.store(true, Ordering::Release);
        if let Some(w) = &*self.inner.ui_waker.lock().unwrap_or_else(|e| e.into_inner()) {
            w();
        }
    }

    pub(crate) fn take_dirty(&self) -> bool {
        self.inner.dirty.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn set_ui_waker(&self, w: Option<Waker>) {
        *self.inner.ui_waker.lock().unwrap_or_else(|e| e.into_inner()) = w;
    }

    /// Queues an event for the app.  Used by the toolkit; also public so a
    /// host can loop its own events through the same queue.
    pub fn push_event(&self, ev: AppEvent) {
        if self.inner.closed.load(Ordering::Acquire) {
            return;
        }
        self.inner.queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(ev);
        self.inner.cond.notify_all();
        if let Some(w) = &*self.inner.event_waker.lock().unwrap_or_else(|e| e.into_inner()) {
            w();
        }
    }

    /// Feeds one key press to the view as the UI would when the view is the
    /// top screen.  Returns whether the view consumed it.  For tests and
    /// hosts without a running [`crate::Ui`].
    pub fn dispatch_key(&self, key: Key) -> bool {
        crate::appview_impl::dispatch_key(self, key)
    }

    /// Feeds a tap at (`x`, `y`) of the view (pixels from the top-left
    /// corner of the content area) as the UI would on the touch skin when
    /// the view is the top screen.  For tests and hosts without a running
    /// [`crate::Ui`].
    pub fn dispatch_tap(&self, x: i32, y: i32) {
        crate::appview_impl::tap(self, x, y);
    }

    /// Feeds a finger that came down at (`x`, `y`) of the view, as
    /// [`AppView::dispatch_tap`]: the UI does before the tap or the drag
    /// the touch becomes.
    pub fn dispatch_down(&self, x: i32, y: i32) {
        crate::appview_impl::down(self, x, y);
    }

    /// Feeds one movement of a drag: the finger came down at (`x0`, `y0`)
    /// of the view and has moved `dx` and `dy` pixels more.  See
    /// [`AppView::dispatch_tap`].
    pub fn dispatch_drag(&self, x0: i32, y0: i32, dx: i32, dy: i32) {
        crate::appview_impl::drag(self, x0, y0, dx, dy);
    }

    /// The finger of the drag was lifted.
    pub fn dispatch_drag_end(&self) {
        crate::appview_impl::drag_end(self);
    }

    /// Types `text` at the cursor of the focused `Input`, as far as its
    /// mode and its length allow, and sends [`AppEvent::InputChanged`] if
    /// that changed it.  For an on-screen keyboard.  Returns whether an
    /// input has the focus.
    pub fn type_text(&self, text: &str) -> bool {
        crate::appview_impl::type_text(self, text)
    }

    /// Deletes the character before the cursor of the focused `Input`
    /// (as its "Delete" softkey does).  Returns whether an input has the
    /// focus.
    pub fn backspace(&self) -> bool {
        crate::appview_impl::backspace(self)
    }
}

/// The app's end of the event queue.  Dropping it closes the queue.
pub struct AppEvents {
    inner: Arc<Inner>,
}

impl AppEvents {
    /// Next event without waiting.
    pub fn try_recv(&self) -> Option<AppEvent> {
        self.inner.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }
    /// Waits for the next event.
    pub fn recv(&self) -> Option<AppEvent> {
        let mut q = self.inner.queue.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(ev) = q.pop_front() {
                return Some(ev);
            }
            q = self.inner.cond.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }
    /// Waits up to `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<AppEvent> {
        let q = self.inner.queue.lock().unwrap_or_else(|e| e.into_inner());
        let (mut q, _) = self
            .inner
            .cond
            .wait_timeout_while(q, timeout, |q| q.is_empty())
            .unwrap_or_else(|e| e.into_inner());
        q.pop_front()
    }
    /// Calls `f` (from the UI thread; keep it cheap, e.g., wake a task or
    /// write to a pipe) whenever an event is queued.
    pub fn set_waker(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.event_waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }
}

impl Drop for AppEvents {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Release);
    }
}

impl AppView {
    /// Locks the tree without marking the view dirty (rendering).
    pub(crate) fn batch_quiet(&self) -> MutexGuard<'_, Tree> {
        self.inner.tree.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.inner.dirty.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_a_widget_drops_the_focus_inside() {
        let (view, _events) = AppView::new("demo", Model::D60);
        let group = view.create(WidgetKind::Group, None, Rect::new(0, 0, 200, 100));
        let inner = view.create(WidgetKind::Group, Some(group), Rect::new(0, 0, 200, 100));
        let list = view.create(WidgetKind::List, Some(inner), Rect::new(0, 0, 200, 50));
        let other = view.create(WidgetKind::Input, None, Rect::new(0, 100, 200, 30));

        // The focused widget.
        view.focus(Some(list));
        view.set_visible(list, false);
        assert_eq!(view.focused(), None);
        // Showing the widget again does not bring the focus back.
        view.set_visible(list, true);
        assert_eq!(view.focused(), None);

        // An ancestor of the focused widget, however far up.
        view.focus(Some(list));
        view.set_visible(group, false);
        assert_eq!(view.focused(), None);
        view.set_visible(group, true);

        // A widget the focused one is not in, a widget that is shown, and
        // a handle that names nothing leave the focus alone.
        view.focus(Some(list));
        view.set_visible(other, false);
        view.set_visible(group, true);
        view.set_visible(WidgetId(9999), false);
        assert_eq!(view.focused(), Some(list));
    }

    #[test]
    fn held_changes_are_marked_once_when_released() {
        let (view, _events) = AppView::new("demo", Model::D60);
        let text = view.create(WidgetKind::Text, None, Rect::new(0, 0, 200, 20));
        assert!(view.take_dirty());
        let marks = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = marks.clone();
        view.set_ui_waker(Some(Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        })));

        view.hold_changes(true);
        view.set_text(text, "one");
        view.set_text(text, "two");
        assert!(!view.is_dirty(), "nothing is marked while the changes are held");
        assert_eq!(marks.load(Ordering::SeqCst), 0);
        // The tree is not locked meanwhile, and has what was set.
        assert_eq!(view.text(text).as_deref(), Some("two"));
        view.hold_changes(false);
        assert!(view.take_dirty());
        assert_eq!(marks.load(Ordering::SeqCst), 1);

        // A handler that changed nothing marks nothing.
        view.hold_changes(true);
        view.hold_changes(false);
        assert!(!view.is_dirty());
        // Without a hold, every change marks.
        view.set_text(text, "three");
        assert!(view.take_dirty());
    }
}
