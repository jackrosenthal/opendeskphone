//! An app view on the touch skin (D80), through the HAL simulator's
//! touchscreen: the stock widgets are used by touch without the app's
//! help, the softkeys and the way back are buttons, and a widget that
//! takes touches gets them with places of its own.

use opendeskphone_hal::Hal;
use opendeskphone_state::{Key, Model, Phase, Store, System};
use opendeskphone_ui::appview::{
    AppView, Cell, Color, InputMode, KeyClaims, Metrics, Rect, Softkey, Touch, WidgetId, WidgetKind,
};
use opendeskphone_ui::gfx::IRect;
use opendeskphone_ui::osk::{Cap, Keyboard};
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::{AppEvent, AppEvents, Hit, Ui, UiConfig};

struct Rig {
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    view: AppView,
    events: AppEvents,
    m: Metrics,
}

impl Rig {
    /// A D80 with an empty app view on its screen.
    fn new() -> Rig {
        Rig::on(Model::D80)
    }
    fn on(model: Model) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        let mut ui = Ui::from_hal(&mut hal, store, UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.set_fixed_time(Some((5_000_000, 1_794_607_087)));
        let (view, events) = AppView::new("demo", model);
        ui.handle().open_app(view.clone());
        ui.pump();
        assert_eq!(events.try_recv(), Some(AppEvent::Foreground));
        let m = view.metrics();
        Rig { ui, sim, view, events, m }
    }
    /// A list of `n` rows under a title's height, to the bottom of the view.
    fn list(&self, n: usize) -> WidgetId {
        let m = self.m;
        let list = self.view.create(WidgetKind::List, None, Rect::new(0, m.title_height as i16, m.width, m.height - m.title_height));
        self.view.list_set_rows(list, (0..n).map(|i| vec![Cell::text(format!("Row {i}"))]).collect());
        list
    }
    fn tap(&mut self, x: i32, y: i32) {
        self.sim.touch_tap(x as u16, y as u16);
        self.ui.pump();
    }
    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) {
        self.sim.touch_drag((from.0 as u16, from.1 as u16), (to.0 as u16, to.1 as u16), 4);
        self.ui.pump();
    }
    /// The region registered with `hit` in the picture on the screen.
    fn region(&self, hit: Hit) -> IRect {
        self.ui.canvas().hit_rect(hit).unwrap_or_else(|| panic!("no region {hit:?}"))
    }
    fn tap_hit(&mut self, hit: Hit) {
        let r = self.region(hit);
        self.tap(r.x + r.w / 2, r.y + r.h / 2);
    }
    fn events(&self) -> Vec<AppEvent> {
        std::iter::from_fn(|| self.events.try_recv()).collect()
    }
}

#[test]
fn the_metrics_are_the_touch_skins() {
    let m = Metrics::for_model(Model::D80);
    let th = Theme::for_model(Model::D80);
    assert_eq!((m.width, m.height), (800, 1160));
    assert_eq!(m.skin, opendeskphone_state::Skin::Touch);
    assert!(m.color && !m.keys);
    // The skin's own sizes: what the app lays out with is what the phone
    // draws with.
    assert_eq!(i32::from(m.row_height), th.row_h);
    assert_eq!(i32::from(m.title_height), th.title_h);
    assert_eq!(i32::from(m.line_height), th.line_h);
    assert_eq!(i32::from(m.scroll_bar_width), th.scroll_w);
    assert_eq!(i32::from(m.back_width), th.back_w);
    // A row and the back button are large enough to touch.
    assert!(i32::from(m.row_height) >= th.touch_min && i32::from(m.back_width) >= th.touch_min);
    // The phones with keys have keys, and no back button.
    for model in [Model::D40, Model::D45, Model::D50, Model::D60, Model::D62, Model::D65, Model::D70] {
        let m = Metrics::for_model(model);
        assert!(m.keys && m.back_width == 0, "{model:?}");
    }
    // Full-screen, the view has the display.
    let (view, _events) = AppView::new("demo", Model::D80);
    view.set_full_screen(true);
    assert_eq!((view.metrics().width, view.metrics().height), (800, 1280));
}

