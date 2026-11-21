//! The built-in wallpapers and their palettes: each wallpaper draws the
//! same picture every time; every palette's text can be read (measured
//! contrast of every pairing of roles that carries text or state, and of
//! the idle screen's text against the wallpaper behind it); the colors
//! with a meaning keep it; an unknown name is the default; a new
//! wallpaper re-themes every screen at once, apps included; and the
//! first wallpaper keeps the skin's palette.  The wallpapers that move do
//! so on the idle screen and in the theme selector, and nowhere else; the
//! theme selector is on a softkey.

use opendeskphone_hal::{Hal, Screen};
use opendeskphone_state::*;
use opendeskphone_state::{ThemeName, DEFAULT_THEME, THEMES};
use opendeskphone_ui::appview::{AppEvent, AppView, ThemeColor};
use opendeskphone_ui::gfx::{IRect, Rgb};
use opendeskphone_ui::skin::Theme;
use opendeskphone_ui::wallpaper::{self, Family, Picture, Scheme, Wallpaper, HEIGHT, WIDTH};
use opendeskphone_ui::{Ui, UiConfig};

/// 2026-11-12 18:15:00 UTC: "11:15 AM", "Thu, Nov 12" seven hours west.
const UNIX: u64 = 1_794_507_300;

/// The least contrast ratio text may have against what is behind it (the
/// WCAG figure for body text; `skin::TEXT_CONTRAST`).  The pairings of
/// roles and their limits are `Palette::pairings`.
const TEXT: f64 = 4.5;

/// Relative luminance of an sRGB color (WCAG 2).
fn luminance(c: Rgb) -> f64 {
    let lin = |v: u32| {
        let v = f64::from(v & 255) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c >> 16) + 0.7152 * lin(c >> 8) + 0.0722 * lin(c)
}

/// Contrast ratio of two colors (WCAG 2): 1 to 21.
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

struct Rig {
    store: Store,
    ui: Ui,
    sim: opendeskphone_hal::sim::SimHandle,
    config: CommandReceiver,
}

impl Rig {
    /// A D65 at its idle screen with status icons, a count, and two
    /// line-key tabs.
    fn new(cfg: UiConfig) -> Rig {
        Rig::with_model(Model::D65, cfg)
    }

    fn with_model(model: Model, cfg: UiConfig) -> Rig {
        let mut hal = Hal::sim(model);
        let sim = hal.sim.clone().unwrap();
        let store = Store::new(model);
        store.update::<System, _>(|s| {
            s.phase = Phase::Running;
            s.time_synced = true;
        });
        store.update::<Network, _>(|n| {
            n.phase = NetPhase::Up;
            n.lan.up = true;
        });
        store.update::<Accounts, _>(|a| {
            for (i, label) in ["Jordan 101", "Front Desk"].iter().enumerate() {
                let mut acct = Account::new(AccountId(i as u8));
                acct.label = label.to_string();
                acct.registration = Registration::Registered { host: ActiveHost::Primary, expires_mono_ms: u64::MAX };
                a.accounts.push(acct);
            }
        });
        store.update::<CallLog, _>(|l| l.missed_unseen = 2);
        store.update::<Features, _>(|f| f.dnd = true);
        let config = store.handle(Domain::Config);
        let mut ui = Ui::from_hal(&mut hal, store.clone(), UiConfig { utc_offset_secs: -7 * 3600, backlight_timeout_secs: 0, ..cfg });
        ui.set_fixed_time(Some((5_000_000, UNIX)));
        ui.pump();
        Rig { store, ui, sim, config }
    }

    fn named(wallpaper: &str) -> Rig {
        Rig::new(UiConfig { theme: wallpaper.into(), ..UiConfig::default() })
    }

    fn tap(&mut self, key: Key) {
        self.sim.tap(key);
        self.ui.pump();
    }

    fn show(&mut self, screen: &str) {
        let p = self.store.send(Command::ShowScreen { name: screen.into() });
        self.ui.pump();
        assert_eq!(p.wait(), Ok(Reply::Done), "{screen}");
    }

    /// The main screen as `0x00RRGGBB`.
    fn frame(&mut self) -> Vec<u32> {
        self.ui.pump();
        let frame = self.sim.frame(Screen::Main).expect("main frame");
        if frame.info.format.bytes_per_pixel() == 4 {
            frame.pixels.as_chunks::<4>().0.iter().map(|p| u32::from_le_bytes(*p) & 0xFF_FFFF).collect()
        } else {
            frame.pixels.iter().map(|p| u32::from(*p) * 0x010101).collect()
        }
    }

    fn commands(&self) -> Vec<Command> {
        let mut v = Vec::new();
        while let Some(env) = self.config.try_recv() {
            v.push(env.command.clone());
            env.done();
        }
        v
    }
}

fn idle_frame(wallpaper: &str) -> Vec<u32> {
    Rig::named(wallpaper).frame()
}

/// The settings menu (a title bar, a list with its first row selected,
/// softkeys) with `wallpaper` selected.
fn menu_frame(wallpaper: &str) -> Vec<u32> {
    let mut r = Rig::named(wallpaper);
    r.show("settings");
    r.frame()
}

fn at(pixels: &[u32], x: i32, y: i32) -> Rgb {
    pixels[(y * WIDTH + x) as usize]
}

#[test]
fn the_list_is_well_formed() {
    let all = wallpaper::all();
    assert!((30..=45).contains(&all.len()), "{} wallpapers", all.len());
    // The default theme is the configuration's default; the first in the
    // list is the picture the firmware first had.
    assert_eq!(wallpaper::default().id, DEFAULT_THEME);
    assert_eq!(wallpaper::first().id, all[0].id);
    for (i, w) in all.iter().enumerate() {
        assert!(!w.id.is_empty() && w.id.bytes().all(|c| c.is_ascii_lowercase() || c == b'-'), "{}", w.id);
        assert!(!w.name.is_empty() && w.name.len() <= 14, "{}", w.name);
        assert!(all[..i].iter().all(|o| o.id != w.id && o.name != w.name), "{} is listed twice", w.id);
        assert_eq!(wallpaper::find(w.id).map(|f| f.id), Some(w.id));
    }
    // The list by name that the configuration and the web UI use is this
    // list.
    let names: Vec<ThemeName> = all.iter().map(|w| ThemeName { id: w.id, name: w.name, family: w.family.name() }).collect();
    assert_eq!(names, THEMES);
    // Grouped by family, in the families' order, none empty.
    let order: Vec<usize> = all.iter().map(|w| Family::ALL.iter().position(|f| *f == w.family).unwrap()).collect();
    assert!(order.is_sorted(), "{order:?}");
    for (i, f) in Family::ALL.iter().enumerate() {
        assert!(order.contains(&i), "no wallpaper in {}", f.name());
    }
}

/// The first wallpaper's palette is the color skin's own, role for
/// role, and its theme is the skin's theme itself: with it, every screen
/// is drawn as it was before there were palettes.
#[test]
fn the_first_wallpaper_keeps_the_skins_palette() {
    let skin = Theme::for_model(Model::D65);
    let default = wallpaper::first();
    assert_eq!(default.palette(), skin.palette());
    assert!(std::ptr::eq(Theme::for_wallpaper(default), skin));
    // The stock values, by measurement (see `skin.rs`).
    let p = default.palette();
    assert_eq!((p.bg, p.fg, p.title_bg, p.sel_bg), (0x212121, 0xEEEEEE, 0x014174, 0x01579B));
    assert_eq!((p.sk_bg, p.sk_edge, p.tab_bg, p.card), (0x2E2E2E, 0x757575, 0x424242, 0x01579B));
    // A palette file's roles are these fields, by these names.
    let mut q = p;
    for (name, color) in p.roles() {
        assert!(q.set(name, color ^ 0xFFFFFF), "{name}");
    }
    assert!(!q.set("no_such_role", 0));
    assert!(p.roles().iter().zip(q.roles()).all(|(a, b)| a.0 == b.0 && a.1 == b.1 ^ 0xFFFFFF));
}

