//! What every screen of the touch skin (D80) has from the toolkit: the
//! look of a control under a finger, the title bar with its ways back and
//! home, the softkey buttons side by side, the card of a question, the
//! button styles and the switch, and the text entry screen with its
//! keyboard.

use opendeskphone_hal::Hal;
use opendeskphone_state::*;
use opendeskphone_ui::gfx::{Canvas, IRect};
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::widgets::{self, ButtonStyle};
use opendeskphone_ui::{Hit, Ui, UiConfig};

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
}

impl Rig {
    fn new(model: Model) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| s.phase = Phase::Running);
        store.update::<Accounts, _>(|a| {
            let mut acct = Account::new(AccountId(0));
            acct.label = "100".into();
            a.accounts.push(acct);
        });
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { backlight_timeout_secs: 0, ..Default::default() });
        ui.set_fixed_time(Some((5_000_000, 1_794_607_087)));
        ui.pump();
        Rig { store, ui, sim }
    }
    fn show(&mut self, screen: &str) {
        let p = self.store.send(Command::ShowScreen { name: screen.into() });
        self.ui.pump();
        assert_eq!(p.wait(), Ok(Reply::Done), "{screen}");
    }
    fn screen(&self) -> String {
        self.ui.screen_name()
    }
    /// The region registered with `hit` in the picture on the screen.
    fn region(&self, hit: Hit) -> IRect {
        self.ui.canvas().hit_rect(hit).unwrap_or_else(|| panic!("no region {hit:?} on {}", self.screen()))
    }
    fn has(&self, hit: Hit) -> bool {
        self.ui.canvas().hit_rect(hit).is_some()
    }
    /// A finger down and up in the middle of `r`.
    fn tap_in(&mut self, r: IRect) {
        self.sim.touch_tap((r.x + r.w / 2) as u16, (r.y + r.h / 2) as u16);
        self.ui.pump();
    }
    fn tap(&mut self, hit: Hit) {
        self.tap_in(self.region(hit));
    }
    fn softkeys(&self) -> Vec<(IRect, u32)> {
        self.ui.canvas().hits().iter().filter_map(|(r, h)| if let Hit::Soft(id) = h { Some((*r, *id)) } else { None }).collect()
    }
    fn fill_call_log(&mut self, n: usize) {
        self.store.update::<CallLog, _>(|l| {
            for i in 0..n {
                l.entries.push_back(CallLogEntry {
                    id: i as u64,
                    kind: CallLogKind::Placed,
                    party: Party { name: String::new(), number: format!("2{i:02}"), uri: String::new() },
                    account: AccountId(0),
                    start_unix: 1_794_600_000,
                    duration_secs: 10,
                });
            }
        });
        self.ui.pump();
    }
    /// The picture, for a person to look at: `target/tmp/toolkit/`.
    fn save(&self, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/toolkit");
        std::fs::create_dir_all(&dir).unwrap();
        save_png(self.ui.canvas(), &dir.join(format!("{name}.png")));
    }
}

fn save_png(c: &Canvas, path: &std::path::Path) {
    let (w, h) = (c.width() as u32, c.height() as u32);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for p in c.pixels() {
        data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).unwrap()), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

/// The theme the UI draws in: the touch skin in the palette of the default
/// wallpaper.
fn th() -> &'static Theme {
    Theme::for_model(Model::D80).in_wallpaper(opendeskphone_ui::wallpaper::find_or_default(""))
}

