//! The four skins: metrics, colors, fonts, and icons per screen class.
//!
//! The geometry here is measured from stock firmware screenshots, so that
//! line-key labels sit beside their physical keys and softkey labels above
//! theirs:
//!
//! | | D40/D45/D50 | D70 | D60/D62/D65 |
//! | --- | --- | --- | --- |
//! | Line-key tab column | x 155..239 | x 206..319 | x 297..479 |
//! | The same, idle screen | x 114..239 | x 160..319 | x 297..479 |
//! | Tab pitch from y = 0 | 23 | 23 | 40 |
//! | Header band (left of the tabs) | y 0..14 | y 0..16 | y 0..39 |
//! | Softkey shapes | y 103..119 | y 141..159 | y 244..268 |
//! | Softkey x: first, pitch, width | 0, 60, 59 | 0, 80, 79 | 14, 116, 104 |
//!
//! Two things on the gray skins are wider than stock's, for the room they
//! give a label: the idle screen's tab column starts as far left as the
//! clock allows, and a softkey shape fills its cell but for one column
//! (stock: two).
//!
//! Colors of the color skin are the stock values by measurement; the
//! wallpaper and every bitmap are our own.
//!
//! The touch skin (D80: 800x1280, upright, about 216 pixels to the inch if
//! its panel is 7 inches across, as is taken but not yet measured) is
//! measured from nothing: the D80 has no keys for a label to sit
//! beside.  It has the color skin's colors, fonts and icons of about twice
//! the color skin's pixels (the same size to the eye), and everything
//! that is touched is at least [`Theme::touch_min`] pixels each way.  Its
//! softkeys are buttons ([`Theme::soft_button`]), and its screens say
//! where they can be touched as they draw ([`crate::touch::Hit`]).

use crate::appview::{Color, Font, ThemeColor};
use crate::atlas;
use crate::font::FontData;
use crate::gfx::{rgb, IRect, Rgb, GRAY};
use crate::icons::{Icon, IconRec};
use opendeskphone_state::{Model, Skin};

/// Everything skin-dependent.
pub struct Theme {
    pub skin: Skin,
    pub gray: bool,
    /// The touch skin ([`Theme::touch`]).
    touch: bool,
    /// How many of this skin's pixels a pixel of the color skin is, for a
    /// length that is the same to the eye ([`Theme::px`]): 2 on the touch
    /// skin, 1 on the others.
    pub scale: i32,
    /// The least width and height of what is touched (a button, a row):
    /// 88 pixels on the touch skin, about 10 mm; 0 on the others.
    pub touch_min: i32,
    /// The width of the back button at the left of a title bar
    /// ([`crate::widgets::title_bar`]); 0 where there is none.
    pub back_w: i32,
    fonts: &'static [&'static FontData; 9],
    icons: &'static [IconRec],
    icon_data: &'static [u8],
    /// Main screen size.
    pub width: i32,
    pub height: i32,
    /// Height reserved for softkeys at the bottom (18, 20, 29).
    pub softkey_h: i32,
    /// Left edge of the first softkey shape, distance between shapes, and
    /// the width, top, and height of each shape.
    pub soft_x0: i32,
    pub soft_pitch: i32,
    pub soft_w: i32,
    pub soft_y: i32,
    pub soft_h: i32,
    /// Line-key tabs: left edge (including the border), width, and pitch.
    /// Tab `i` occupies rows `i * tab_h .. (i + 1) * tab_h` from the top of
    /// the screen, beside physical line key `i + 1`.
    pub tab_x: i32,
    pub tab_w: i32,
    pub tab_h: i32,
    /// Left edge of the tabs on the idle screen, where only the clock is
    /// beside them.
    pub idle_tab_x: i32,
    /// Height of the header band at the top left of the idle and call
    /// screens (15, 17, 40).
    pub header_h: i32,
    /// `Text.LINE_HEIGHT`.
    pub line_h: i32,
    /// `Text.ROW_HEIGHT`.
    pub row_h: i32,
    /// `Text.TITLE_HEIGHT`.
    pub title_h: i32,
    pub scroll_w: i32,
    /// General padding.
    pub pad: i32,
    /// Left inset of titles and header text.
    pub title_pad: i32,
    /// Size of status icons.
    pub icon: i32,
    pub bg: Rgb,
    pub fg: Rgb,
    pub title_bg: Rgb,
    pub title_fg: Rgb,
    pub sel_bg: Rgb,
    pub sel_fg: Rgb,
    pub secondary: Rgb,
    pub border: Rgb,
    pub modal_bg: Rgb,
    /// Softkey fill, outline, and label.
    pub sk_bg: Rgb,
    pub sk_edge: Rgb,
    pub sk_fg: Rgb,
    /// Softkey fills for "go" (answer) and "stop" (end, ignore) actions.
    pub sk_go: Rgb,
    pub sk_stop: Rgb,
    pub good: Rgb,
    pub warn: Rgb,
    pub error: Rgb,
    pub accent: Rgb,
    /// Call card: fill for an active call, a ringing call, and a held one;
    /// and the dark part at its right.
    pub card: Rgb,
    pub card_ringing: Rgb,
    pub card_held: Rgb,
    pub card_dark: Rgb,
    /// Line-key tab fill.
    pub tab_bg: Rgb,
    pub tab_fg: Rgb,
    /// The boot screen's loading bar: the empty track and the filled part.
    pub load_track: Rgb,
    pub load_fill: Rgb,
    /// Text and icons drawn on the wallpaper itself: the idle screen's
    /// clock, date, and status icons, and an app's idle view.  The same
    /// as `fg` unless the wallpaper and the menus' background are unlike
    /// (a bright wallpaper over dark menus, or the reverse).
    pub wallpaper_fg: Rgb,
}