#[test]
fn an_unknown_name_is_the_default() {
    let default = wallpaper::default();
    for name in ["", "  ", "no-such-wallpaper", "harbor.png", "DUSK!"] {
        assert!(wallpaper::find(name).is_none(), "{name:?}");
        assert_eq!(wallpaper::find_or_default(name).id, default.id, "{name:?}");
    }
    // Case and surrounding blanks do not matter.
    assert_eq!(wallpaper::find(" Dusk ").map(|w| w.id), Some("dusk"));
    assert_eq!(wallpaper::find_or_default("DEEP-SEA").id, "deep-sea");
    // The screens, too: an unknown name shows the default wallpaper in
    // the default palette.
    assert_eq!(idle_frame("no-such-wallpaper"), idle_frame(default.id));
    assert_eq!(idle_frame(""), idle_frame(default.id));
    assert_eq!(menu_frame("no-such-wallpaper"), menu_frame(""));
}

#[test]
fn every_wallpaper_draws_the_same_picture_every_time() {
    for w in wallpaper::all() {
        let first = w.render();
        assert_eq!(first.len(), (WIDTH * HEIGHT) as usize, "{}", w.id);
        assert!(first.iter().all(|p| *p <= 0xFF_FFFF), "{}", w.id);
        assert_eq!(first, w.render(), "{} differs between two renders", w.id);
        // The kept copy is the same picture, whichever was kept before.
        assert!(w.with_pixels(|kept| kept == first.as_slice()), "{}", w.id);
    }
}

/// The least contrast of `fg` against the pixels of `r`.
fn least_contrast(pixels: &[u32], r: IRect, fg: Rgb) -> f64 {
    let mut least = f64::MAX;
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            least = least.min(contrast(fg, at(pixels, x, y)));
        }
    }
    least
}

/// Where the idle screen draws text on the wallpaper itself: (element,
/// area).  The areas are the whole of where each can be, not the ink of
/// one particular time.
fn text_areas(th: &Theme) -> [(&'static str, IRect); 4] {
    [
        ("status icons", th.header_rect_to(th.idle_tab_x)),
        // Stock positions: digits x 68..222 (to 290 with the meridiem),
        // y 89..152.
        ("clock", IRect::new(30, 84, 266, 72)),
        ("date", IRect::new(30, 176, 236, 28)),
        ("app idle view", IRect::new(0, th.soft_y - 1 - 36, th.idle_tab_x, 36)),
    ]
}

fn hue(c: Rgb) -> &'static str {
    let (r, g, b) = ((c >> 16) as i32, ((c >> 8) & 255) as i32, (c & 255) as i32);
    if r > g + 40 && r > b + 40 && g < b + 60 {
        "red"
    } else if g > r + 20 && g > b + 20 {
        "green"
    } else if r > b + 60 && g > b + 30 {
        "amber"
    } else {
        "other"
    }
}

#[test]
fn every_palette_can_be_read() {
    let mut report = String::new();
    let mut failures = Vec::new();
    for w in wallpaper::all() {
        let th = Theme::for_wallpaper(w);
        assert_eq!(th.palette(), w.palette(), "{}", w.id);
        let pixels = w.render();
        let (mut worst, mut worst_what) = (f64::MAX, "");
        let mut check = |what: &'static str, ratio: f64, least: f64| {
            if ratio / least < worst {
                (worst, worst_what) = (ratio / least, what);
            }
            if ratio < least {
                failures.push(format!("{}: {what}: {ratio:.2}, under {least}", w.id));
            }
        };
        // The idle screen's text, on the wallpaper itself: against every
        // pixel it can be on.
        let mut idle = f64::MAX;
        for (what, area) in text_areas(th) {
            let ratio = least_contrast(&pixels, area, th.wallpaper_fg);
            idle = idle.min(ratio);
            check(what, ratio, TEXT);
        }
        // Every pairing of roles that carries text or a state.
        let pairings = th.palette().pairings();
        assert!(pairings.len() >= 22);
        for (what, fg, bg, least) in pairings {
            check(what, contrast(fg, bg), f64::from(least));
        }
        assert!(th.palette().low_contrast().is_empty(), "{}: {:?}", w.id, th.palette().low_contrast());
        // The colors with a meaning keep it.
        for (what, color, want) in [
            ("stop softkey", th.sk_stop, "red"),
            ("error", th.error, "red"),
            ("go softkey", th.sk_go, "green"),
            ("good", th.good, "green"),
            ("ringing call card", th.card_ringing, "green"),
            ("warning", th.warn, "amber"),
        ] {
            if hue(color) != want {
                failures.push(format!("{}: {what} is {color:06X}, not {want}", w.id));
            }
        }
        report.push_str(&format!("{:12} idle text {idle:5.2}; closest to its limit: {worst_what} ({:.2} x)\n", w.id, worst));
    }
    println!("{report}");
    assert!(failures.is_empty(), "palettes that cannot be read:\n{}", failures.join("\n"));
}

/// A palette file's palette: derived as the built-in ones are, role by
/// role where it says, the skin's own where it says nothing; and a
/// palette that cannot be read is reported, not refused.
#[test]
fn palettes_from_palette_files() {
    let skin = Theme::for_model(Model::D65).palette();
    // Nothing given: the skin's own palette.
    assert_eq!(wallpaper::palette_from(false, None, None, &[]), (skin, vec![]));
    // The two colors: the built-in rule.
    let evergreen = wallpaper::find("evergreen").unwrap();
    let Scheme::Dark { primary, surface } = evergreen.scheme else { panic!("evergreen is dark") };
    assert_eq!(wallpaper::palette_from(false, Some(primary), Some(surface), &[]).0, evergreen.palette());
    let stone = wallpaper::find("stone").unwrap();
    let Scheme::Light { primary, surface } = stone.scheme else { panic!("stone is light") };
    assert_eq!(wallpaper::palette_from(true, Some(primary), Some(surface), &[]).0, stone.palette());
    // One of the two: the other is the skin's.
    let p = wallpaper::palette_from(false, Some(0x1F6E4E), None, &[]).0;
    assert_eq!((p.sel_bg, p.bg, p.sk_bg), (0x1F6E4E, skin.bg, skin.sk_bg));
    let p = wallpaper::palette_from(false, None, Some(0x0F1E18), &[]).0;
    assert_eq!((p.sel_bg, p.bg), (skin.sel_bg, 0x0F1E18));
    let p = wallpaper::palette_from(true, None, None, &[]).0;
    assert!(p.is_light() && p.low_contrast().is_empty(), "{:?}", p.low_contrast());
    // Text on the wallpaper is the text color, unless it is given.
    let p = wallpaper::palette_from(false, None, None, &[("fg", 0xC0C0C0)]).0;
    assert_eq!((p.fg, p.wallpaper_fg), (0xC0C0C0, 0xC0C0C0));
    let p = wallpaper::palette_from(false, None, None, &[("wallpaper_fg", 0x101010), ("fg", 0xC0C0C0)]).0;
    assert_eq!((p.fg, p.wallpaper_fg), (0xC0C0C0, 0x101010));
    // Roles by name go over the derived colors; unknown names come back.
    let (p, unknown) = wallpaper::palette_from(false, Some(0x1F6E4E), Some(0x0F1E18), &[("title_bg", 0x14503A), ("sparkle", 1), ("accent", 0x8FD6B4)]);
    assert_eq!((p.title_bg, p.accent, p.sel_bg), (0x14503A, 0x8FD6B4, 0x1F6E4E));
    assert_eq!(unknown, ["sparkle"]);
    // Text that cannot be read is said, with the roles to look at.
    let (p, _) = wallpaper::palette_from(false, None, None, &[("fg", 0x303030), ("sel_bg", 0xF0F0F0)]);
    let low = p.low_contrast();
    assert!(low.iter().any(|l| l.starts_with("text (fg) on the background (bg): contrast 1.")), "{low:?}");
    assert!(low.iter().any(|l| l.starts_with("selected row (sel_fg on sel_bg)")), "{low:?}");
}

