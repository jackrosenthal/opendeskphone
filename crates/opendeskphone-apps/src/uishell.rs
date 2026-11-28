//! [`Shell`] and [`AppWindow`] over the phone UI toolkit (`opendeskphone-ui`'s
//! app view).  Enabled by the `ui` feature.
//!
//! ```ignore
//! let shell = UiShell::new(ui_handle.clone(), store.model());
//! let apps = AppManager::new(ManagerConfig::phone(), store, Box::new(shell), services)?;
//! ```

use crate::view::*;
use opendeskphone_state::{Key, Model};
use opendeskphone_ui::appview as tk;
use opendeskphone_ui::{AppEvent, AppEvents, AppView, UiHandle};
use std::collections::HashMap;
use std::sync::Arc;

pub struct UiShell {
    ui: UiHandle,
    model: Model,
    views: HashMap<String, AppView>,
}

impl UiShell {
    pub fn new(ui: UiHandle, model: Model) -> UiShell {
        UiShell {
            ui,
            model,
            views: HashMap::new(),
        }
    }
}

struct UiWindow {
    view: AppView,
    events: AppEvents,
    /// The app's pictures, by name, no larger than the window.
    images: HashMap<String, opendeskphone_ui::image::Image>,
}

fn id(w: u32) -> tk::WidgetId {
    tk::WidgetId(w)
}

fn rect(r: Rect) -> tk::Rect {
    tk::Rect::new(r.x, r.y, r.width, r.height)
}

fn tk_font(font: Font) -> tk::Font {
    match font {
        Font::Small => tk::Font::Small,
        Font::Normal => tk::Font::Normal,
        Font::Large => tk::Font::Large,
        Font::Bold => tk::Font::Bold,
        Font::Title => tk::Font::Title,
    }
}