static SMALL_GRAY: Theme = gray_theme(Skin::SmallGray);
static LARGE_GRAY: Theme = gray_theme(Skin::LargeGray);
static COLOR: Theme = COLOR_INIT;
/// The color skin, as a value the touch skin is made from.
const COLOR_INIT: Theme = Theme {
    skin: Skin::Color,
    gray: false,
    touch: false,
    scale: 1,
    touch_min: 0,
    back_w: 0,
    fonts: &atlas::FONTS_COLOR,
    icons: atlas::ICONS_COLOR,
    icon_data: atlas::ICON_DATA_COLOR,
    width: 480,
    height: 272,
    softkey_h: 29,
    soft_x0: 14,
    soft_pitch: 116,
    soft_w: 104,
    soft_y: 244,
    soft_h: 25,
    tab_x: 296,
    tab_w: 184,
    tab_h: 40,
    idle_tab_x: 296,
    header_h: 40,
    line_h: 25,
    row_h: 32,
    title_h: 40,
    scroll_w: 8,
    pad: 8,
    title_pad: 15,
    icon: 22,
    bg: rgb(0x21, 0x21, 0x21),
    fg: rgb(0xEE, 0xEE, 0xEE),
    title_bg: rgb(0x01, 0x41, 0x74),
    title_fg: rgb(0xFF, 0xFF, 0xFF),
    sel_bg: rgb(0x01, 0x57, 0x9B),
    sel_fg: rgb(0xFF, 0xFF, 0xFF),
    secondary: rgb(0xB4, 0xB8, 0xBE),
    border: rgb(0x4D, 0x4D, 0x4D),
    modal_bg: rgb(0x2E, 0x2E, 0x2E),
    sk_bg: rgb(0x2E, 0x2E, 0x2E),
    sk_edge: rgb(0x75, 0x75, 0x75),
    sk_fg: rgb(0xEE, 0xEE, 0xEE),
    sk_go: rgb(0x38, 0x8E, 0x3C),
    sk_stop: rgb(0xB7, 0x1C, 0x1C),
    good: rgb(0x4C, 0xAF, 0x50),
    warn: rgb(0xFF, 0xB3, 0x00),
    error: rgb(0xEF, 0x53, 0x50),
    accent: rgb(0x67, 0xB4, 0xF1),
    card: rgb(0x01, 0x57, 0x9B),
    card_ringing: rgb(0x2A, 0x6B, 0x2D),
    card_held: rgb(0x45, 0x5A, 0x64),
    card_dark: rgb(0x21, 0x21, 0x21),
    tab_bg: rgb(0x42, 0x42, 0x42),
    tab_fg: rgb(0xEE, 0xEE, 0xEE),
    load_track: rgb(0x1F, 0x35, 0x5E),
    load_fill: rgb(0x2F, 0x80, 0xED),
    wallpaper_fg: rgb(0xEE, 0xEE, 0xEE),
};