#[test]
fn softkey_buttons_share_their_rows_and_leave_no_place_empty() {
    let th = th();
    // One button is as wide as the screen.
    let one = th.soft_button(0, 1);
    assert_eq!((one.x, one.right()), (th.soft_x0, th.width - th.soft_x0));
    for n in 1..=9usize {
        let all: Vec<IRect> = (0..n).map(|i| th.soft_button(i, n)).collect();
        let rows = th.soft_rows(n);
        for (i, r) in all.iter().enumerate() {
            assert!(r.h >= th.touch_min && r.w >= th.touch_min, "{n}: {r:?}");
            assert!(r.x >= 0 && r.right() <= th.width && r.bottom() <= th.height, "{n}: {r:?}");
            assert!(r.y >= th.height - rows * th.softkey_h, "{n}: {r:?}");
            for o in &all[i + 1..] {
                assert!(r.intersect(o).is_empty(), "{n}: {r:?} {o:?}");
            }
        }
        // Every row begins at the left and ends at the right, and the
        // buttons of a row are as wide as each other, to a pixel.
        let mut ys: Vec<i32> = all.iter().map(|r| r.y).collect();
        ys.dedup();
        assert_eq!(ys.len() as i32, rows, "{n}");
        for y in ys {
            let row: Vec<&IRect> = all.iter().filter(|r| r.y == y).collect();
            assert!(row.len() <= 4, "{n}");
            assert_eq!(row[0].x, th.soft_x0, "{n}");
            assert_eq!(row[row.len() - 1].right(), th.width - th.soft_x0, "{n}");
            let (min, max) = (row.iter().map(|r| r.w).min().unwrap(), row.iter().map(|r| r.w).max().unwrap());
            assert!(max - min <= 1, "{n}");
        }
    }
    // Five are three and two, not four and one.
    assert_eq!((0..5).filter(|i| th.soft_button(*i, 5).y == th.soft_button(0, 5).y).count(), 3);
}

#[test]
fn a_native_screens_softkeys_are_packed_without_more() {
    let mut r = Rig::new(Model::D80);
    // The idle screen has seven by default: all on the screen, in two
    // rows that reach both sides.
    let keys = r.softkeys();
    assert_eq!(keys.len(), 7);
    let th = th();
    assert_eq!(keys.iter().map(|(k, _)| k.x).min(), Some(th.soft_x0));
    assert_eq!(keys.iter().map(|(k, _)| k.right()).max(), Some(th.width - th.soft_x0));
    assert!(keys.iter().all(|(_, id)| *id != u32::MAX));
    // The call log's softkeys on the phones with keys have empty places
    // between them; here its buttons begin at the left edge.
    r.fill_call_log(3);
    r.show("call_log");
    let keys = r.softkeys();
    assert!(!keys.is_empty());
    assert_eq!(keys[0].0.x, th.soft_x0);
    assert_eq!(keys[keys.len() - 1].0.right(), th.width - th.soft_x0);
}

#[test]
fn a_question_is_a_card_with_cancel_at_the_left() {
    let mut r = Rig::new(Model::D80);
    r.fill_call_log(3);
    r.show("call_log");
    // (Clear, the call log's softkey 3.)
    r.tap(Hit::Soft(3));
    assert_eq!(r.screen(), "confirm");
    r.save("confirm");
    let th = th();
    let (action, cancel) = (r.region(Hit::Soft(1)), r.region(Hit::Soft(4)));
    // Two buttons side by side in the middle of the screen, not softkeys
    // at its bottom: Cancel at the left, with room between the two.
    assert_eq!(r.softkeys().len(), 2);
    assert_eq!((action.y, action.h), (cancel.y, cancel.h));
    assert!(cancel.h >= th.touch_min && cancel.w >= 2 * th.touch_min);
    assert!(action.x >= cancel.right() + 16, "{cancel:?} {action:?}");
    assert!(cancel.y > th.height / 4 && cancel.bottom() < th.height * 3 / 4, "{cancel:?}");
    // The card is on the dialogs' fill, and the action is filled.
    let c = r.ui.canvas();
    assert_eq!(c.pixel(cancel.x - 8, cancel.y - 8), th.modal_bg);
    assert_eq!(c.pixel(action.x + 12, action.y + 12), ButtonStyle::Primary.colors(th).0);
    // Cancel leaves everything as it was.
    r.tap(Hit::Soft(4));
    assert_eq!(r.screen(), "call_log");
    assert_eq!(r.store.get::<CallLog>().entries.len(), 3);
    // The phones with keys have the question and Yes and No, as before.
    let mut k = Rig::new(Model::D65);
    k.show("call_log");
    k.sim.tap(Key::Soft(3));
    k.ui.pump();
    assert_eq!(k.screen(), "confirm");
    assert!(k.ui.canvas().hits().is_empty());
}