#[test]
fn a_tap_on_a_row_selects_and_activates_it() {
    let mut r = Rig::new();
    let list = r.list(5);
    r.ui.pump();
    let (top, row_h) = (i32::from(r.m.title_height), i32::from(r.m.row_height));
    // The third row: selected, and activated.
    r.tap(400, top + 2 * row_h + row_h / 2);
    assert_eq!(
        r.events(),
        [
            AppEvent::Focused { widget: list },
            AppEvent::ListSelected { widget: list, row: 2 },
            AppEvent::ListActivated { widget: list, row: 2 },
        ]
    );
    assert_eq!(r.view.list_selected(list), Some(2));
    assert_eq!(r.view.focused(), Some(list));
    // The row that is selected already is only activated.
    r.tap(10, top + 2 * row_h + 1);
    assert_eq!(r.events(), [AppEvent::ListActivated { widget: list, row: 2 }]);
    // The first row, at its upper edge.
    r.tap(790, top);
    assert_eq!(r.events(), [AppEvent::ListSelected { widget: list, row: 0 }, AppEvent::ListActivated { widget: list, row: 0 }]);
    // Below the last row, and above the list, nothing is touched.
    r.tap(400, top + 5 * row_h + 10);
    r.tap(400, top - 10);
    assert_eq!(r.events(), []);
    // A long press is a tap.
    r.sim.touch(opendeskphone_state::TouchPhase::Down, 400, (top + row_h + 5) as u16);
    r.ui.pump();
    r.ui.set_fixed_time(Some((5_000_000 + 1000, 1_794_607_088)));
    r.ui.pump();
    r.sim.touch(opendeskphone_state::TouchPhase::Up, 400, (top + row_h + 5) as u16);
    r.ui.pump();
    assert_eq!(r.events(), [AppEvent::ListSelected { widget: list, row: 1 }, AppEvent::ListActivated { widget: list, row: 1 }]);
}

#[test]
fn a_drag_scrolls_a_list_and_a_tap_then_finds_the_row_under_the_finger() {
    let mut r = Rig::new();
    let list = r.list(40);
    r.ui.pump();
    let (top, row_h) = (i32::from(r.m.title_height), i32::from(r.m.row_height));
    // Up by three rows and a bit: the fourth row is the first shown.
    r.drag((400, top + 5 * row_h), (400, top + 2 * row_h - 10));
    assert_eq!(r.events(), [], "scrolling tells the app nothing");
    r.tap(400, top + row_h / 2);
    assert_eq!(
        r.events(),
        [
            AppEvent::Focused { widget: list },
            AppEvent::ListSelected { widget: list, row: 3 },
            AppEvent::ListActivated { widget: list, row: 3 },
        ]
    );
    // Down again, farther than there are rows: the first row is first.
    r.drag((400, top + row_h), (400, top + 9 * row_h));
    r.tap(400, top + row_h / 2);
    assert_eq!(r.events(), [AppEvent::ListSelected { widget: list, row: 0 }, AppEvent::ListActivated { widget: list, row: 0 }]);
    // To the end: the last row is the last one shown, and no farther.
    for _ in 0..8 {
        r.drag((400, top + 10 * row_h), (400, top));
    }
    let shown = (i32::from(r.m.height) - top) / row_h;
    r.tap(400, top + (shown - 1) * row_h + 5);
    assert_eq!(r.events(), [AppEvent::ListSelected { widget: list, row: 39 }, AppEvent::ListActivated { widget: list, row: 39 }]);
    // The list did not follow its selection while the finger scrolled; a
    // row the app selects is brought into view.
    r.view.list_select(list, 7);
    r.ui.pump();
    r.tap(400, top + 5);
    assert_eq!(r.events(), [AppEvent::ListActivated { widget: list, row: 7 }]);
    // A list whose rows all show does not move.
    let mut r = Rig::new();
    let list = r.list(3);
    r.ui.pump();
    r.drag((400, top + 2 * row_h + 5), (400, top + 5));
    r.tap(400, top + 5);
    assert_eq!(r.events(), [AppEvent::Focused { widget: list }, AppEvent::ListActivated { widget: list, row: 0 }]);
}