/// The touch skin: the color skin's colors, at the D80's size.
///
/// From the bottom: softkey buttons in rows of four, each row
/// `softkey_h` (120) pixels with a button of 96 in it.  Lists have rows of
/// 96, and a title bar is 96 tall with the back button at its left.  The
/// lines (the D80's stand-in for line keys) are a column of tabs of 96 at
/// the right half of the screen, from the top, as on the other skins.
///
/// Only in a build with the feature `touch`: its fonts and icons are the
/// largest there are, and a phone with keys has no room for them.
#[cfg(feature = "touch")]
static TOUCH: Theme = Theme {
    skin: Skin::Touch,
    touch: true,
    scale: 2,
    touch_min: 88,
    back_w: 96,
    fonts: &atlas::FONTS_TOUCH,
    icons: atlas::ICONS_TOUCH,
    icon_data: atlas::ICON_DATA_TOUCH,
    width: 800,
    height: 1280,
    softkey_h: 120,
    soft_x0: 12,
    soft_pitch: 197,
    soft_w: 185,
    soft_y: 1172,
    soft_h: 96,
    tab_x: 400,
    tab_w: 400,
    tab_h: 96,
    idle_tab_x: 400,
    header_h: 96,
    line_h: 44,
    row_h: 96,
    title_h: 96,
    scroll_w: 12,
    pad: 16,
    title_pad: 24,
    icon: 44,
    ..COLOR_INIT
};

const fn gray_theme(skin: Skin) -> Theme {
    let large = matches!(skin, Skin::LargeGray);
    Theme {
        skin,
        gray: true,
        touch: false,
        scale: 1,
        touch_min: 0,
        back_w: 0,
        fonts: if large { &atlas::FONTS_LARGE_GRAY } else { &atlas::FONTS_SMALL_GRAY },
        icons: if large { atlas::ICONS_LARGE_GRAY } else { atlas::ICONS_SMALL_GRAY },
        icon_data: if large { atlas::ICON_DATA_LARGE_GRAY } else { atlas::ICON_DATA_SMALL_GRAY },
        width: if large { 320 } else { 240 },
        height: if large { 160 } else { 120 },
        softkey_h: if large { 20 } else { 18 },
        soft_x0: 0,
        soft_pitch: if large { 80 } else { 60 },
        soft_w: if large { 79 } else { 59 },
        soft_y: if large { 141 } else { 103 },
        soft_h: if large { 19 } else { 17 },
        tab_x: if large { 206 } else { 155 },
        tab_w: if large { 114 } else { 85 },
        tab_h: 23,
        // The widest clock ("12:58 PM") is 102 pixels on the small panel
        // and 105 on the large one.
        idle_tab_x: if large { 160 } else { 114 },
        header_h: if large { 17 } else { 15 },
        line_h: if large { 17 } else { 15 },
        row_h: if large { 17 } else { 15 },
        title_h: if large { 17 } else { 15 },
        scroll_w: 4,
        pad: if large { 4 } else { 3 },
        title_pad: 3,
        icon: if large { 15 } else { 13 },
        bg: GRAY[3],
        fg: GRAY[0],
        title_bg: GRAY[0],
        title_fg: GRAY[3],
        sel_bg: GRAY[0],
        sel_fg: GRAY[3],
        secondary: GRAY[1],
        border: GRAY[1],
        modal_bg: GRAY[3],
        sk_bg: GRAY[0],
        sk_edge: GRAY[1],
        sk_fg: GRAY[3],
        sk_go: GRAY[0],
        sk_stop: GRAY[0],
        good: GRAY[0],
        warn: GRAY[0],
        error: GRAY[0],
        accent: GRAY[0],
        card: GRAY[3],
        card_ringing: GRAY[3],
        card_held: GRAY[3],
        card_dark: GRAY[3],
        tab_bg: GRAY[3],
        tab_fg: GRAY[0],
        load_track: GRAY[3],
        load_fill: GRAY[0],
        wallpaper_fg: GRAY[0],
    }
}