impl AppWindow for UiWindow {
    fn metrics(&self) -> Metrics {
        let m = self.view.metrics();
        Metrics {
            skin: match m.skin {
                opendeskphone_state::Skin::SmallGray => Skin::SmallGray,
                opendeskphone_state::Skin::LargeGray => Skin::LargeGray,
                opendeskphone_state::Skin::Color => Skin::Color,
                opendeskphone_state::Skin::Touch => Skin::Touch,
            },
            width: m.width,
            height: m.height,
            line_height: m.line_height,
            title_height: m.title_height,
            title_offset: m.title_offset,
            row_height: m.row_height,
            scroll_bar_width: m.scroll_bar_width,
            color: m.color,
            keys: m.keys,
            back_width: m.back_width,
        }
    }
    fn create(&mut self, kind: WidgetKind, parent: Option<u32>, bounds: Rect) -> u32 {
        let kind = match kind {
            WidgetKind::Group => tk::WidgetKind::Group,
            WidgetKind::Text => tk::WidgetKind::Text,
            WidgetKind::TextList => tk::WidgetKind::List,
            WidgetKind::Scroll => tk::WidgetKind::Scroll,
            WidgetKind::Input => tk::WidgetKind::Input,
            WidgetKind::Image => tk::WidgetKind::Image,
        };
        self.view.create(kind, parent.map(id), rect(bounds)).0
    }
    fn destroy(&mut self, w: u32) {
        self.view.destroy(id(w));
    }
    fn clear(&mut self, w: Option<u32>) {
        match w {
            Some(w) => self.view.clear_children(id(w)),
            None => {
                let mut b = self.view.batch();
                b.clear();
                b.set_softkeys(None, Vec::new());
                b.set_window_key_claims(tk::KeyClaims::none());
            }
        }
    }
    fn set_bounds(&mut self, w: u32, bounds: Rect) {
        self.view.set_rect(id(w), rect(bounds));
    }
    fn set_visible(&mut self, w: u32, visible: bool) {
        self.view.set_visible(id(w), visible);
    }
    fn set_focus(&mut self, w: Option<u32>) {
        self.view.focus(w.map(id));
    }
    fn set_text(&mut self, w: u32, text: &str) {
        self.view.set_text(id(w), text);
    }
    fn set_align(&mut self, w: u32, align: Align) {
        let a = match align {
            Align::Left => tk::Align::Left,
            Align::Center => tk::Align::Center,
            Align::Right => tk::Align::Right,
        };
        self.view.set_align(id(w), a, tk::VAlign::Middle);
    }
    fn set_font(&mut self, w: u32, font: Font) {
        let f = tk_font(font);
        self.view.set_font(id(w), f);
    }
    fn set_wrap(&mut self, w: u32, wrap: bool) {
        self.view.set_wrap(id(w), wrap);
    }
    fn set_highlight(&mut self, w: u32, on: bool) {
        let (fg, bg) = if on {
            (
                tk::Color::Theme(tk::ThemeColor::SelectForeground),
                tk::Color::Theme(tk::ThemeColor::SelectBackground),
            )
        } else {
            (tk::Color::Default, tk::Color::Default)
        };
        self.view.set_colors(id(w), fg, bg);
    }
    fn set_items(&mut self, w: u32, items: &[ListItem]) {
        let icons = items.iter().any(|i| i.icon.is_some());
        let details = items.iter().any(|i| !i.detail.is_empty());
        let m = self.view.metrics();
        let mut columns = Vec::new();
        if icons {
            columns.push(tk::Column::px(m.row_height));
        }
        columns.push(tk::Column::rest());
        if details {
            columns.push(tk::Column::percent(40).align(tk::Align::Right));
        }
        let rows = items
            .iter()
            .map(|i| {
                let mut cells = Vec::new();
                if icons {
                    cells.push(match &i.icon {
                        None => tk::Cell::Empty,
                        Some(Image::Icon(icon)) => tk::Cell::icon(icon_name(*icon).to_string()),
                        // A picture of the app's, reduced to the row.
                        Some(Image::Picture(name)) => match self.images.get(name) {
                            Some(img) => tk::Cell::Image(img.fit(m.row_height, m.row_height)),
                            None => tk::Cell::Empty,
                        },
                    });
                }
                cells.push(tk::Cell::text(i.text.clone()));
                if details {
                    cells.push(tk::Cell::text(i.detail.clone()));
                }
                cells
            })
            .collect();
        let mut b = self.view.batch();
        let keep = b.list_selected(id(w)).unwrap_or(0);
        b.list_set_columns(id(w), &columns);
        b.list_set_rows(id(w), rows);
        b.list_select(id(w), keep.min(items.len().saturating_sub(1) as u32));
    }
    fn set_selected(&mut self, w: u32, index: u32) {
        self.view.list_select(id(w), index);
    }
    fn selected(&self, w: u32) -> Option<u32> {
        self.view.list_selected(id(w))
    }
    fn hold_changes(&mut self, on: bool) {
        self.view.hold_changes(on);
    }
    fn set_value(&mut self, w: u32, value: &str) {
        self.view.set_text(id(w), value);
    }
    fn value(&self, w: u32) -> String {
        self.view.text(id(w)).unwrap_or_default()
    }
    fn set_input_mode(&mut self, w: u32, mode: InputMode) {
        let m = match mode {
            InputMode::Text => tk::InputMode::Text,
            InputMode::Numeric => tk::InputMode::Numeric,
            InputMode::Dial => tk::InputMode::Dial,
            InputMode::Password => tk::InputMode::Password,
            InputMode::Pin => tk::InputMode::Pin,
        };
        self.view.input_set_mode(id(w), m);
    }
    fn set_max_length(&mut self, w: u32, max: u32) {
        self.view
            .input_set_max_len(id(w), max.min(u16::MAX as u32) as u16);
    }
    fn set_image(&mut self, w: u32, image: &Image) {
        let source = match image {
            Image::Icon(icon) => tk::ImageSource::Icon(icon_name(*icon).to_string()),
            Image::Picture(name) => match self.images.get(name) {
                Some(img) => tk::ImageSource::Rgba {
                    width: img.width,
                    height: img.height,
                    pixels: img.pixels.clone(),
                },
                // Nothing: no icon has an empty name.
                None => tk::ImageSource::Icon(String::new()),
            },
        };
        self.view.set_image(id(w), source);
    }
    fn define_image(&mut self, name: &str, file: &[u8]) -> bool {
        // A bounded set.
        if !self.images.contains_key(name) && self.images.len() >= 32 {
            return false;
        }
        let m = self.view.metrics();
        match opendeskphone_ui::image::decode_png(file) {
            Ok(img) => {
                self.images
                    .insert(name.to_string(), img.fit(m.width, m.height));
                true
            }
            Err(_) => false,
        }
    }
    fn set_full_screen(&mut self, on: bool) {
        self.view.set_full_screen(on);
    }
    fn capture_input(&mut self, on: bool) {
        self.view.set_capture(on);
    }
    fn set_lamp(&mut self, lamp: Lamp, color: opendeskphone_state::LampColor) {
        let lamp = match lamp {
            Lamp::Line(i) => tk::Lamp::Line(i),
            Lamp::Blf(i) => tk::Lamp::Blf(i),
            Lamp::Message => tk::Lamp::Message,
            Lamp::Mute => tk::Lamp::Mute,
            Lamp::Headset => tk::Lamp::Headset,
            Lamp::Speaker => tk::Lamp::Speaker,
        };
        self.view.set_lamp(lamp, color);
    }
    fn release_lamps(&mut self) {
        self.view.release_lamps();
    }
    fn theme_color(&self, role: ColorRole) -> Color {
        let (r, g, b) = self.view.theme_color(match role {
            ColorRole::Background => tk::ThemeColor::Background,
            ColorRole::Foreground => tk::ThemeColor::Foreground,
            ColorRole::Highlight => tk::ThemeColor::SelectBackground,
            ColorRole::HighlightText => tk::ThemeColor::SelectForeground,
            ColorRole::TitleBackground => tk::ThemeColor::TitleBackground,
            ColorRole::TitleText => tk::ThemeColor::TitleForeground,
            ColorRole::Secondary => tk::ThemeColor::Secondary,
            ColorRole::Border => tk::ThemeColor::Border,
        });
        Color { r, g, b }
    }
    fn text_size(&self, font: Font, text: &str) -> (u16, u16) {
        self.view.text_ink(tk_font(font), text)
    }
    fn create_canvas(&mut self, parent: Option<u32>, bounds: Rect) -> u32 {
        self.view
            .create(tk::WidgetKind::Canvas, parent.map(id), rect(bounds))
            .0
    }
    fn draw(&mut self, w: u32, ops: Vec<DrawOp>) -> bool {
        let color = |c: Color| tk::Color::Rgb(c.r, c.g, c.b);
        let ops = ops
            .into_iter()
            .map(|op| match op {
                DrawOp::Fill((r, c)) => tk::DrawOp::Fill {
                    rect: rect(r),
                    color: color(c),
                },
                DrawOp::Bitmap(b) => tk::DrawOp::Bitmap {
                    x: b.x,
                    y: b.y,
                    width: b.width,
                    height: b.height,
                    scale: b.scale,
                    color: color(b.color),
                    bits: b.bits.into(),
                },
                DrawOp::Text(t) => tk::DrawOp::Text {
                    rect: rect(t.box_),
                    font: tk_font(t.font),
                    color: color(t.color),
                    text: t.text,
                },
            })
            .collect();
        self.view.canvas_set_ops(id(w), ops)
    }
    fn set_softkeys(&mut self, w: Option<u32>, keys: &[Softkey]) {
        let keys = keys
            .iter()
            .map(|k| tk::Softkey::new(k.id, k.label.clone()))
            .collect();
        self.view.set_softkeys(w.map(id), keys);
    }
    fn claim_keys(&mut self, w: Option<u32>, keys: &[Key]) {
        let claims = tk::KeyClaims::keys(keys.iter().copied());
        match w {
            Some(w) => self.view.set_key_claims(id(w), claims),
            None => self.view.set_window_key_claims(claims),
        }
    }
    fn claim_touch(&mut self, w: Option<u32>, on: bool) {
        self.view.set_touch_claim(w.map(id), on);
    }
    fn poll_event(&mut self) -> Option<WindowEvent> {
        let point = |x, y| Point { x, y };
        loop {
            return Some(match self.events.try_recv()? {
                AppEvent::Foreground => WindowEvent::Foreground,
                AppEvent::Background => WindowEvent::Background,
                AppEvent::Key { key, .. } => WindowEvent::Key(key),
                AppEvent::Hook { off_hook } => WindowEvent::Hook(off_hook),
                AppEvent::Softkey { id } => WindowEvent::Softkey(id),
                AppEvent::ListSelected { widget, row } => WindowEvent::Highlighted {
                    widget: widget.0,
                    row,
                },
                AppEvent::ListActivated { widget, row } => WindowEvent::Activated {
                    widget: widget.0,
                    row,
                },
                AppEvent::InputChanged { widget, value } => WindowEvent::Changed {
                    widget: widget.0,
                    value,
                },
                AppEvent::InputSubmitted { widget, value } => WindowEvent::Submitted {
                    widget: widget.0,
                    value,
                },
                AppEvent::Touch { widget, touch } => WindowEvent::Touch {
                    widget: widget.map_or(0, |w| w.0),
                    touch: match touch {
                        tk::Touch::Down { x, y } => Touch::Down(point(x, y)),
                        tk::Touch::Tap { x, y } => Touch::Tap(point(x, y)),
                        tk::Touch::Drag { x0, y0, x, y } => Touch::Drag((point(x0, y0), point(x, y))),
                        tk::Touch::DragEnd { x0, y0, x, y } => Touch::DragEnd((point(x0, y0), point(x, y))),
                    },
                },
                AppEvent::CloseRequested => WindowEvent::CloseRequested,
                AppEvent::Focused { .. } => continue,
            });
        }
    }
}