#[test]
fn the_softkeys_are_buttons_and_more_than_four_page() {
    let mut r = Rig::new();
    r.list(2);
    r.view.set_softkeys(None, (1..=6).map(|i| Softkey::new(i, format!("Key {i}"))).collect());
    r.ui.pump();
    let th = Theme::for_model(Model::D80);
    // Four buttons below the view, each large enough to touch.
    for n in 1..=4 {
        let b = r.region(Hit::Key(Key::Soft(n)));
        assert!(b.w >= th.touch_min && b.h >= th.touch_min, "button {n}: {b:?}");
        assert!(b.y >= i32::from(r.m.height), "button {n} is below the view");
    }
    r.tap_hit(Hit::Key(Key::Soft(2)));
    assert_eq!(r.events(), [AppEvent::Softkey { id: 2 }]);
    // The fourth is "More": the next three, and around again.
    r.tap_hit(Hit::Key(Key::Soft(4)));
    assert_eq!(r.events(), []);
    r.tap_hit(Hit::Key(Key::Soft(1)));
    r.tap_hit(Hit::Key(Key::Soft(3)));
    assert_eq!(r.events(), [AppEvent::Softkey { id: 4 }, AppEvent::Softkey { id: 6 }]);
    r.tap_hit(Hit::Key(Key::Soft(4)));
    r.tap_hit(Hit::Key(Key::Soft(1)));
    assert_eq!(r.events(), [AppEvent::Softkey { id: 1 }]);
    // A position without a softkey has no button.
    r.view.set_softkeys(None, vec![Softkey::new(9, "Only")]);
    r.ui.pump();
    assert_eq!(r.ui.canvas().hit_rect(Hit::Key(Key::Soft(2))), None);
    r.tap_hit(Hit::Key(Key::Soft(1)));
    assert_eq!(r.events(), [AppEvent::Softkey { id: 9 }]);
}

#[test]
fn the_back_button_is_the_cancel_key() {
    let mut r = Rig::new();
    r.list(2);
    r.ui.pump();
    // In the corner the metrics tell the app to keep free.
    let back = r.region(Hit::Key(Key::Cancel));
    assert_eq!(back, IRect::new(0, 0, i32::from(r.m.back_width), i32::from(r.m.title_height)));
    // Unclaimed, it asks the app to close, as the X key does.
    r.tap_hit(Hit::Key(Key::Cancel));
    assert_eq!(r.events(), [AppEvent::CloseRequested]);
    // Claimed, it is the app's key.
    r.view.set_window_key_claims(KeyClaims::keys([Key::Cancel]));
    r.tap_hit(Hit::Key(Key::Cancel));
    assert_eq!(r.events(), [AppEvent::Key { widget: None, key: Key::Cancel }]);
    // It is over whatever the app has there, also a widget that takes
    // touches, and also in a full-screen view, where the arrow is on a
    // round button of its own (the app draws there what it likes): more
    // of the corner is drawn on than the arrow alone.
    let drawn = |r: &Rig| (0..96 * 96).filter(|i| r.ui.canvas().pixel(i % 96, i / 96) != r.ui.canvas().pixel(95, 95)).count();
    let arrow = drawn(&r);
    r.view.set_touch_claim(None, true);
    r.view.set_full_screen(true);
    r.ui.pump();
    assert!(arrow > 0 && drawn(&r) > 72 * 72 / 2, "{arrow} {}", drawn(&r));
    assert_eq!(r.ui.canvas().pixel(1, 1), r.ui.canvas().pixel(95, 95), "round: not the corner itself");
    assert_eq!(r.region(Hit::Key(Key::Cancel)), back);
    r.tap(10, 10);
    assert_eq!(r.events(), [AppEvent::Key { widget: None, key: Key::Cancel }]);
    // A view that captures every key is left by three in a row.
    r.view.set_capture(true);
    for _ in 0..3 {
        r.tap(10, 10);
    }
    assert_eq!(r.events().len(), 3, "two keys and the background");
    assert_ne!(r.ui.screen_name(), "app:demo");
}