/// True for the model with the touch skin (the D80) in a build that has
/// the skin (the feature `touch`); always false, and known to the compiler
/// to be, in one that has not.  As [`Theme::touch`], for code that has the
/// model and no theme.
#[inline(always)]
pub(crate) const fn is_touch(model: Model) -> bool {
    cfg!(feature = "touch") && model.is_touch()
}

/// The colors of a theme: every color role a skin has, and nothing else.
/// Each built-in wallpaper has one ([`crate::wallpaper::Wallpaper::palette`]),
/// and a wallpaper picture from the provisioning server can come with one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    pub title_bg: Rgb,
    pub title_fg: Rgb,
    pub sel_bg: Rgb,
    pub sel_fg: Rgb,
    pub secondary: Rgb,
    pub border: Rgb,
    pub modal_bg: Rgb,
    pub sk_bg: Rgb,
    pub sk_edge: Rgb,
    pub sk_fg: Rgb,
    pub sk_go: Rgb,
    pub sk_stop: Rgb,
    pub good: Rgb,
    pub warn: Rgb,
    pub error: Rgb,
    pub accent: Rgb,
    pub card: Rgb,
    pub card_ringing: Rgb,
    pub card_held: Rgb,
    pub card_dark: Rgb,
    pub tab_bg: Rgb,
    pub tab_fg: Rgb,
    pub load_track: Rgb,
    pub load_fill: Rgb,
    pub wallpaper_fg: Rgb,
}

impl Palette {
    /// The names of the roles (as a palette file has them) with their
    /// colors, in the order of the fields.
    pub fn roles(&self) -> [(&'static str, Rgb); 27] {
        [
            ("bg", self.bg),
            ("fg", self.fg),
            ("title_bg", self.title_bg),
            ("title_fg", self.title_fg),
            ("sel_bg", self.sel_bg),
            ("sel_fg", self.sel_fg),
            ("secondary", self.secondary),
            ("border", self.border),
            ("modal_bg", self.modal_bg),
            ("sk_bg", self.sk_bg),
            ("sk_edge", self.sk_edge),
            ("sk_fg", self.sk_fg),
            ("sk_go", self.sk_go),
            ("sk_stop", self.sk_stop),
            ("good", self.good),
            ("warn", self.warn),
            ("error", self.error),
            ("accent", self.accent),
            ("card", self.card),
            ("card_ringing", self.card_ringing),
            ("card_held", self.card_held),
            ("card_dark", self.card_dark),
            ("tab_bg", self.tab_bg),
            ("tab_fg", self.tab_fg),
            ("load_track", self.load_track),
            ("load_fill", self.load_fill),
            ("wallpaper_fg", self.wallpaper_fg),
        ]
    }

    /// Sets the role called `name`; false when there is no such role.
    pub fn set(&mut self, name: &str, color: Rgb) -> bool {
        match name {
            "bg" => self.bg = color,
            "fg" => self.fg = color,
            "title_bg" => self.title_bg = color,
            "title_fg" => self.title_fg = color,
            "sel_bg" => self.sel_bg = color,
            "sel_fg" => self.sel_fg = color,
            "secondary" => self.secondary = color,
            "border" => self.border = color,
            "modal_bg" => self.modal_bg = color,
            "sk_bg" => self.sk_bg = color,
            "sk_edge" => self.sk_edge = color,
            "sk_fg" => self.sk_fg = color,
            "sk_go" => self.sk_go = color,
            "sk_stop" => self.sk_stop = color,
            "good" => self.good = color,
            "warn" => self.warn = color,
            "error" => self.error = color,
            "accent" => self.accent = color,
            "card" => self.card = color,
            "card_ringing" => self.card_ringing = color,
            "card_held" => self.card_held = color,
            "card_dark" => self.card_dark = color,
            "tab_bg" => self.tab_bg = color,
            "tab_fg" => self.tab_fg = color,
            "load_track" => self.load_track = color,
            "load_fill" => self.load_fill = color,
            "wallpaper_fg" => self.wallpaper_fg = color,
            _ => return false,
        }
        true
    }
}

/// The least contrast ratio text may have against what is behind it (the
/// WCAG figure for body text).
pub const TEXT_CONTRAST: f32 = 4.5;
/// The least for an icon or another graphic that carries a state (the
/// WCAG figure for graphical objects).
pub const GRAPHIC_CONTRAST: f32 = 3.0;

/// The contrast ratio of two colors (WCAG 2): 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    fn luminance(c: Rgb) -> f32 {
        let lin = |v: u32| {
            let v = (v & 255) as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c >> 16) + 0.7152 * lin(c >> 8) + 0.0722 * lin(c)
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

impl Palette {
    /// True for dark text on a light background.
    pub fn is_light(&self) -> bool {
        let sum = |c: Rgb| (c >> 16) + ((c >> 8) & 255) + (c & 255);
        sum(self.fg) < sum(self.bg)
    }