impl Shell for UiShell {
    fn open(
        &mut self,
        app: &str,
        kind: WindowKind,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Box<dyn AppWindow> {
        // With the UI's theme from the start: an app asks for the theme's
        // colors before its window is first on screen.
        let (view, events) = self.ui.new_view(app, self.model);
        events.set_waker(move || wake());
        match kind {
            WindowKind::App => {
                self.views.insert(app.to_string(), view.clone());
            }
            WindowKind::Idle => self.ui.add_idle_view(view.clone()),
        }
        Box::new(UiWindow {
            view,
            events,
            images: HashMap::new(),
        })
    }
    fn show(&mut self, app: &str) {
        if let Some(v) = self.views.get(app) {
            self.ui.open_app(v.clone());
        }
    }
    fn hide(&mut self, app: &str) {
        self.ui.close_app(app);
    }
    fn close(&mut self, app: &str, kind: WindowKind) {
        match kind {
            WindowKind::App => {
                self.ui.close_app(app);
                self.views.remove(app);
            }
            WindowKind::Idle => self.ui.remove_idle_view(app),
        }
    }
    fn idle_size(&self) -> Size {
        let s = opendeskphone_ui::idle_app_area(self.model);
        Size {
            width: s.width,
            height: s.height,
        }
    }
    fn glyph_height(&self) -> u16 {
        opendeskphone_ui::glyph_height(self.model)
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{ICONS, icon_name};

    #[test]
    fn the_interface_names_every_icon_of_the_phone() {
        let mut ours: Vec<&str> = ICONS.iter().map(|i| icon_name(*i)).collect();
        // The boot screen's artwork is not an icon for apps.  Another case
        // in the interface's enum would make the phone refuse apps built
        // without it, so neither are the six twins that are not status
        // icons too (an app names the app icon, e.g., `app-calendar`), nor
        // the Timer's icon, which the phone finds by the app's name.
        let twins = ["boxpusher", "calendar", "desksparrow", "diagnostics", "lightup", "timer"];
        let mut phone: Vec<&str> = opendeskphone_ui::icons::names()
            .into_iter()
            .filter(|n| !["splash", "app-timer"].contains(n) && !twins.contains(n))
            .collect();
        ours.sort();
        phone.sort();
        assert_eq!(ours, phone);
    }
}