#[test]
fn a_tap_on_an_input_focuses_it_for_the_keyboard() {
    let mut r = Rig::new();
    let m = r.m;
    let list = r.list(2);
    r.view.set_rect(list, Rect::new(0, 400, m.width, 2 * m.row_height));
    let name = r.view.create(WidgetKind::Input, None, Rect::new(16, 100, m.width - 32, m.row_height));
    let pin = r.view.create(WidgetKind::Input, None, Rect::new(16, 220, m.width - 32, m.row_height));
    r.view.input_set_mode(pin, InputMode::Pin);
    r.view.input_set_max_len(pin, 4);
    r.view.focus(Some(list));
    r.ui.pump();
    assert_eq!(r.view.keyboard_input(), None);

    r.tap(400, 100 + 40);
    assert_eq!(r.events(), [AppEvent::Focused { widget: name }]);
    assert_eq!(r.view.focused(), Some(name));
    assert_eq!(r.view.keyboard_input(), Some((name, InputMode::Text)));
    // What a keyboard types goes in at the cursor, and the app is told.
    assert!(r.view.type_text("Ada"));
    assert!(r.view.backspace());
    assert!(r.view.type_text("am"));
    assert_eq!(
        r.events(),
        [
            AppEvent::InputChanged { widget: name, value: "Ada".into() },
            AppEvent::InputChanged { widget: name, value: "Ad".into() },
            AppEvent::InputChanged { widget: name, value: "Adam".into() },
        ]
    );
    // The focused input again: nothing new.
    r.tap(400, 100 + 40);
    assert_eq!(r.events(), []);

    // The other input: its mode's characters only, and no more than fit.
    r.tap(400, 220 + 40);
    assert_eq!(r.view.keyboard_input(), Some((pin, InputMode::Pin)));
    assert!(r.view.type_text("1a2-3456"));
    assert_eq!(r.events(), [AppEvent::Focused { widget: pin }, AppEvent::InputChanged { widget: pin, value: "1234".into() }]);
    // The keyboard's Enter is Select: the input is submitted.
    assert!(r.view.dispatch_key(Key::Select));
    assert_eq!(r.events(), [AppEvent::InputSubmitted { widget: pin, value: "1234".into() }]);

    // A list is not typed into.
    r.tap(400, 400 + 10);
    assert_eq!(r.view.keyboard_input(), None);
    assert!(!r.view.type_text("x") && !r.view.backspace());
    // No model with keys has an on-screen keyboard.
    let (view, _events) = AppView::new("demo", Model::D60);
    let input = view.create(WidgetKind::Input, None, Rect::new(0, 0, 100, 30));
    view.focus(Some(input));
    assert_eq!(view.keyboard_input(), None);
}

/// Saves the picture on the screen as `name` among the D80's snapshots.
fn save_picture(r: &Rig, name: &str) {
    let frame = r.sim.frame(opendeskphone_hal::Screen::Main).expect("the main screen");
    let (w, h) = (frame.info.width as usize, frame.info.height as usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let i = y * frame.info.stride + x * 4;
            rgb.extend_from_slice(&[frame.pixels[i + 2], frame.pixels[i + 1], frame.pixels[i]]);
        }
    }
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-snapshots").join("D80");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::io::BufWriter::new(std::fs::File::create(dir.join(name)).unwrap());
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

/// A form as the SDK lays one out: a title, a name, a PIN, and softkeys.
fn form(r: &Rig) -> (WidgetId, WidgetId) {
    let m = r.m;
    let title = r.view.create(WidgetKind::Text, None, Rect::new(m.back_width as i16, 0, m.width - m.back_width, m.title_height));
    r.view.set_font(title, opendeskphone_ui::appview::Font::Title);
    r.view.set_text(title, "Log In");
    let name = r.view.create(WidgetKind::Input, None, Rect::new(16, 112, m.width - 32, m.row_height));
    let pin = r.view.create(WidgetKind::Input, None, Rect::new(16, 224, m.width - 32, m.row_height));
    r.view.input_set_mode(pin, InputMode::Pin);
    r.view.set_softkeys(None, vec![Softkey::new(1, "Log In"), Softkey::new(2, "Cancel")]);
    (name, pin)
}