    /// The fill of a text input: sunk below the background in a dark
    /// palette (stock's 0x141414 on the skin's own gray), the fill of
    /// dialogs in a light one.
    pub fn input_bg(&self) -> Rgb {
        if self.is_light() {
            return self.modal_bg;
        }
        let ch = |s: u32| (((self.bg >> s) & 255) * 20 / 33) << s;
        ch(16) | ch(8) | ch(0)
    }

    /// Every pairing of roles that the screens draw as text, or as a
    /// graphic that carries a state, on a fill: (what, foreground,
    /// background, the least contrast it should have).  From the drawing
    /// code: lists and menus, dialogs and notices, inputs, the softkey
    /// bar, the line-key tabs, and the call cards.  (The idle screen's
    /// text, `wallpaper_fg`, is on the wallpaper, which is not a role.)
    pub fn pairings(&self) -> Vec<(&'static str, Rgb, Rgb, f32)> {
        const TEXT: f32 = TEXT_CONTRAST;
        const GRAPHIC: f32 = GRAPHIC_CONTRAST;
        vec![
            ("text (fg) on the background (bg)", self.fg, self.bg, TEXT),
            ("secondary text on the background (bg)", self.secondary, self.bg, TEXT),
            ("title (title_fg) on the title bar (title_bg)", self.title_fg, self.title_bg, TEXT),
            ("selected row (sel_fg on sel_bg)", self.sel_fg, self.sel_bg, TEXT),
            ("text (fg) in a dialog (modal_bg)", self.fg, self.modal_bg, TEXT),
            ("text (fg) in an input", self.fg, self.input_bg(), TEXT),
            ("placeholder (secondary) in an input", self.secondary, self.input_bg(), TEXT),
            ("softkey label (sk_fg on sk_bg)", self.sk_fg, self.sk_bg, TEXT),
            // Stock's own green softkey is 3.5 under its label.
            ("label of the green softkey (sk_fg on sk_go)", self.sk_fg, self.sk_go, GRAPHIC),
            ("label of the red softkey (sk_fg on sk_stop)", self.sk_fg, self.sk_stop, TEXT),
            ("line-key label (tab_fg on tab_bg)", self.tab_fg, self.tab_bg, TEXT),
            ("active call card (fg on card)", self.fg, self.card, TEXT),
            ("ringing call card (fg on card_ringing)", self.fg, self.card_ringing, TEXT),
            ("held call card (fg on card_held)", self.fg, self.card_held, TEXT),
            ("dark part of a call card (fg on card_dark)", self.fg, self.card_dark, TEXT),
            ("error text on the background (bg)", self.error, self.bg, TEXT),
            ("error icon in a dialog (modal_bg)", self.error, self.modal_bg, GRAPHIC),
            ("warning icon (warn) in a dialog (modal_bg)", self.warn, self.modal_bg, GRAPHIC),
            ("information icon (accent) in a dialog (modal_bg)", self.accent, self.modal_bg, GRAPHIC),
            ("good on the background (bg)", self.good, self.bg, GRAPHIC),
            ("focus ring (accent) on the background (bg)", self.accent, self.bg, GRAPHIC),
            ("scroll thumb (secondary) on its track (border)", self.secondary, self.border, GRAPHIC),
        ]
    }

