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