/// The same on the screens as drawn, for every wallpaper: the idle screen
/// and a menu are in the wallpaper's palette.
#[test]
fn every_screen_is_drawn_in_the_wallpapers_palette() {
    for w in wallpaper::all() {
        let th = Theme::for_wallpaper(w);
        let (frame, bare) = (idle_frame(w.id), w.render());
        // Left of the tabs and above the softkeys the idle screen is the
        // wallpaper with text on it: the pixels at full ink are the text
        // color, on wallpaper pixels it contrasts with.
        let (mut ink, mut least) = (0, f64::MAX);
        for y in 0..th.soft_y {
            for x in 0..th.idle_tab_x {
                if at(&frame, x, y) == th.wallpaper_fg && at(&bare, x, y) != th.wallpaper_fg {
                    ink += 1;
                    least = least.min(contrast(th.wallpaper_fg, at(&bare, x, y)));
                }
            }
        }
        assert!(ink > 400, "{}: {ink} pixels of text in its text color", w.id);
        assert!(least >= TEXT, "{}: text on a pixel with contrast {least:.2}", w.id);
        // A line-key tab: its fill and its label.
        let tab = th.tab_rect_from(th.idle_tab_x, 0);
        assert_eq!(at(&frame, tab.x + 5, tab.y + 3), th.tab_bg, "{}: tab fill", w.id);
        let label = (tab.y..tab.bottom()).flat_map(|y| (tab.x + 14..tab.right() - 40).map(move |x| (x, y)));
        assert!(label.filter(|(x, y)| at(&frame, *x, *y) == th.tab_fg).count() > 40, "{}: tab label", w.id);
        // A softkey: its fill, its outline, and its label.
        let key = th.softkey_rect(0);
        assert_eq!(at(&frame, key.x + 1, key.y + 3), th.sk_bg, "{}: softkey fill", w.id);
        assert_eq!(at(&frame, key.x, key.y + 6), th.sk_edge, "{}: softkey outline", w.id);
        let label = (key.y..key.bottom()).flat_map(|y| (key.x..key.right()).map(move |x| (x, y)));
        assert!(label.filter(|(x, y)| at(&frame, *x, *y) == th.sk_fg).count() > 40, "{}: softkey label", w.id);
        // Everything else is the wallpaper: under the tabs' rows and
        // between the softkeys.
        assert_eq!(at(&frame, 400, 200), at(&bare, 400, 200), "{}", w.id);
        assert_eq!(at(&frame, 4, 250), at(&bare, 4, 250), "{}", w.id);

        // A menu: the title bar, the selected row, the background, and
        // the softkeys.
        let menu = menu_frame(w.id);
        assert_eq!(at(&menu, 470, 5), th.title_bg, "{}: title bar", w.id);
        assert_eq!(at(&menu, 440, th.title_h + 8), th.sel_bg, "{}: selected row", w.id);
        assert_eq!(at(&menu, 440, 200), th.bg, "{}: menu background", w.id);
        assert_eq!(at(&menu, 4, 250), th.bg, "{}: beside the softkeys", w.id);
        assert_eq!(at(&menu, key.x + 1, key.y + 3), th.sk_bg, "{}: menu softkey", w.id);
        let count = |c: Rgb| menu.iter().filter(|p| **p == c).count();
        assert!(count(th.title_fg) > 40 && count(th.fg) > 100, "{}: menu text", w.id);
    }
}

/// A new wallpaper is a new palette at once, without a restart, on the
/// screen that is up and for apps: `theme_color` answers in the new
/// palette, and an app in the foreground is told to draw again.
#[test]
fn a_new_wallpaper_rethemes_at_once() {
    let mut r = Rig::named("harbor");
    let handle = r.ui.handle();
    r.show("settings");
    let skin = Theme::for_model(Model::D65);
    assert_eq!(at(&r.frame(), 470, 5), skin.title_bg);
    let dusk = Theme::for_wallpaper(wallpaper::find("dusk").unwrap());
    assert_ne!(dusk.title_bg, skin.title_bg);
    handle.set_config(UiConfig { theme: "dusk".into(), utc_offset_secs: -7 * 3600, backlight_timeout_secs: 0, ..UiConfig::default() });
    let frame = r.frame();
    assert_eq!(at(&frame, 470, 5), dusk.title_bg);
    assert_eq!(at(&frame, 440, dusk.title_h + 8), dusk.sel_bg);
    assert_eq!(frame, menu_frame("dusk"), "the same screen as after a start with that wallpaper");

    // An app: opened under the new palette, it gets that palette's colors.
    let rgb = |c: Rgb| ((c >> 16) as u8, (c >> 8) as u8, c as u8);
    let (view, events) = AppView::new("probe", Model::D65);
    assert_eq!(view.theme_color(ThemeColor::TitleBackground), rgb(skin.title_bg), "before it is on a screen");
    handle.open_app(view.clone());
    r.ui.pump();
    assert_eq!(r.ui.screen_name(), "app:probe");
    assert_eq!(view.theme_color(ThemeColor::TitleBackground), rgb(dusk.title_bg));
    assert_eq!(view.theme_color(ThemeColor::SelectBackground), rgb(dusk.sel_bg));
    while events.try_recv().is_some() {}
    // The palette changes under it: the colors follow, and the app is
    // told it is in the foreground (again), on which apps draw.
    let tides = Theme::for_wallpaper(wallpaper::find("tides").unwrap());
    handle.set_config(UiConfig { theme: "tides".into(), utc_offset_secs: -7 * 3600, backlight_timeout_secs: 0, ..UiConfig::default() });
    r.ui.pump();
    assert_eq!(view.theme_color(ThemeColor::TitleBackground), rgb(tides.title_bg));
    assert_eq!(view.theme_color(ThemeColor::Background), rgb(tides.bg));
    assert_eq!(events.try_recv(), Some(AppEvent::Foreground));
    // The same configuration again changes nothing and tells no one.
    handle.set_config(UiConfig { theme: "tides".into(), utc_offset_secs: -7 * 3600, backlight_timeout_secs: 0, ..UiConfig::default() });
    r.ui.pump();
    assert_eq!(events.try_recv(), None);
    // A view made by the UI (as the app host makes them) has the theme's
    // colors from the start, before it is on screen, for every role: an
    // app asks for its colors as it starts.  They follow a new theme
    // while the view is still not on screen.
    let roles = |th: &'static Theme| {
        [
            (ThemeColor::Background, th.bg),
            (ThemeColor::Foreground, th.fg),
            (ThemeColor::TitleBackground, th.title_bg),
            (ThemeColor::TitleForeground, th.title_fg),
            (ThemeColor::SelectBackground, th.sel_bg),
            (ThemeColor::SelectForeground, th.sel_fg),
            (ThemeColor::Secondary, th.secondary),
            (ThemeColor::Border, th.border),
        ]
    };
    let (made, _events) = handle.new_view("early", Model::D65);
    for (role, color) in roles(tides) {
        assert_eq!(made.theme_color(role), rgb(color), "{role:?} before the view is on screen");
    }
    assert_ne!(tides.bg, skin.bg);
    handle.set_config(UiConfig { theme: "merlot".into(), utc_offset_secs: -7 * 3600, backlight_timeout_secs: 0, ..UiConfig::default() });
    r.ui.pump();
    let merlot = Theme::for_wallpaper(wallpaper::find("merlot").unwrap());
    for (role, color) in roles(merlot) {
        assert_eq!(made.theme_color(role), rgb(color), "{role:?} after the theme changed");
    }
    // The default theme, with nothing configured, is no exception.
    let mut fresh = Rig::named("");
    let (made, _events) = fresh.ui.handle().new_view("early", Model::D65);
    fresh.ui.pump();
    assert_eq!(made.theme_color(ThemeColor::Background), rgb(tides.bg));
    // A gray model has its skin's colors, as ever.
    let gray = Rig::with_model(Model::D50, UiConfig::default());
    let (made, _events) = gray.ui.handle().new_view("early", Model::D50);
    assert_eq!(made.theme_color(ThemeColor::Background), (255, 255, 255));
}