    /// The pairings with less contrast than they should have, as text
    /// for a log.
    pub fn low_contrast(&self) -> Vec<String> {
        self.pairings()
            .into_iter()
            .filter_map(|(what, fg, bg, least)| {
                let ratio = contrast(fg, bg);
                (ratio < least).then(|| format!("{what}: contrast {ratio:.1}, under {least}"))
            })
            .collect()
    }
}

impl Theme {
    /// The colors of this theme.
    pub fn palette(&self) -> Palette {
        Palette {
            bg: self.bg,
            fg: self.fg,
            title_bg: self.title_bg,
            title_fg: self.title_fg,
            sel_bg: self.sel_bg,
            sel_fg: self.sel_fg,
            secondary: self.secondary,
            border: self.border,
            modal_bg: self.modal_bg,
            sk_bg: self.sk_bg,
            sk_edge: self.sk_edge,
            sk_fg: self.sk_fg,
            sk_go: self.sk_go,
            sk_stop: self.sk_stop,
            good: self.good,
            warn: self.warn,
            error: self.error,
            accent: self.accent,
            card: self.card,
            card_ringing: self.card_ringing,
            card_held: self.card_held,
            card_dark: self.card_dark,
            tab_bg: self.tab_bg,
            tab_fg: self.tab_fg,
            load_track: self.load_track,
            load_fill: self.load_fill,
            wallpaper_fg: self.wallpaper_fg,
        }
    }

    /// This theme in other colors.
    fn with_palette(&self, p: &Palette) -> Theme {
        Theme {
            bg: p.bg,
            fg: p.fg,
            title_bg: p.title_bg,
            title_fg: p.title_fg,
            sel_bg: p.sel_bg,
            sel_fg: p.sel_fg,
            secondary: p.secondary,
            border: p.border,
            modal_bg: p.modal_bg,
            sk_bg: p.sk_bg,
            sk_edge: p.sk_edge,
            sk_fg: p.sk_fg,
            sk_go: p.sk_go,
            sk_stop: p.sk_stop,
            good: p.good,
            warn: p.warn,
            error: p.error,
            accent: p.accent,
            card: p.card,
            card_ringing: p.card_ringing,
            card_held: p.card_held,
            card_dark: p.card_dark,
            tab_bg: p.tab_bg,
            tab_fg: p.tab_fg,
            load_track: p.load_track,
            load_fill: p.load_fill,
            wallpaper_fg: p.wallpaper_fg,
            skin: self.skin,
            gray: self.gray,
            touch: self.touch,
            scale: self.scale,
            touch_min: self.touch_min,
            back_w: self.back_w,
            fonts: self.fonts,
            icons: self.icons,
            icon_data: self.icon_data,
            width: self.width,
            height: self.height,
            softkey_h: self.softkey_h,
            soft_x0: self.soft_x0,
            soft_pitch: self.soft_pitch,
            soft_w: self.soft_w,
            soft_y: self.soft_y,
            soft_h: self.soft_h,
            tab_x: self.tab_x,
            tab_w: self.tab_w,
            tab_h: self.tab_h,
            idle_tab_x: self.idle_tab_x,
            header_h: self.header_h,
            line_h: self.line_h,
            row_h: self.row_h,
            title_h: self.title_h,
            scroll_w: self.scroll_w,
            pad: self.pad,
            title_pad: self.title_pad,
            icon: self.icon,
        }
    }

    /// The fill of a text input on the color skin
    /// ([`Palette::input_bg`]).
    pub fn input_bg(&self) -> Rgb {
        self.palette().input_bg()
    }