#[test]
fn a_focused_input_has_the_on_screen_keyboard() {
    let mut r = Rig::new();
    let th = Theme::for_model(Model::D80);
    let (name, pin) = form(&r);
    r.ui.pump();
    // Nothing has the focus: the softkeys are buttons, and there is no
    // keyboard.
    assert!(r.ui.canvas().hit_rect(Hit::Key(Key::Soft(1))).is_some());
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_none());

    // The app focuses the name (as a form does when it opens): the
    // letters, across the bottom of the screen, over the softkeys.
    r.view.focus(Some(name));
    r.ui.pump();
    let done = r.region(Cap::Done.hit());
    assert_eq!(done.bottom(), 1280 - th.px(4));
    assert!(r.region(Cap::Char('q').hit()).y >= 1280 - Keyboard::height(th));
    assert!(r.ui.canvas().hit_rect(Hit::Key(Key::Soft(1))).is_none());
    // The field is above the keyboard, where the app put it.
    assert_eq!(r.ui.canvas().hit_at(400, 112 + 40), Some(Hit::Custom(0)));
    save_picture(&r, "app-keyboard.png");

    // What is typed goes into the input, and the app is told.
    r.tap_hit(Cap::Shift.hit());
    r.tap_hit(Cap::Char('A').hit());
    r.tap_hit(Cap::Char('d').hit());
    r.tap_hit(Cap::Char('x').hit());
    r.tap_hit(Cap::Backspace.hit());
    r.tap_hit(Cap::Char('a').hit());
    assert_eq!(r.view.text(name).as_deref(), Some("Ada"));
    assert_eq!(
        r.events().last(),
        Some(&AppEvent::InputChanged { widget: name, value: "Ada".into() })
    );
    // Between the keys nothing is touched (not the view under them).
    let q = r.region(Cap::Char('q').hit());
    r.tap(q.right() + 1, q.bottom() + 1);
    assert_eq!(r.events(), []);
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_some());
    // A finger that slides on the keys types nothing.
    r.drag((q.x + 10, q.y + 10), (q.x + 300, q.y + 10));
    assert_eq!(r.events(), []);

    // The done key submits the input.  The app moves the focus to the
    // PIN: the number pad, with digits only.
    r.tap_hit(Cap::Done.hit());
    assert_eq!(r.events(), [AppEvent::InputSubmitted { widget: name, value: "Ada".into() }]);
    r.view.focus(Some(pin));
    r.ui.pump();
    assert!(r.ui.canvas().hit_rect(Cap::Char('a').hit()).is_none());
    assert!(r.ui.canvas().hit_rect(Cap::Char('*').hit()).is_none());
    for ch in "1234".chars() {
        r.tap_hit(Cap::Char(ch).hit());
    }
    assert_eq!(r.view.text(pin).as_deref(), Some("1234"));
    r.events();
    // Done on the last input: submitted, and the keyboard is put away,
    // so the softkeys are there again.
    r.tap_hit(Cap::Done.hit());
    assert_eq!(r.events(), [AppEvent::InputSubmitted { widget: pin, value: "1234".into() }]);
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_none());
    r.tap_hit(Hit::Key(Key::Soft(1)));
    assert_eq!(r.events(), [AppEvent::Softkey { id: 1 }]);
}

#[test]
fn the_keyboard_is_put_away_and_brought_back() {
    let mut r = Rig::new();
    let (name, pin) = form(&r);
    r.view.focus(Some(name));
    r.ui.pump();
    let back = IRect::new(0, 0, 96, 96);
    // While the keyboard is up, the back button puts it away, and the
    // app hears nothing.
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_some());
    r.tap(48, 48);
    assert_eq!(r.events(), []);
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_none());
    assert_eq!(r.view.focused(), Some(name));
    assert!(r.ui.canvas().hit_rect(Hit::Key(Key::Soft(2))).is_some());
    // Then it is the way back again.
    assert_eq!(r.region(Hit::Key(Key::Cancel)), back);
    // A touch on the input brings the keyboard back.
    r.tap(400, 112 + 40);
    assert_eq!(r.events(), []);
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_some());
    // A touch on the view beside the input puts it away too.
    r.tap(400, 500);
    assert_eq!(r.events(), []);
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_none());
    // A touch on the other input: the keyboard of that one.
    r.tap(400, 224 + 40);
    assert_eq!(r.events(), [AppEvent::Focused { widget: pin }]);
    assert!(r.ui.canvas().hit_rect(Cap::Char('5').hit()).is_some());
    // And back on the first, which the keyboard was put away for before.
    r.tap(400, 112 + 40);
    assert!(r.ui.canvas().hit_rect(Cap::Char('q').hit()).is_some());
    // The app takes the focus away: no keyboard.
    r.view.focus(None);
    r.ui.pump();
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_none());
}