#[test]
fn the_button_styles_and_the_switch() {
    let th = th();
    let mut c = Canvas::new(800, 1280, false);
    c.clear(th.bg);
    let styles = [ButtonStyle::Secondary, ButtonStyle::Primary, ButtonStyle::Call, ButtonStyle::Destructive, ButtonStyle::On];
    for (i, style) in styles.into_iter().enumerate() {
        let r = IRect::new(40, 40 + i as i32 * 120, 400, 96);
        widgets::button_styled(&mut c, th, r, "Label", style, Hit::Custom(i as u32));
        let (fill, edge, fg) = style.colors(th);
        assert_eq!(c.pixel(r.x + 20, r.y + 20), fill, "{style:?}");
        assert_eq!(c.hit_rect(Hit::Custom(i as u32)), Some(r));
        // The label can be read on the fill, and only the outlined style
        // has an outline.
        assert!(opendeskphone_ui::skin::contrast(fg, fill) >= 3.0, "{style:?}");
        assert_eq!(edge.is_some(), style == ButtonStyle::Secondary);
    }
    assert_eq!(ButtonStyle::Call.colors(th).0, th.sk_go);
    assert_eq!(ButtonStyle::Destructive.colors(th).0, th.sk_stop);
    // A switch: at the right end of its row, in the middle of its height,
    // the knob at the right when on and at the left when off.
    let (w, h) = widgets::switch_size(th);
    let row = |y: i32| IRect::new(0, y, 760, 96);
    let (on, off) = (widgets::switch(&mut c, th, row(700), true), widgets::switch(&mut c, th, row(800), false));
    assert_eq!(on, IRect::new(760 - w, 700 + (96 - h) / 2, w, h));
    assert_eq!(off, IRect::new(760 - w, 800 + (96 - h) / 2, w, h));
    let mid = |r: IRect, right: bool| c.pixel(if right { r.right() - h / 2 } else { r.x + h / 2 }, r.y + h / 2);
    // On: the track is filled in the text's color, with a dark knob.
    assert_eq!(mid(on, false), th.fg);
    assert_eq!(mid(on, true), th.bg);
    // Off: an outline, with the knob at the left.
    assert_eq!(mid(off, true), th.sk_bg);
    assert_eq!(mid(off, false), th.secondary);
    // It registers nothing: its row is what is touched.
    assert_eq!(c.hits().len(), styles.len());
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/toolkit");
    std::fs::create_dir_all(&dir).unwrap();
    save_png(&c, &dir.join("buttons-and-switch.png"));
}

/// Whether any pixel of `r` differs between the two pictures.
fn differ(a: &[u32], b: &[u32], r: IRect) -> bool {
    (r.y..r.bottom()).any(|y| (r.x..r.right()).any(|x| a[(y * 800 + x) as usize] != b[(y * 800 + x) as usize]))
}