/// The wallpaper selector, in the Display menu on the color models: it
/// is the idle screen over the wallpaper the selection is on, in that
/// wallpaper's palette, with a bubble at the bottom that names it between
/// two arrows; Left and Right step through the wallpapers; Select saves.
#[test]
fn the_theme_selector_is_the_idle_screen_with_a_bubble() {
    let mut r = Rig::named("harbor");
    r.show("settings");
    r.tap(Key::Digit(3)); // Display
    assert_eq!(r.ui.screen_name(), "display");
    r.tap(Key::Digit(4)); // Theme
    assert_eq!(r.ui.screen_name(), "theme");
    let all = wallpaper::all();
    let skin = Theme::for_model(Model::D65);
    // The bubble: centered under the date, left of the tabs, above the
    // softkeys.
    let bubble = IRect::new(38, 210, 220, 28);
    // The frame is the idle screen over wallpaper `w`, pixel for pixel,
    // but for the bubble and the softkeys' labels (Select and Back here).
    let is_idle_with_bubble = |frame: &[u32], w: &'static Wallpaper| {
        let th = Theme::for_wallpaper(w);
        let idle = idle_frame(w.id);
        for y in 0..th.soft_y {
            for x in 0..WIDTH {
                let inside = x >= bubble.x && x < bubble.right() && y >= bubble.y && y < bubble.bottom();
                if !inside {
                    assert_eq!(at(frame, x, y), at(&idle, x, y), "{}: at {x},{y}", w.id);
                }
            }
        }
        // The softkeys are the idle screen's shapes in the same colors.
        for i in 0..4 {
            let key = th.softkey_rect(i);
            assert_eq!(at(frame, key.x + 1, key.y + 3), th.sk_bg, "{}: softkey {i}", w.id);
            assert_eq!(at(frame, key.x, key.y + 6), th.sk_edge, "{}: softkey {i}", w.id);
        }
        assert_eq!(at(frame, 4, 250), at(&idle, 4, 250), "{}: beside the softkeys", w.id);
        // The bubble: the softkeys' fill with their outline around it, the
        // name and the two arrows in their label color, and the wallpaper
        // at its corners, which are round.
        let mid = bubble.y + bubble.h / 2;
        assert_eq!(at(frame, bubble.x + 40, bubble.y + 3), th.sk_bg, "{}: bubble fill", w.id);
        assert_eq!(at(frame, bubble.x + 60, bubble.y), th.sk_edge, "{}: bubble outline", w.id);
        assert_eq!(at(frame, bubble.x, bubble.y), at(&idle, bubble.x, bubble.y), "{}: round corner", w.id);
        assert_eq!(at(frame, bubble.x + 17, mid), th.sk_fg, "{}: left arrow", w.id);
        assert_eq!(at(frame, bubble.right() - 18, mid), th.sk_fg, "{}: right arrow", w.id);
        // The arrows point outward: each is one pixel at its tip.
        assert_eq!(at(frame, bubble.x + 14, mid - 4), th.sk_bg, "{}: left arrow's tip", w.id);
        assert_eq!(at(frame, bubble.right() - 15, mid - 4), th.sk_bg, "{}: right arrow's tip", w.id);
        let name = (bubble.y..bubble.bottom()).flat_map(|y| (bubble.x + 30..bubble.right() - 30).map(move |x| (x, y)));
        assert!(name.filter(|(x, y)| at(frame, *x, *y) == th.sk_fg).count() > 60, "{}: the name", w.id);
    };
    assert_eq!(at(&idle_frame("harbor"), 400, 200), at(&all[0].render(), 400, 200));
    is_idle_with_bubble(&r.frame(), &all[0]);
    // Right: the next wallpaper fills the screen, in its palette, at once.
    r.tap(Key::Right);
    is_idle_with_bubble(&r.frame(), &all[1]);
    assert_ne!(Theme::for_wallpaper(&all[1]).sk_bg, skin.sk_bg);
    // Left twice: back, and around the end to the last one.
    r.tap(Key::Left);
    is_idle_with_bubble(&r.frame(), &all[0]);
    r.tap(Key::Left);
    is_idle_with_bubble(&r.frame(), all.last().unwrap());
    r.tap(Key::Right);
    // Up and Down step too.
    r.tap(Key::Down);
    r.tap(Key::Down);
    is_idle_with_bubble(&r.frame(), &all[2]);
    r.tap(Key::Up);
    is_idle_with_bubble(&r.frame(), &all[1]);
    // Nothing is saved by looking; Back leaves the wallpaper as it was.
    assert!(r.commands().is_empty());
    r.tap(Key::Soft(4));
    assert_eq!(r.ui.screen_name(), "display");
    assert_eq!(at(&r.frame(), 470, 5), skin.title_bg);
    assert!(r.commands().is_empty());
    // Select saves the one that is showing.
    r.tap(Key::Digit(4));
    r.tap(Key::Right);
    r.tap(Key::Right);
    r.tap(Key::Soft(1));
    assert_eq!(r.commands(), vec![Command::SetTheme { name: all[2].id.into() }]);
    assert_eq!(r.ui.screen_name(), "display");
    // No family is named anywhere on the screen: nothing but the idle
    // screen and the bubble is drawn (checked above, pixel for pixel).
}

/// A wallpaper the provisioning server fixed can be looked at, not set.
#[test]
fn a_fixed_wallpaper_cannot_be_changed_in_the_menu() {
    let mut r = Rig::new(UiConfig { theme: "slate".into(), theme_locked: true, ..UiConfig::default() });
    r.show("settings");
    r.tap(Key::Digit(3));
    r.tap(Key::Digit(4));
    assert_eq!(r.ui.screen_name(), "theme");
    r.tap(Key::Right);
    r.tap(Key::Soft(1));
    r.tap(Key::Select);
    assert_eq!(r.ui.screen_name(), "theme");
    assert!(r.commands().is_empty());
}

/// A wallpaper picture that is not built in is shown instead of the
/// built-in one: in the skin's own palette when it has none of its own,
/// in its palette when it has.
#[test]
fn a_wallpaper_picture_and_its_palette() {
    // A picture of another size and shape: 96 x 96, the left half red,
    // the right half blue.  It is scaled to cover the screen.
    let pixels: Vec<u8> =
        (0..96 * 96).flat_map(|i| if i % 96 < 48 { [0x40, 0x10, 0x10, 255] } else { [0x10, 0x10, 0x40, 255] }).collect();
    let image = opendeskphone_ui::image::Image { width: 96, height: 96, pixels: pixels.into() };
    let skin = Theme::for_model(Model::D65);
    let plain = std::sync::Arc::new(Picture::new(&image, None));
    assert_eq!(plain.pixels().len(), (WIDTH * HEIGHT) as usize);
    assert_eq!((at(plain.pixels(), 10, 10), at(plain.pixels(), 470, 260)), (0x401010, 0x101040));
    // It wins over a built-in wallpaper's name, palette and all.
    let mut r = Rig::new(UiConfig { theme: "dusk".into(), wallpaper_picture: Some(plain), ..UiConfig::default() });
    let frame = r.frame();
    assert_eq!((at(&frame, 4, 250), at(&frame, 400, 200)), (0x401010, 0x101040));
    assert_eq!(at(&frame, skin.softkey_rect(0).x + 1, skin.soft_y + 3), skin.sk_bg);
    r.show("settings");
    assert_eq!(r.frame(), menu_frame("harbor"), "the skin's own palette");
    // With a palette of its own.
    let mut palette = wallpaper::find("merlot").unwrap().palette();
    palette.title_bg = 0x123456;
    let themed = std::sync::Arc::new(Picture::new(&image, Some(palette)));
    let mut r = Rig::new(UiConfig { wallpaper_picture: Some(themed), ..UiConfig::default() });
    let frame = r.frame();
    assert_eq!(at(&frame, 4, 250), 0x401010);
    assert_eq!(at(&frame, skin.softkey_rect(0).x + 1, skin.soft_y + 3), palette.sk_bg);
    r.show("settings");
    let frame = r.frame();
    assert_eq!((at(&frame, 470, 5), at(&frame, 440, 200)), (0x123456, palette.bg));
}