    /// The color skin in the palette of a built-in wallpaper.  The default
    /// wallpaper's is the skin's own.
    pub fn for_wallpaper(wallpaper: &'static crate::wallpaper::Wallpaper) -> &'static Theme {
        COLOR.in_wallpaper(wallpaper)
    }

    /// This skin (the color skin's, or the touch skin's, own theme) in
    /// the palette of a built-in wallpaper; the skin's own theme when the
    /// palette is the skin's own.  A gray skin has no palettes: itself.
    pub fn in_wallpaper(&'static self, wallpaper: &'static crate::wallpaper::Wallpaper) -> &'static Theme {
        static THEMES: [std::sync::OnceLock<Vec<Theme>>; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
        let (base, slot): (&'static Theme, usize) = match self.skin {
            Skin::Color => (&COLOR, 0),
            Skin::Touch => (Self::for_skin(Skin::Touch), 1),
            Skin::SmallGray | Skin::LargeGray => return self,
        };
        let all = crate::wallpaper::all();
        let themes = THEMES[slot].get_or_init(|| all.iter().map(|w| base.with_palette(&w.palette())).collect());
        match all.iter().position(|w| w.id == wallpaper.id) {
            Some(i) if themes[i].palette() != base.palette() => &themes[i],
            _ => base,
        }
    }

    /// The color skin in `palette` (a palette file's).  Each distinct
    /// palette is kept for as long as the program runs; there are as many
    /// as there were palette files.
    pub fn color_in(palette: &Palette) -> &'static Theme {
        COLOR.in_palette(palette)
    }

    /// This skin (the color skin's, or the touch skin's, own theme) in
    /// `palette`, kept as [`Theme::color_in`] keeps its themes.  A gray
    /// skin has no palettes: itself.
    pub fn in_palette(&'static self, palette: &Palette) -> &'static Theme {
        static MADE: std::sync::Mutex<Vec<&'static Theme>> = std::sync::Mutex::new(Vec::new());
        let base: &'static Theme = match self.skin {
            Skin::Color => &COLOR,
            Skin::Touch => Self::for_skin(Skin::Touch),
            Skin::SmallGray | Skin::LargeGray => return self,
        };
        let mut made = MADE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(th) = made.iter().find(|th| th.skin == base.skin && th.palette() == *palette) {
            return th;
        }
        let th: &'static Theme = Box::leak(Box::new(base.with_palette(palette)));
        made.push(th);
        th
    }