#[test]
fn an_input_under_the_keyboard_is_brought_above_it() {
    let mut r = Rig::new();
    let th = Theme::for_model(Model::D80);
    let m = r.m;
    let (_name, pin) = form(&r);
    // An input at the foot of the view.
    let low = r.view.create(WidgetKind::Input, None, Rect::new(16, 1040, m.width - 32, m.row_height));
    r.view.set_text(low, "low");
    r.ui.pump();
    let top = 1280 - Keyboard::height(th);
    // Touched: it has the focus, and the view is drawn farther up, so
    // that the input is above the keyboard.
    r.tap(400, 1040 + 40);
    assert_eq!(r.events(), [AppEvent::Focused { widget: low }]);
    // (The region is the part of the view on the screen: its foot tells
    // how far up the view is.)
    let shown = r.region(Hit::Custom(0));
    let view_y = shown.bottom() - 1160;
    assert!(view_y < 0);
    assert!(view_y + 1040 + i32::from(m.row_height) <= top, "{shown:?}");
    // Nothing shows between the view and the keyboard.
    assert!(shown.bottom() >= top);
    save_picture(&r, "app-keyboard-low.png");
    // What is typed goes in, and a touch goes to what is drawn there:
    // the input where it is now keeps the keyboard, and another input
    // is found where it is now.
    r.tap_hit(Cap::Char('e').hit());
    assert_eq!(r.view.text(low).as_deref(), Some("lowe"));
    r.events();
    r.tap(400, view_y + 1040 + 40);
    assert_eq!(r.events(), []);
    assert!(r.ui.canvas().hit_rect(Cap::Done.hit()).is_some());
    r.tap(400, view_y + 224 + 80);
    assert_eq!(r.events(), [AppEvent::Focused { widget: pin }]);
    // That one needs no room made: the view is in its place again.
    assert_eq!(r.region(Hit::Custom(0)).bottom(), 1160);
    // Put away, the view is in its place too.
    r.tap(48, 48);
    r.tap(400, 1040 + 40);
    assert!(r.region(Hit::Custom(0)).bottom() < 1160);
    r.tap(48, 48);
    assert_eq!(r.region(Hit::Custom(0)), IRect::new(0, 0, 800, 1160));
}

#[test]
fn a_drag_scrolls_a_scroll_widget_from_anything_in_it() {
    let mut r = Rig::new();
    let m = r.m;
    let top = m.title_height as i16;
    let scroll = r.view.create(WidgetKind::Scroll, None, Rect::new(0, top, m.width, 600));
    // Twice as much as shows: texts, with an input and a short list among
    // them, and at the end a marker.
    let text = r.view.create(WidgetKind::Text, Some(scroll), Rect::new(0, 0, m.width, 300));
    r.view.set_text(text, "Text");
    let input = r.view.create(WidgetKind::Input, Some(scroll), Rect::new(16, 300, m.width - 32, m.row_height));
    let list = r.view.create(WidgetKind::List, Some(scroll), Rect::new(0, 400, m.width, 2 * m.row_height));
    r.view.list_set_rows(list, vec![vec![Cell::text("One")], vec![Cell::text("Two")]]);
    let marker = r.view.create(WidgetKind::Group, Some(scroll), Rect::new(0, 1100, m.width, 100));
    r.view.set_colors(marker, Color::Default, Color::Rgb(255, 0, 255));
    r.ui.pump();
    let magenta = 0xFF_00FF;
    let top = i32::from(top);
    let shows = |r: &Rig, y: i32| r.ui.canvas().pixel(400, y) == magenta;
    assert!(!shows(&r, top + 550));

    // From the text, up by 200 pixels; from the input, by 100; from the
    // list (whose rows all show), by 190 and by 100; and from the marker,
    // which does nothing with a finger itself, to the end and no farther.
    r.drag((400, top + 250), (400, top + 50));
    assert!(!shows(&r, top + 599));
    r.drag((400, top + 150), (400, top + 50));
    r.drag((400, top + 200), (400, top + 10));
    r.drag((400, top + 50), (400, top - 50));
    assert!(shows(&r, top + 599) && shows(&r, top + 511) && !shows(&r, top + 509));
    r.drag((400, top + 550), (400, top + 300));
    assert!(shows(&r, top + 599) && shows(&r, top + 501) && !shows(&r, top + 499));
    assert_eq!(r.events(), [], "scrolling tells the app nothing, and focuses nothing");
    assert_eq!(r.view.focused(), None);
    // Down by 200: the rows are where they now show.
    r.drag((400, top + 300), (400, top + 500));
    r.tap(400, top + 96 + 10);
    assert_eq!(
        r.events(),
        [
            AppEvent::Focused { widget: list },
            AppEvent::ListSelected { widget: list, row: 1 },
            AppEvent::ListActivated { widget: list, row: 1 },
        ]
    );
    // A focus the app gives a widget out of sight is scrolled to, once;
    // after that the finger scrolls away from it again.
    r.view.scroll_to(scroll, 600);
    r.ui.pump();
    assert!(shows(&r, top + 599));
    r.view.focus(Some(input));
    r.ui.pump();
    assert!(!shows(&r, top + 599), "the input is brought into view");
    r.drag((400, top + 500), (400, top + 100));
    assert!(shows(&r, top + 599));
    assert_eq!(r.view.focused(), Some(input));
}