/// Gray screens have no wallpaper and one palette: every screen is the
/// same whatever is selected, and the Display menu has no Wallpaper item.
#[test]
fn gray_models_are_unaffected() {
    for model in [Model::D40, Model::D45, Model::D50, Model::D70] {
        let image = opendeskphone_ui::image::Image { width: 1, height: 1, pixels: vec![255, 0, 0, 255].into() };
        let picture = std::sync::Arc::new(Picture::new(&image, Some(wallpaper::find("sunrise").unwrap().palette())));
        let mut plain = Rig::with_model(model, UiConfig::default());
        let mut named = Rig::with_model(model, UiConfig { theme: "sunrise".into(), ..UiConfig::default() });
        let mut pictured = Rig::with_model(model, UiConfig { wallpaper_picture: Some(picture), ..UiConfig::default() });
        assert_eq!(named.frame(), plain.frame(), "{model}: idle");
        assert_eq!(pictured.frame(), plain.frame(), "{model}: idle");
        for r in [&mut plain, &mut named, &mut pictured] {
            r.show("settings");
            r.tap(Key::Digit(3));
            assert_eq!(r.ui.screen_name(), "display");
        }
        assert_eq!(named.frame(), plain.frame(), "{model}: display menu");
        assert_eq!(pictured.frame(), plain.frame(), "{model}: display menu");
        // Brightness, Contrast, Dim when idle, Backlight timeout: a fifth
        // item would open on 5.
        named.tap(Key::Digit(5));
        assert_eq!(named.ui.screen_name(), "display", "{model}");
    }
}

// ------------------------------------------------------------------ motion

/// The wallpapers that move.
const MOVING: [&str; 8] = ["regatta", "bokeh", "fizz", "lanterns", "orbit", "tides", "dunes", "drift"];

/// The most a channel of a pixel changes from one frame of a wallpaper's
/// motion to the next, in steps of the frame buffer's 255: motion in
/// steps as small as this is not seen as steps.
const SMOOTH: u32 = 8;

/// One test at a time draws wallpapers in motion: what a frame needs from
/// the frame before is kept for one wallpaper, for one user of it.
static MOTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How many pixels differ, and the largest difference of a channel.
fn difference(a: &[u32], b: &[u32]) -> (usize, u32) {
    let mut most = 0;
    let count = a
        .iter()
        .zip(b)
        .filter(|(p, q)| p != q)
        .inspect(|(p, q)| {
            for shift in [16, 8, 0] {
                most = most.max(((*p >> shift) & 255).abs_diff((*q >> shift) & 255));
            }
        })
        .count();
    (count, most)
}