    /// The theme of a skin.  A build without the feature `touch` (the
    /// firmware of the phones with keys) has no touch skin: it answers
    /// with the color skin, and is not for a D80 (`dphone` does not start
    /// as one).
    pub fn for_skin(skin: Skin) -> &'static Theme {
        match skin {
            Skin::SmallGray => &SMALL_GRAY,
            Skin::LargeGray => &LARGE_GRAY,
            Skin::Color => &COLOR,
            #[cfg(feature = "touch")]
            Skin::Touch => &TOUCH,
            #[cfg(not(feature = "touch"))]
            Skin::Touch => &COLOR,
        }
    }
    pub fn for_model(model: Model) -> &'static Theme {
        Self::for_skin(model.skin())
    }

    /// The touch skin: the screen is touched, and there are no keys.
    ///
    /// Always false, and known to the compiler to be, in a build without
    /// the feature `touch`: what the screens do only on the touch skin is
    /// behind this, and so is left out of that build with the skin's
    /// fonts and icons.  (Hence a function, where `gray` is a field.)
    #[inline(always)]
    pub const fn touch(&self) -> bool {
        cfg!(feature = "touch") && self.touch
    }

    pub fn font(&self, f: Font) -> &'static FontData {
        self.fonts[match f {
            Font::Normal => 0,
            Font::Bold => 1,
            Font::Small => 2,
            Font::Title => 3,
            Font::Large => 4,
        }]
    }

    /// The idle clock's digits (digits, colon, and space only).
    pub fn clock_font(&self) -> &'static FontData {
        self.fonts[5]
    }
    /// The idle screen's meridiem and date.
    pub fn date_font(&self) -> &'static FontData {
        self.fonts[6]
    }
    /// The number line of a call card (stock: 17 px on color).
    pub fn number_font(&self) -> &'static FontData {
        self.fonts[7]
    }
    /// The AM/PM beside the idle clock.
    pub fn meridiem_font(&self) -> &'static FontData {
        self.fonts[8]
    }

    pub fn text_width(&self, f: Font, s: &str) -> u16 {
        self.font(f).width(s).clamp(0, u16::MAX as i32) as u16
    }

    pub fn icon(&self, name: &str) -> Option<Icon> {
        let i = self.icons.binary_search_by(|r| r.name.cmp(name)).ok()?;
        Some(Icon { rec: &self.icons[i], data: self.icon_data })
    }

    pub fn theme_color(&self, c: ThemeColor) -> Rgb {
        match c {
            ThemeColor::Background => self.bg,
            ThemeColor::Foreground => self.fg,
            ThemeColor::TitleBackground => self.title_bg,
            ThemeColor::TitleForeground => self.title_fg,
            ThemeColor::SelectBackground => self.sel_bg,
            ThemeColor::SelectForeground => self.sel_fg,
            ThemeColor::Secondary => self.secondary,
            ThemeColor::Border => self.border,
            ThemeColor::ModalBackground => self.modal_bg,
            ThemeColor::SoftkeyBackground => self.sk_bg,
            ThemeColor::SoftkeyForeground => self.sk_fg,
            ThemeColor::Good => self.good,
            ThemeColor::Warning => self.warn,
            ThemeColor::Error => self.error,
        }
    }

    /// Resolves an app color; `None` means "do not paint".
    pub fn resolve(&self, c: Color, default: Option<Rgb>) -> Option<Rgb> {
        match c {
            Color::Default => default,
            Color::Transparent => None,
            Color::Theme(t) => Some(self.theme_color(t)),
            Color::Rgb(r, g, b) => Some(rgb(r, g, b)),
        }
    }

    /// A length given in pixels of the color skin, in this skin's: the
    /// same on the color skin, twice as many on the touch skin.  For the
    /// sizes the screens give in numbers (a dialog's width, an inset).
    #[inline(always)]
    pub const fn px(&self, color_px: i32) -> i32 {
        // (Without the touch skin, every skin's scale is 1.)
        if cfg!(feature = "touch") { color_px * self.scale } else { color_px }
    }

    /// How many rows of buttons `n` softkeys take on the touch skin: four
    /// to a row.
    pub const fn soft_rows(&self, n: usize) -> i32 {
        n.div_ceil(4) as i32
    }

    /// Button `i` of `n` softkeys on the touch skin: rows of four,
    /// the first row uppermost, the last row at the bottom of the screen.
    /// With four or fewer it is [`Theme::softkey_rect`].
    pub const fn soft_button(&self, i: usize, n: usize) -> IRect {
        let rows = self.soft_rows(n);
        let row = (i / 4) as i32;
        IRect::new(
            self.soft_x0 + (i % 4) as i32 * self.soft_pitch,
            self.soft_y - (rows - 1 - row) * self.softkey_h,
            self.soft_w,
            self.soft_h,
        )
    }

    /// The app content area on the main screen.
    pub fn content_h(&self) -> i32 {
        self.height - self.softkey_h
    }

    /// The outline of softkey shape `i` (0..4): the rectangle that holds
    /// it, including the pointed part on the color skin.
    pub fn softkey_rect(&self, i: usize) -> IRect {
        IRect::new(self.soft_x0 + i as i32 * self.soft_pitch, self.soft_y, self.soft_w, self.soft_h)
    }

    /// Line-key tab `i` (0 = beside line key 1), border included.
    pub fn tab_rect(&self, i: usize) -> IRect {
        self.tab_rect_from(self.tab_x, i)
    }

    /// Line-key tab `i` of a tab column whose left edge is `x`.
    pub fn tab_rect_from(&self, x: i32, i: usize) -> IRect {
        IRect::new(x, i as i32 * self.tab_h, self.width - x, self.tab_h)
    }

    /// The header band left of the tab column.
    pub fn header_rect(&self) -> IRect {
        self.header_rect_to(self.tab_x)
    }

    /// The header band left of a tab column whose left edge is `x`.
    pub fn header_rect_to(&self, x: i32) -> IRect {
        IRect::new(0, 0, x - if self.gray { 1 } else { 0 }, self.header_h)
    }
}