#[test]
fn a_widget_that_takes_touches_gets_them_with_its_own_places() {
    let mut r = Rig::new();
    let m = r.m;
    // A canvas in a group, each away from the corner of what it is in.
    let group = r.view.create(WidgetKind::Group, None, Rect::new(50, 200, 700, 800));
    let canvas = r.view.create(WidgetKind::Canvas, Some(group), Rect::new(30, 40, 400, 300));
    // A text over part of the canvas takes nothing from it.
    let label = r.view.create(WidgetKind::Text, Some(group), Rect::new(30, 40, 200, 100));
    r.view.set_text(label, "Label");
    r.view.set_touch_claim(Some(canvas), true);
    r.ui.pump();

    // A tap: the finger coming down, when it does, and then the tap.
    r.sim.touch(opendeskphone_state::TouchPhase::Down, 80 + 10, 240 + 20);
    r.ui.pump();
    assert_eq!(r.events(), [AppEvent::Touch { widget: Some(canvas), touch: Touch::Down { x: 10, y: 20 } }]);
    r.sim.touch(opendeskphone_state::TouchPhase::Up, 80 + 10, 240 + 20);
    r.ui.pump();
    assert_eq!(r.events(), [AppEvent::Touch { widget: Some(canvas), touch: Touch::Tap { x: 10, y: 20 } }]);
    r.tap(80 + 399, 240 + 299);
    assert_eq!(
        r.events(),
        [
            AppEvent::Touch { widget: Some(canvas), touch: Touch::Down { x: 399, y: 299 } },
            AppEvent::Touch { widget: Some(canvas), touch: Touch::Tap { x: 399, y: 299 } },
        ]
    );
    // Beside it, nothing takes the touch.
    r.tap(80 + 400, 240 + 20);
    r.tap(80 + 10, 240 - 1);
    assert_eq!(r.events(), []);

    // A drag: the finger coming down, every movement, from where it came
    // down, and the end; the finger may leave the widget.
    r.drag((80 + 100, 240 + 100), (80 + 500, 240 - 60));
    let events = r.events();
    let drags: Vec<(i16, i16)> = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::Touch { widget, touch: Touch::Drag { x0: 100, y0: 100, x, y } } if *widget == Some(canvas) => Some((*x, *y)),
            _ => None,
        })
        .collect();
    assert_eq!(drags, [(200, 60), (300, 20), (400, -20), (500, -60)]);
    assert_eq!(
        events.last(),
        Some(&AppEvent::Touch { widget: Some(canvas), touch: Touch::DragEnd { x0: 100, y0: 100, x: 500, y: -60 } })
    );
    assert_eq!(events.first(), Some(&AppEvent::Touch { widget: Some(canvas), touch: Touch::Down { x: 100, y: 100 } }));
    assert_eq!(events.len(), 6);
    // The next drag is a new one.
    r.drag((80 + 5, 240 + 5), (80 + 105, 240 + 5));
    assert_eq!(r.events().last(), Some(&AppEvent::Touch { widget: Some(canvas), touch: Touch::DragEnd { x0: 5, y0: 5, x: 105, y: 5 } }));

    // The window takes what no widget takes, with places of the view; the
    // canvas is over it, and so is a list.
    r.view.set_touch_claim(None, true);
    let list = r.view.create(WidgetKind::List, None, Rect::new(0, 1000, m.width, m.row_height));
    r.view.list_set_rows(list, vec![vec![Cell::text("One")]]);
    r.ui.pump();
    r.tap(300, 150);
    r.tap(80 + 1, 240 + 1);
    r.tap(300, 1010);
    assert_eq!(
        r.events(),
        [
            AppEvent::Touch { widget: None, touch: Touch::Down { x: 300, y: 150 } },
            AppEvent::Touch { widget: None, touch: Touch::Tap { x: 300, y: 150 } },
            AppEvent::Touch { widget: Some(canvas), touch: Touch::Down { x: 1, y: 1 } },
            AppEvent::Touch { widget: Some(canvas), touch: Touch::Tap { x: 1, y: 1 } },
            // (A list hears nothing of a finger until it has tapped.)
            AppEvent::Focused { widget: list },
            AppEvent::ListActivated { widget: list, row: 0 },
        ]
    );
    // A list that takes touches is the app's too; a hidden widget takes
    // none; and clearing the view ends the window's claim.
    r.view.set_touch_claim(Some(list), true);
    r.view.set_visible(group, false);
    r.ui.pump();
    r.tap(300, 1010);
    r.tap(80 + 1, 240 + 1);
    assert_eq!(
        r.events(),
        [
            AppEvent::Touch { widget: Some(list), touch: Touch::Down { x: 300, y: 10 } },
            AppEvent::Touch { widget: Some(list), touch: Touch::Tap { x: 300, y: 10 } },
            AppEvent::Touch { widget: None, touch: Touch::Down { x: 81, y: 241 } },
            AppEvent::Touch { widget: None, touch: Touch::Tap { x: 81, y: 241 } },
        ]
    );
    r.view.clear();
    r.ui.pump();
    r.tap(300, 150);
    assert_eq!(r.events(), []);
}