#[test]
fn what_a_finger_is_on_looks_pressed_until_it_is_lifted() {
    let mut r = Rig::new(Model::D80);
    r.show("menu");
    let rest = r.ui.canvas().pixels().to_vec();
    let (row1, row2) = (r.region(Hit::Row { list: 0, row: 1 }), r.region(Hit::Row { list: 0, row: 2 }));
    // A finger down on a row: the row looks pressed, and nothing else
    // does.  Nothing has been done yet.
    r.sim.touch(TouchPhase::Down, 400, (row1.y + 40) as u16);
    r.ui.pump();
    let down = r.ui.canvas().pixels().to_vec();
    assert_eq!(r.screen(), "menu");
    assert!(differ(&rest, &down, row1));
    assert_ne!(down[(row1.y * 800 + 790) as usize], rest[(row1.y * 800 + 790) as usize], "the whole row");
    assert!(!differ(&rest, &down, row2));
    assert!(!differ(&rest, &down, IRect::new(0, 0, 800, row1.y)));
    // It stays so while the finger stays (and moves no farther than a
    // tap may).
    r.sim.touch(TouchPhase::Move, 404, (row1.y + 44) as u16);
    r.ui.pump();
    assert!(r.ui.canvas().pixels() == &down[..]);
    // A touch that is canceled: as before, and nothing done.
    r.sim.touch(TouchPhase::Cancel, 404, (row1.y + 44) as u16);
    r.ui.pump();
    assert!(r.ui.canvas().pixels() == &rest[..]);
    assert_eq!(r.screen(), "menu");
    // A finger that drags presses nothing.
    r.sim.touch(TouchPhase::Down, 400, (row1.y + 40) as u16);
    r.ui.pump();
    assert!(differ(&rest, r.ui.canvas().pixels(), row1));
    r.sim.touch(TouchPhase::Move, 400, (row1.y + 90) as u16);
    r.ui.pump();
    assert!(!differ(&rest, r.ui.canvas().pixels(), row1));
    r.sim.touch(TouchPhase::Up, 400, (row1.y + 90) as u16);
    r.ui.pump();
    assert_eq!(r.screen(), "menu");
    // The buttons of the title bar are pressed like anything else.
    let back = r.region(Hit::Key(Key::Cancel));
    r.sim.touch(TouchPhase::Down, (back.x + 48) as u16, (back.y + 48) as u16);
    r.ui.pump();
    assert!(differ(&rest, r.ui.canvas().pixels(), back));
    // Lifted, the finger has tapped: the back button goes back.
    r.sim.touch(TouchPhase::Up, (back.x + 48) as u16, (back.y + 48) as u16);
    r.ui.pump();
    assert_eq!(r.screen(), "idle");
    // A finger where there is nothing to touch shows nothing.
    r.show("menu");
    assert_eq!(r.ui.canvas().hit_at(400, 1250), None);
    r.sim.touch(TouchPhase::Down, 400, 1250);
    r.ui.pump();
    assert!(r.ui.canvas().pixels() == &rest[..]);
    r.sim.touch(TouchPhase::Up, 400, 1250);
    r.ui.pump();
}

#[test]
fn a_pressed_button_has_its_own_shape() {
    let mut r = Rig::new(Model::D80);
    // The volume popup's buttons (the volume keys of a phone that has
    // them open it).
    let sent = r.store.send(Command::InjectKey { key: Key::VolumeUp, down: true });
    r.ui.pump();
    assert_eq!(sent.wait(), Ok(Reply::Done));
    let plus = r.region(Hit::Key(Key::VolumeUp));
    let rest = r.ui.canvas().pixels().to_vec();
    r.sim.touch(TouchPhase::Down, (plus.x + 10) as u16, (plus.y + 40) as u16);
    r.ui.pump();
    r.save("pressed-button");
    let c = r.ui.canvas();
    let at = |x: i32, y: i32| (y * 800 + x) as usize;
    // The button's fill is lighter (the theme is a dark one), all of it;
    // the corner of its rectangle, outside its round shape, is as it was.
    let lum = |p: u32| ((p >> 16) & 255) + ((p >> 8) & 255) + (p & 255);
    for (x, y) in [(plus.x + 10, plus.y + 40), (plus.right() - 6, plus.bottom() - 20), (plus.x + plus.w / 2, plus.y + 4)] {
        assert!(lum(c.pixels()[at(x, y)]) > lum(rest[at(x, y)]), "({x}, {y})");
    }
    assert_eq!(c.pixels()[at(plus.x, plus.y)], rest[at(plus.x, plus.y)]);
    assert_eq!(c.pixels()[at(plus.right() - 1, plus.bottom() - 1)], rest[at(plus.right() - 1, plus.bottom() - 1)]);
    // The other button is as it was.
    let minus = r.region(Hit::Key(Key::VolumeDown));
    assert!(!differ(&rest, c.pixels(), minus));
    r.sim.touch(TouchPhase::Cancel, 0, 0);
    r.ui.pump();
    assert!(r.ui.canvas().pixels() == &rest[..]);
}