/// The wallpapers that move do so slowly: a frame is the picture of a
/// time and nothing else, it comes back after a cycle, and drawing only
/// what moved since the frame before gives the picture drawn whole.  The
/// text of the idle screen can be read on every frame.
#[test]
fn the_moving_wallpapers() {
    let _alone = MOTION.lock().unwrap_or_else(|p| p.into_inner());
    for w in wallpaper::all().iter().filter(|w| w.frame_ms().is_none()) {
        // The others are their still pictures at every time.
        if ["harbor", "dusk", "pebbles", "houndstooth"].contains(&w.id) {
            assert_eq!(w.render_at(12_345), w.render(), "{}", w.id);
        }
    }
    let mut failures = Vec::new();
    for id in MOVING {
        let w = wallpaper::find(id).unwrap();
        let every = u64::from(w.frame_ms().unwrap());
        // Ten to twenty frames a second, a whole number of them (and of
        // pairs of them: the rows of a flow take turns) in a cycle.
        assert!((50..=100).contains(&every), "{id}: a frame every {every} ms");
        assert_eq!(wallpaper::CYCLE_MS % (2 * every), 0, "{id}");
        // Nothing goes farther than a pixel between two frames.
        let farthest = w.farthest_step().unwrap();
        assert!((1..=16).contains(&farthest), "{id}: {farthest} sixteenths of a pixel in a frame");
        let th = Theme::for_wallpaper(w);
        // The picture of a time: the same every time, and a cycle later.
        let start = 7_000;
        let first = w.render_at(start);
        assert_eq!(first, w.render_at(start), "{id}");
        assert_eq!(first, w.render_at(start + wallpaper::CYCLE_MS), "{id}: after a cycle");
        assert_eq!(first, w.render_at(start + 5 * wallpaper::CYCLE_MS), "{id}: after five cycles");
        // Frame after frame, each drawn from the one before, is the
        // picture drawn whole; it differs from the one before by little
        // (the motion is slow; where every circle is a large, faint one
        // that moves once in several frames, some frames by nothing), and
        // after a few seconds by more.
        let mut last = first.clone();
        let mut moved = 0;
        assert!(w.with_frame(start, |f| f == first.as_slice()), "{id}");
        for n in 1..=24 {
            let t = start + n * every;
            let whole = w.render_at(t);
            assert!(w.with_frame(t, |f| f == whole.as_slice()), "{id}: frame {n} drawn from the one before");
            let (count, most) = difference(&last, &whole);
            moved += u32::from(count > 0);
            assert!(most <= SMOOTH, "{id}: frame {n} differs from the one before by {most} in a channel");
            last = whole;
        }
        assert!(moved >= 8, "{id}: {moved} of 24 frames differ from the one before");
        // A frame that is skipped to (the UI was busy), and one before.
        for t in [start + 31_000, start + 2_000] {
            let whole = w.render_at(t);
            assert!(w.with_frame(t, |f| f == whole.as_slice()), "{id}: the frame of {t}");
        }
        assert!(difference(&first, &w.render_at(start + 5_000)).0 > 2_000, "{id}: five seconds later");
        // Every frame is the wallpaper still: no pixel far from the
        // still picture's, and the idle screen's text can be read.
        for t in (0..wallpaper::CYCLE_MS).step_by(1_500) {
            let frame = w.render_at(t);
            for (what, area) in text_areas(th) {
                let ratio = least_contrast(&frame, area, th.wallpaper_fg);
                if ratio < TEXT {
                    failures.push(format!("{id} at {t} ms: {what}: {ratio:.2}, under {TEXT}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "frames on which text cannot be read:\n{}", failures.join("\n"));
    // The flow's frame at the start of a cycle is its still picture, to
    // within eight steps of a channel: its rows are of two times, a frame
    // apart, and the frames keep a channel away from its ends, take the
    // waves from tables, work the flow out for one pixel in eight, and
    // place the colors more finely than the still picture does.
    let regatta = wallpaper::find("regatta").unwrap();
    let (_, most) = difference(&regatta.render_at(0), &regatta.render());
    assert!(most <= 8, "regatta at the start of a cycle: {most} from its still picture");
}

/// A frame of the motion is made from the frame before when nothing else
/// on the screen has changed: the pixels the wallpaper's frame changed,
/// with what the screen draws over them, and the screen is not drawn
/// again.  The result is the screen drawn whole at that time: the same
/// pixel for pixel where the wallpaper shows or is hidden, and within one
/// step of a channel where something is blended over it (the edges of the
/// clock's digits, of the softkeys, of the selector's bubble).
#[test]
fn a_frame_made_from_the_one_before_is_the_screen_drawn_whole() {
    let _alone = MOTION.lock().unwrap_or_else(|p| p.into_inner());
    for id in MOVING {
        let w = wallpaper::find(id).unwrap();
        let every = u64::from(w.frame_ms().unwrap());
        for screen in ["idle", "theme", "notice"] {
            let rig = |t: u64| {
                let mut r = Rig::with_model(Model::D60, UiConfig { theme: id.into(), ..UiConfig::default() });
                match screen {
                    "theme" => r.show("theme"),
                    "notice" => {
                        let note = Command::Notify { level: opendeskphone_state::NoticeLevel::Info, title: "A notice".into(), text: "Over the wallpaper.".into(), timeout_ms: 0 };
                        let asked = r.store.send(note);
                        r.ui.pump();
                        assert!(asked.wait().is_ok());
                    }
                    _ => {}
                }
                r.ui.set_motion_time(Some(t));
                r.frame();
                r
            };
            let mut t = 2_000;
            let mut r = rig(t);
            let first = r.frame();
            for n in 1..=24u64 {
                t += every;
                let made = r.ui.motion_frames();
                r.ui.motion_step(t);
                // (The frame after the comparison below is drawn whole:
                // the other screen has drawn the wallpaper since.)
                let whole = n % 8 == 1 && n > 1;
                assert_eq!(r.ui.motion_frames() - made, u64::from(!whole), "{id} on {screen}: frame {n}");
                if n % 8 != 0 {
                    continue;
                }
                let (stepped, whole) = (r.frame(), rig(t).frame());
                let (_, most) = difference(&stepped, &whole);
                assert!(most <= 1, "{id} on {screen}: frame {n} differs by {most} from the screen drawn whole");
                // The wallpaper alone, and the fill of a softkey.
                let th = Theme::for_wallpaper(w);
                let key = th.softkey_rect(0);
                for (x, y) in [(3, 60), (300, 215), (40, 230), (key.x + 6, key.y + 4)] {
                    assert_eq!(at(&stepped, x, y), at(&whole, x, y), "{id} on {screen}: frame {n} at ({x}, {y})");
                }
                assert_ne!(stepped, first, "{id} on {screen}: frame {n} is the first");
            }
        }
    }
}

/// The wallpaper moves on the idle screen and in the theme selector, and
/// nowhere else: not under a menu, not behind a call, not on a gray
/// model, not while the backlight is off.  A new frame is due a frame's
/// time after the last.
#[test]
fn the_wallpaper_moves_on_the_idle_screen_only() {
    let _alone = MOTION.lock().unwrap_or_else(|p| p.into_inner());
    let tides = wallpaper::find("tides").unwrap();
    let mut r = Rig::with_model(Model::D60, UiConfig { theme: "tides".into(), ..UiConfig::default() });
    // Where no text or tab is: the wallpaper itself.
    let spot = |frame: &[u32]| -> Vec<u32> { (100..244).flat_map(|y| (0..30).map(move |x| (x, y))).map(|(x, y)| at(frame, x, y)).collect() };
    // The time is fixed (as in every other test here): the still picture.
    let still = r.frame();
    assert_eq!(spot(&still), spot(&tides.render()));
    assert_eq!(r.ui.motion_deadline(), None);
    // With a time for the motion: the frame of that time, and the next
    // is due.
    r.ui.set_motion_time(Some(3_000));
    let a = r.frame();
    assert_eq!(spot(&a), spot(&tides.render_at(3_000)));
    assert_ne!(spot(&a), spot(&still));
    let due = r.ui.motion_deadline().expect("the next frame is due");
    let now = clock::mono_ms();
    assert!(due <= now + 150 && due + 150 >= now, "due {due} at {now}");
    r.ui.set_motion_time(Some(9_000));
    let b = r.frame();
    assert_eq!(spot(&b), spot(&tides.render_at(9_000)));
    assert_ne!(spot(&a), spot(&b), "frames differ over time");
    // The clock, the tabs, and the softkeys are where they were.
    assert_eq!(at(&a, 400, 20), at(&b, 400, 20));
    let key = Theme::for_wallpaper(tides).softkey_rect(0);
    assert_eq!(at(&b, key.x + 1, key.y + 3), Theme::for_wallpaper(tides).sk_bg);

    // Under a menu nothing moves, and no frame is due.
    r.show("settings");
    r.ui.set_motion_time(Some(12_000));
    let menu = r.frame();
    assert_eq!(r.ui.motion_deadline(), None);
    r.ui.set_motion_time(Some(15_000));
    assert_eq!(r.frame(), menu);
    assert_eq!(r.ui.motion_deadline(), None);
    // The theme selector shows the wallpaper as the idle screen does.
    r.show("theme");
    assert_eq!(r.ui.screen_name(), "theme");
    assert_eq!(spot(&r.frame()), spot(&tides.render_at(15_000)));
    assert!(r.ui.motion_deadline().is_some());
    // The selection on a theme that does not move (Glacier, before
    // Tides): nothing is due.  Back on Tides, a frame is.
    r.tap(Key::Left);
    assert_eq!(r.ui.motion_deadline(), None);
    r.tap(Key::Right);
    assert!(r.ui.motion_deadline().is_some());
    r.show("idle");
    assert!(r.ui.motion_deadline().is_some());

    // Behind a call the wallpaper is its still picture.
    let id = r.store.next_call_id();
    r.store.update::<Calls, _>(|c| {
        let mut call = Call::new(id, AccountId(0), Direction::Incoming, Party { number: "1002".into(), ..Default::default() });
        call.state = CallState::Ringing;
        c.calls.push(call);
        c.focused = Some(id);
    });
    let call = r.frame();
    assert_ne!(r.ui.screen_name(), "idle");
    assert_eq!(r.ui.motion_deadline(), None);
    assert_eq!(at(&call, 2, 242), at(&tides.render(), 2, 242));
    r.store.update::<Calls, _>(|c| {
        c.calls.clear();
        c.focused = None;
    });
    r.show("idle");
    assert_eq!(spot(&r.frame()), spot(&tides.render_at(15_000)));

    // A theme that does not move; a provisioned picture; a gray model.
    let mut dusk = Rig::named("dusk");
    dusk.ui.set_motion_time(Some(3_000));
    assert_eq!(dusk.frame(), idle_frame("dusk"));
    assert_eq!(dusk.ui.motion_deadline(), None);
    let image = opendeskphone_ui::image::Image { width: 2, height: 2, pixels: vec![0x40; 16].into() };
    let picture = std::sync::Arc::new(Picture::new(&image, None));
    let mut pictured = Rig::new(UiConfig { theme: "tides".into(), wallpaper_picture: Some(picture), ..UiConfig::default() });
    pictured.ui.set_motion_time(Some(3_000));
    pictured.frame();
    assert_eq!(pictured.ui.motion_deadline(), None);
    let mut gray = Rig::with_model(Model::D50, UiConfig { theme: "tides".into(), ..UiConfig::default() });
    let before = gray.frame();
    gray.ui.set_motion_time(Some(3_000));
    assert_eq!(gray.frame(), before);
    assert_eq!(gray.ui.motion_deadline(), None);
}

/// By the clock: frames follow one another on the idle screen, and stop
/// when the backlight goes off (dimmed to nothing), until a key lights it
/// again.  A backlight that is dimmed and still gives light leaves the
/// wallpaper moving.
#[test]
fn the_motion_stops_while_the_backlight_is_off() {
    let _alone = MOTION.lock().unwrap_or_else(|p| p.into_inner());
    let tides = wallpaper::find("tides").unwrap();
    let spot = |frame: &[u32]| -> Vec<u32> { (100..244).flat_map(|y| (0..30).map(move |x| (x, y))).map(|(x, y)| at(frame, x, y)).collect() };
    let wait = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
    for (dim_level, moves_when_dim) in [(0, false), (2, true)] {
        let cfg = UiConfig { theme: "tides".into(), dim_backlight: true, backlight_dim_level: dim_level, ..UiConfig::default() };
        let mut r = Rig::with_model(Model::D60, cfg.clone());
        // The clock runs; the backlight dims after a second without a key.
        r.ui.set_fixed_time(None);
        r.ui.handle().set_config(UiConfig { utc_offset_secs: -7 * 3600, backlight_timeout_secs: 1, ..cfg });
        r.tap(Key::Cancel);
        let first = r.frame();
        assert!(r.ui.motion_deadline().is_some());
        assert!(r.store.read::<opendeskphone_state::Ui, _>(|u| u.backlight_on));
        // Half a second on, the frame is another: the UI drew it because
        // it was due, with nothing else to do.
        wait(500);
        let second = r.frame();
        assert_ne!(spot(&first), spot(&second), "dim level {dim_level}: frames differ over time");
        // Past the timeout the backlight is dimmed.
        wait(1_000);
        let dimmed = r.frame();
        assert_eq!(r.store.read::<opendeskphone_state::Ui, _>(|u| u.backlight_on), moves_when_dim);
        assert_eq!(r.ui.motion_deadline().is_some(), moves_when_dim, "dim level {dim_level}");
        wait(400);
        let later = r.frame();
        if moves_when_dim {
            assert_ne!(spot(&dimmed), spot(&later), "dimmed, with light: the wallpaper moves");
        } else {
            // Dark: nothing is drawn, and what is there is the still
            // picture.
            assert_eq!(later, dimmed);
            assert_eq!(spot(&later), spot(&tides.render()));
            // A key lights the screen, and the motion goes on.
            r.tap(Key::Cancel);
            assert!(r.store.read::<opendeskphone_state::Ui, _>(|u| u.backlight_on));
            assert!(r.ui.motion_deadline().is_some());
            assert_ne!(spot(&r.frame()), spot(&tides.render()));
        }
    }
}

/// The theme selector is on a softkey of the color models, after
/// Forward; a gray model, which has no themes, leaves the softkey out.
#[test]
fn the_theme_softkey() {
    use opendeskphone_ui::IdleSoftkey::{Apps, CallLog, Contacts, Dnd, Forward, Menu, Redial, Status};
    const THEME: opendeskphone_ui::IdleSoftkey = opendeskphone_ui::IdleSoftkey::Theme;
    // The default softkeys of a color model (`opendeskphone-config` has them
    // for each model): three to a page behind "More".
    let keys = vec![CallLog, Contacts, Status, Menu, Apps, Dnd, Forward, THEME];
    let mut r = Rig::new(UiConfig { theme: "dusk".into(), idle_softkeys: keys, ..UiConfig::default() });
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(1));
    assert_eq!(r.ui.screen_name(), "forward");
    r.tap(Key::Cancel);
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(4));
    r.tap(Key::Soft(2));
    assert_eq!(r.ui.screen_name(), "theme");
    // The selector, on the theme in use: Right and Select choose the
    // next one, and Back and Select both lead back to the idle screen.
    let all = wallpaper::all();
    let dusk = all.iter().position(|w| w.id == "dusk").unwrap();
    r.tap(Key::Right);
    r.tap(Key::Select);
    assert_eq!(r.commands(), vec![Command::SetTheme { name: all[dusk + 1].id.into() }]);
    assert_eq!(r.ui.screen_name(), "idle");
    // The softkey says "Theme": the page with it differs from the same
    // page without it only in the second softkey.
    let labels = |cfg_keys: Vec<opendeskphone_ui::IdleSoftkey>| {
        let mut r = Rig::new(UiConfig { theme: "dusk".into(), idle_softkeys: cfg_keys, ..UiConfig::default() });
        r.tap(Key::Soft(4));
        r.tap(Key::Soft(4));
        r.frame()
    };
    let th = Theme::for_wallpaper(wallpaper::find("dusk").unwrap());
    let (with, without) = (labels(vec![CallLog, Contacts, Status, Menu, Apps, Dnd, Forward, THEME]), labels(vec![CallLog, Contacts, Status, Menu, Apps, Dnd, Forward, Redial]));
    let key = th.softkey_rect(1);
    let label = |f: &[u32]| (key.y..key.bottom()).flat_map(|y| (key.x..key.right()).map(move |x| (x, y))).filter(|(x, y)| at(f, *x, *y) == th.sk_fg).count();
    assert!(label(&with) > 30 && label(&with) != label(&without));

    // A gray model: the softkey is not there, so the second softkey is
    // the one after it, and there is no third.
    let mut gray = Rig::with_model(Model::D50, UiConfig { idle_softkeys: vec![Menu, THEME, Forward], ..UiConfig::default() });
    gray.tap(Key::Soft(2));
    assert_eq!(gray.ui.screen_name(), "forward");
    gray.tap(Key::Cancel);
    gray.tap(Key::Soft(3));
    assert_eq!(gray.ui.screen_name(), "idle");
    // Nor can the selector be asked for, and the Display menu has no
    // such item (its last is the backlight timeout).
    let asked = gray.store.send(Command::ShowScreen { name: "theme".into() });
    gray.ui.pump();
    assert!(asked.wait().is_err());
    assert_ne!(gray.ui.screen_name(), "theme");
    gray.show("settings");
    gray.tap(Key::Digit(3));
    assert_eq!(gray.ui.screen_name(), "display");
    gray.tap(Key::Digit(5));
    assert_ne!(gray.ui.screen_name(), "theme");
}

// ------------------------------------------------------------- the D80

/// The touch skin's screen (D80): upright.
const D80_W: i32 = 800;
const D80_H: i32 = 1280;

fn at_d80(pixels: &[u32], x: i32, y: i32) -> Rgb {
    pixels[(y * D80_W + x) as usize]
}

/// Whether two colors are within `by` of each other in every channel
/// (the dither moves a channel by up to two steps).
fn near(a: Rgb, b: Rgb, by: u32) -> bool {
    [16, 8, 0].iter().all(|s| ((a >> s) & 255).abs_diff((b >> s) & 255) <= by)
}

/// Every wallpaper drawn for the D80, in the order of `wallpaper::all`
/// (drawn once for the tests that look at them).
fn d80_pictures() -> &'static [Vec<u32>] {
    static PICTURES: std::sync::OnceLock<Vec<Vec<u32>>> = std::sync::OnceLock::new();
    PICTURES.get_or_init(|| wallpaper::all().iter().map(|w| w.render_sized(D80_W, D80_H)).collect())
}

/// On the D80 every built-in wallpaper is drawn from its description for
/// its upright screen, in the same palette: the same picture every time,
/// composed for the screen's shape and not the color skin's picture
/// turned.  A gradient from top to bottom runs from top to bottom, a
/// pattern keeps its size to the eye (a pixel of it is two of the
/// screen's), and a circle stays round.  At the color skin's size the
/// picture is the color skin's.
#[test]
fn the_d80_wallpapers_are_drawn_for_its_screen() {
    let d80 = Theme::for_model(Model::D80);
    let picture = |id: &str| &d80_pictures()[wallpaper::all().iter().position(|w| w.id == id).unwrap()];
    for (w, p) in wallpaper::all().iter().zip(d80_pictures()) {
        assert_eq!(p.len(), (D80_W * D80_H) as usize, "{}", w.id);
        assert!(p.iter().all(|px| *px <= 0xFF_FFFF), "{}", w.id);
        assert_eq!(w.render_sized(WIDTH, HEIGHT), w.render(), "{}", w.id);
        assert_eq!(d80.in_wallpaper(w).palette(), w.palette(), "{}", w.id);
    }
    // The same picture every time (those drawn from random numbers too).
    for id in ["prism", "bokeh", "summit", "harbor"] {
        assert_eq!(picture(id), &wallpaper::find(id).unwrap().render_sized(D80_W, D80_H), "{id} differs between two renders");
    }
    // Dusk: from its first stop at the top to its last at the bottom, and
    // each row one color across.
    let dusk = picture("dusk");
    assert!(near(at_d80(dusk, 400, 0), 0x141A3A, 2) && near(at_d80(dusk, 400, D80_H - 1), 0x9A5540, 2));
    for y in [100, 640, 1200] {
        assert!(near(at_d80(dusk, 0, y), at_d80(dusk, D80_W - 1, y), 4), "dusk, row {y}");
    }
    // The stand's awning: upright stripes of 160 pixels, red then yellow,
    // from top to bottom.
    let stand = picture("hot-dog-stand");
    for y in [0, 640, D80_H - 1] {
        let colors: Vec<Rgb> = [40, 120, 200, 280, 760].iter().map(|x| at_d80(stand, *x, y)).collect();
        assert_eq!(colors, [0xFF0000, 0xFFFF00, 0xFF0000, 0xFFFF00, 0xFFFF00], "row {y}");
    }
    // The pinstripes: one every 24 pixels, two pixels wide.
    let pinstripe = picture("pinstripe");
    let base = at_d80(pinstripe, 0, 300);
    let striped: Vec<i32> = (0..96).filter(|x| at_d80(pinstripe, *x, 300) != base).collect();
    assert_eq!(striped, [10, 11, 34, 35, 58, 59, 82, 83]);
    // Eclipse: its rings are round.  Its middle ring, at the same distance
    // from their center (below the screen) up and up to the left.
    let eclipse = picture("eclipse");
    let (cx, cy) = (390 * D80_W / WIDTH, 290 * D80_H / HEIGHT);
    let up = at_d80(eclipse, cx, cy - 330);
    let left = at_d80(eclipse, cx - 233, cy - 233);
    assert!(near(up, 0x5A3440, 2) && near(left, 0x5A3440, 2), "{up:06X} {left:06X}");
}

/// The idle screen's text can be read on every wallpaper of the D80 where
/// it can be: the status icons (the middle of the header band, left of the
/// lines), and the places of the color skin's clock and date, in
/// proportion.
#[test]
fn the_d80_wallpapers_can_be_read() {
    let d80 = Theme::for_model(Model::D80);
    let (sx, sy) = (|v: i32| v * D80_W / WIDTH, |v: i32| v * D80_H / HEIGHT);
    let areas = [
        ("status icons", IRect::new(0, (d80.header_h - d80.icon) / 2, d80.idle_tab_x, d80.icon)),
        ("clock", IRect::new(sx(30), sy(84), sx(266), sy(72))),
        ("date", IRect::new(sx(30), sy(176), sx(236), sy(28))),
    ];
    let mut failures = Vec::new();
    for (w, p) in wallpaper::all().iter().zip(d80_pictures()) {
        let th = d80.in_wallpaper(w);
        for (what, r) in areas {
            let mut least = f64::MAX;
            for y in r.y..r.bottom() {
                for x in r.x..r.right() {
                    least = least.min(contrast(th.wallpaper_fg, at_d80(p, x, y)));
                }
            }
            if least < TEXT {
                failures.push(format!("{}: {what}: {least:.2}, under {TEXT}", w.id));
            }
        }
    }
    assert!(failures.is_empty(), "D80 wallpapers on which text cannot be read:\n{}", failures.join("\n"));
}

/// A provisioned picture on the D80 is scaled from the picture as it came
/// to cover the upright screen: not turned, and not cut from the color
/// skin's 480 x 272 (an upright picture shows whole, and a small one is
/// not in blocks).  On the color skin it is what it was.
#[test]
fn a_wallpaper_picture_on_the_d80() {
    // Upright, 80 x 128: a green band at the top, red above the middle,
    // blue below.
    let pixels: Vec<u8> = (0..80 * 128)
        .flat_map(|i| match i / 80 {
            0..8 => [0x10, 0x60, 0x10, 255],
            8..64 => [0x60, 0x10, 0x10, 255],
            _ => [0x10, 0x10, 0x60, 255],
        })
        .collect();
    let image = opendeskphone_ui::image::Image { width: 80, height: 128, pixels: pixels.into() };
    let picture = std::sync::Arc::new(Picture::new(&image, None));
    let fitted = picture.pixels_sized(D80_W, D80_H);
    assert_eq!(fitted.len(), (D80_W * D80_H) as usize);
    assert_eq!((at_d80(&fitted, 400, 30), at_d80(&fitted, 400, 300), at_d80(&fitted, 400, 1000)), (0x106010, 0x601010, 0x101060));
    // Between two of the picture's pixels, a color between theirs.
    let edge = at_d80(&fitted, 400, 640);
    assert!(edge != 0x601010 && edge != 0x101060 && (edge >> 16) > 0x10 && (edge & 255) > 0x10, "{edge:06X}");
    assert_eq!(picture.pixels_sized(WIDTH, HEIGHT), picture.pixels());
    // On the idle screen, where nothing is drawn over it.
    let mut r = Rig::with_model(Model::D80, UiConfig { theme: "dusk".into(), wallpaper_picture: Some(picture), ..UiConfig::default() });
    let frame = r.frame();
    assert_eq!((at_d80(&frame, 790, 400), at_d80(&frame, 790, 900)), (0x601010, 0x101060));
    // Wide, 96 x 96, the left half red and the right half blue: its middle
    // covers the screen, red on the left and blue on the right.
    let pixels: Vec<u8> = (0..96 * 96).flat_map(|i| if i % 96 < 48 { [0x40, 0x10, 0x10, 255] } else { [0x10, 0x10, 0x40, 255] }).collect();
    let image = opendeskphone_ui::image::Image { width: 96, height: 96, pixels: pixels.into() };
    let fitted = Picture::new(&image, None).pixels_sized(D80_W, D80_H);
    assert_eq!((at_d80(&fitted, 100, 0), at_d80(&fitted, 700, D80_H - 1)), (0x401010, 0x101040));
}

/// On the D80 the waves move on the idle screen, as they do on the color
/// skin, and a frame made from the one before is the screen drawn whole.
/// The other motions change what grows with the screen's area, and cost
/// too much there (`Wallpaper::frame_ms_sized`): those wallpapers are
/// still on the D80.
#[test]
fn on_the_d80_the_waves_move() {
    let _alone = MOTION.lock().unwrap_or_else(|p| p.into_inner());
    for id in MOVING {
        let w = wallpaper::find(id).unwrap();
        let waves = ["tides", "dunes"].contains(&id);
        assert_eq!(w.frame_ms_sized(D80_W, D80_H).is_some(), waves, "{id}");
        assert_eq!(w.frame_ms_sized(WIDTH, HEIGHT), w.frame_ms(), "{id}");
        let mut r = Rig::with_model(Model::D80, UiConfig { theme: id.into(), ..UiConfig::default() });
        r.ui.set_motion_time(Some(3_000));
        r.frame();
        assert_eq!(r.ui.motion_deadline().is_some(), waves, "{id}");
    }
    let tides = wallpaper::find("tides").unwrap();
    let every = u64::from(tides.frame_ms().unwrap());
    // Nothing goes farther than a pixel between two frames.
    let farthest = tides.farthest_step_sized(D80_W, D80_H).unwrap();
    assert!((1..=16).contains(&farthest), "{farthest} sixteenths of a pixel in a frame");
    // Frame after frame, each drawn from the one before, is the picture
    // drawn whole; and it comes back after a cycle.
    let start = 7_000;
    let first = tides.render_at_sized(D80_W, D80_H, start);
    assert_eq!(first, tides.render_at_sized(D80_W, D80_H, start + wallpaper::CYCLE_MS));
    assert!(tides.with_frame_and_changes_sized(D80_W, D80_H, start, |f, _| f == first.as_slice()));
    for n in 1..=6 {
        let t = start + n * every;
        let whole = tides.render_at_sized(D80_W, D80_H, t);
        assert!(tides.with_frame_and_changes_sized(D80_W, D80_H, t, |f, _| f == whole.as_slice()), "frame {n}");
    }
    // On the idle screen.
    let rig = |t: u64| {
        let mut r = Rig::with_model(Model::D80, UiConfig { theme: "tides".into(), ..UiConfig::default() });
        r.ui.set_motion_time(Some(t));
        r.frame();
        r
    };
    let mut t = 2_000;
    let mut r = rig(t);
    let first = r.frame();
    for n in 1..=8 {
        t += every;
        let made = r.ui.motion_frames();
        r.ui.motion_step(t);
        assert_eq!(r.ui.motion_frames() - made, 1, "frame {n}");
    }
    let (stepped, whole) = (r.frame(), rig(t).frame());
    let (_, most) = difference(&stepped, &whole);
    assert!(most <= 1, "differs by {most} from the screen drawn whole");
    assert_ne!(stepped, first);
}