#[test]
fn a_drag_ends_when_the_view_leaves_the_screen() {
    let mut r = Rig::new();
    r.view.set_touch_claim(None, true);
    r.ui.pump();
    r.sim.touch(opendeskphone_state::TouchPhase::Down, 100, 300);
    r.sim.touch(opendeskphone_state::TouchPhase::Move, 100, 400);
    r.ui.pump();
    assert_eq!(
        r.events(),
        [
            AppEvent::Touch { widget: None, touch: Touch::Down { x: 100, y: 300 } },
            AppEvent::Touch { widget: None, touch: Touch::Drag { x0: 100, y0: 300, x: 100, y: 400 } },
        ]
    );
    r.ui.handle().close_app("demo");
    r.ui.pump();
    assert_eq!(
        r.events(),
        [AppEvent::Touch { widget: None, touch: Touch::DragEnd { x0: 100, y0: 300, x: 100, y: 400 } }, AppEvent::Background]
    );
}

#[test]
fn the_phones_with_keys_are_as_they_were() {
    for model in [Model::D60, Model::D40, Model::D70] {
        let mut r = Rig::on(model);
        let list = r.list(3);
        r.view.set_touch_claim(None, true);
        r.ui.pump();
        // No region to touch, no back button, and a touch does nothing
        // (the simulator can make one; the phone has no touchscreen).
        assert!(r.ui.canvas().hits().is_empty(), "{model:?}");
        r.tap(20, i32::from(r.m.title_height) + 5);
        assert_eq!(r.events(), [], "{model:?}");
        // The keys work the list as ever.
        r.sim.tap(Key::Down);
        r.sim.tap(Key::Select);
        r.ui.pump();
        assert_eq!(
            r.events(),
            [AppEvent::ListSelected { widget: list, row: 1 }, AppEvent::ListActivated { widget: list, row: 1 }],
            "{model:?}"
        );
    }
}

/// A picture of an app view on the D80, for the eye:
/// `<target dir>/tmp/ui-snapshots/D80/app-view.png`, beside the pictures
/// of `tests/snapshots.rs`.
#[test]
fn a_picture_of_an_app_view() {
    let mut r = Rig::new();
    let m = r.m;
    // As the SDK's `screen` lays a title out: after the back button.
    let title = r.view.create(WidgetKind::Text, None, Rect::new(m.back_width as i16, 0, m.width - m.back_width, m.title_height));
    r.view.set_font(title, opendeskphone_ui::appview::Font::Title);
    r.view.set_text(title, "Voicemail");
    let list = r.list(14);
    r.view.set_rect(list, Rect::new(0, m.title_height as i16, m.width, 9 * m.row_height));
    r.view.list_select(list, 2);
    let input = r.view.create(WidgetKind::Input, None, Rect::new(16, 1000, m.width - 32, m.row_height));
    r.view.set_text(input, "An input");
    let labels = ["Play", "Delete", "Call", "Info", "Exit"];
    r.view.set_softkeys(None, labels.iter().enumerate().map(|(i, l)| Softkey::new(i as u32, *l)).collect());
    r.ui.pump();
    save_picture(&r, "app-view.png");
    // The back button's arrow is drawn: its corner is not one flat color.
    let corner: Vec<u32> = (0..96 * 96).map(|i| r.ui.canvas().pixel(i % 96, i / 96)).collect();
    assert!(corner.iter().any(|p| *p != corner[0]));
}