#[test]
fn the_pressed_look_is_for_controls() {
    let th = th();
    let button = IRect::new(100, 100, 300, 96);
    let draw = |c: &mut Canvas| {
        c.clear_hits();
        c.clear(th.bg);
        // A surface under everything, a control, a region that shows
        // nothing, and a region that does nothing.
        c.hit(IRect::new(0, 0, 800, 1280), Hit::Custom(1));
        widgets::button_styled(c, th, button, "Go", ButtonStyle::On, Hit::Custom(2));
        c.hit_quiet(IRect::new(100, 300, 300, 96), Hit::Custom(3));
        c.hit(IRect::new(100, 500, 300, 96), Hit::Nothing);
    };
    let mut c = Canvas::new(800, 1280, false);
    draw(&mut c);
    let rest = c.pixels().to_vec();
    // A light control is mixed toward black, a dark one toward white; the
    // way taken is returned, and can be given.
    c.set_press(Some((150, 150)));
    assert!(c.pressed_in(button) && !c.pressed_in(IRect::new(0, 0, 100, 100)));
    assert_eq!(c.show_press(Hit::Custom(2), None), Some(false));
    assert!(differ(&rest, c.pixels(), button));
    assert!(!differ(&rest, c.pixels(), IRect::new(0, 200, 800, 1080)));
    draw(&mut c);
    assert_eq!(c.show_press(Hit::Custom(2), Some(true)), Some(true));
    // Not when the picture has something else there now.
    draw(&mut c);
    assert_eq!(c.show_press(Hit::Custom(7), None), None);
    // Not a surface, a region that shows nothing, or one that does
    // nothing; and nothing without a finger.
    for (at, hit) in [((600, 900), Hit::Custom(1)), ((150, 350), Hit::Custom(3)), ((150, 550), Hit::Nothing)] {
        c.set_press(Some(at));
        assert_eq!(c.show_press(hit, None), None, "{hit:?}");
    }
    c.set_press(None);
    assert_eq!(c.show_press(Hit::Custom(2), None), None);
    assert!(c.pixels() == &rest[..]);
}

#[test]
fn every_title_bar_has_back_at_the_left_and_home_at_the_right() {
    let mut r = Rig::new(Model::D80);
    let th = th();
    for screen in ["menu", "about", "call_log"] {
        r.show(screen);
        let (back, home) = (r.region(Hit::Key(Key::Cancel)), r.region(Hit::Home));
        assert_eq!(back, IRect::new(0, 0, th.back_w, th.title_h), "{screen}");
        assert_eq!(home, IRect::new(th.width - th.back_w, 0, th.back_w, th.title_h), "{screen}");
        assert!(back.w >= th.touch_min && back.h >= th.touch_min);
        // Both are on the title bar's ground.
        assert_eq!(r.ui.canvas().pixel(2, 2), th.title_bg, "{screen}");
        assert_eq!(r.ui.canvas().pixel(th.width - 3, 2), th.title_bg, "{screen}");
    }
    r.save("title-bar");
    // Home, from however deep: the home screen, in one touch.
    r.show("menu");
    r.tap(Hit::Row { list: 0, row: 1 });
    assert_ne!(r.screen(), "menu");
    assert_ne!(r.screen(), "idle");
    r.tap(Hit::Home);
    assert_eq!(r.screen(), "idle");
    // The home screen has no title bar's buttons: there is nowhere to go.
    assert!(!r.has(Hit::Home));
    // A call that is up stays on the screen: home is the call.
    let id = r.store.next_call_id();
    r.store.update::<Calls, _>(|c| {
        let mut call = Call::new(id, AccountId(0), Direction::Incoming, Party { number: "200".into(), ..Default::default() });
        call.state = CallState::Active;
        c.calls.push(call);
        c.focused = Some(id);
    });
    r.ui.pump();
    assert_eq!(r.screen(), "call");
    r.show("menu");
    assert_eq!(r.screen(), "menu");
    r.tap(Hit::Home);
    assert_eq!(r.screen(), "call");
    // The phones with keys have no such buttons.
    let mut k = Rig::new(Model::D65);
    k.show("menu");
    assert!(k.ui.canvas().hits().is_empty());
}
