//! The built-in wallpapers, described for the color skin (480 x 272) and
//! drawn from the same description for the touch skin's upright screen
//! (800 x 1280), composed for its shape ([`Fit`], [`with_fitted`]).
//!
//! A wallpaper is a small description (a gradient and a few layers of
//! shapes) that is drawn on the phone, in integers, when it is first
//! shown; the picture is kept until another wallpaper is shown, so a
//! repaint only copies it.  No bitmap is shipped.
//!
//! Each wallpaper comes with a palette for the whole system
//! ([`Wallpaper::palette`]): while it is selected, every screen of the
//! color skin is drawn in its colors.  The palette is derived from two
//! colors by one rule ([`Scheme`]); the first wallpaper's is the skin's
//! own.  The two together are what the phone calls a theme: the user
//! chooses a theme, and this module has its picture and its colors.
//!
//! Some wallpapers move, slowly, on the idle screen (the submodule
//! `motion`).
//!
//! The panel's depth is not known (the frame buffer has eight bits per
//! channel; the panel may show six), so gradients are computed with two
//! more bits than the frame buffer has and dithered by the size of one
//! six-bit step.  Flat designs are not dithered.

use crate::gfx::Rgb;
use crate::skin::{Palette, Theme};
use std::sync::Mutex;

/// The size every wallpaper is described at: the color skin's screen.
pub const WIDTH: i32 = 480;
pub const HEIGHT: i32 = 272;

/// How a wallpaper's description is laid out on a picture of some size:
/// the description's numbers are pixels of the color skin's screen
/// ([`WIDTH`] x [`HEIGHT`]), and this says what they are on the picture.
/// On the color skin's screen ([`Fit::NATIVE`]) every number is a pixel.
///
/// On a screen of another shape (the touch skin's, upright) the picture
/// is composed for that shape: it is not the color skin's picture turned,
/// cut, or stretched.
///
/// - A place is where it is in proportion, on each axis: what is a third
///   of the way across and a quarter of the way up the color skin's
///   screen is a third of the way across and a quarter of the way up this
///   one.  So the reach of a gradient (corner to corner) and of the
///   slanted bands is the screen's.
/// - The size of a thing in the composition (a circle, a glow, a wave,
///   the radius of a radial gradient) grows with the screen's diagonal,
///   so that it covers about as much of the screen as it does on the
///   color skin's ([`Fit::size`]).
/// - A pattern (threads, stripes, dots, contour lines, a field of
///   triangles) keeps its size to the eye: a pixel of it is a square of
///   [`Fit::grain`] pixels each way.
///
/// Slants keep their angles, and shapes their proportions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    /// The picture's size in pixels.
    pub w: i32,
    pub h: i32,
    /// The picture's pixels to one of the description's, for a size in
    /// the composition, in 1/256.
    size: i32,
    /// The picture's pixels to one of the description's, for a pattern.
    grain: i32,
}

impl Fit {
    /// The color skin's screen, which the descriptions are made for.
    pub const NATIVE: Fit = Fit { w: WIDTH, h: HEIGHT, size: 256, grain: 1 };

    /// A picture of `w` x `h` pixels.
    pub fn new(w: i32, h: i32) -> Fit {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) == (WIDTH, HEIGHT) {
            return Fit::NATIVE;
        }
        // The diagonals, in 1/256 pixel.
        let diagonal = |w: i32, h: i32| ((i64::from(w) * i64::from(w) + i64::from(h) * i64::from(h)) * 65536).isqrt();
        let size = (diagonal(w, h) * 256 / diagonal(WIDTH, HEIGHT)) as i32;
        // The whole number nearest to the lesser of the two ratios of the
        // sides (on the touch skin, 800 / 480: 2, the skin's own scale).
        let least = (i64::from(w) * i64::from(HEIGHT)).min(i64::from(h) * i64::from(WIDTH));
        let grain = ((2 * least + i64::from(WIDTH * HEIGHT)) / i64::from(2 * WIDTH * HEIGHT)).max(1) as i32;
        Fit { w, h, size: size.max(1), grain }
    }

    /// Whether this is the color skin's screen.
    pub fn is_native(&self) -> bool {
        *self == Fit::NATIVE
    }

    /// A place across the screen, in pixels of the description or in
    /// parts of them (sixteenths), in the same parts of the picture's.
    #[inline]
    fn x(&self, v: i32) -> i32 {
        v * self.w / WIDTH
    }

    /// A place down the screen, as [`Fit::x`].
    #[inline]
    fn y(&self, v: i32) -> i32 {
        v * self.h / HEIGHT
    }

    /// A size in the composition (a radius, the length and height of a
    /// wave, how far a thing moves), in any parts of a pixel.
    #[inline]
    fn size(&self, v: i32) -> i32 {
        (v * self.size + 128) >> 8
    }

    /// A size in a pattern.
    #[inline]
    fn grain(&self, v: i32) -> i32 {
        v * self.grain
    }

    /// Where a circle of the description is, in sixteenths of a pixel of
    /// the picture: its center and its radius.
    fn circle(&self, (cx, cy, r, ..): Circle) -> (i32, i32, i32) {
        (self.x(cx * 16), self.y(cy * 16), self.size(r * 16))
    }

    /// The span of `x + y * num / den` over the screen, in sixteenths of a
    /// pixel: where it begins, and how far it goes.
    fn slant_span(&self, num: i32, den: i32) -> (i32, i32) {
        let slant = self.h * 16 * num / den;
        (slant.min(0), self.w * 16 + slant.abs())
    }

    /// A band of the description ([`Layer::Bands`]): where it begins and
    /// how wide it is, in sixteenths of a pixel of the picture, along
    /// `x + y * num / den`.  It is where it is in proportion along the span
    /// of that over the screen.
    fn band(&self, num: i32, den: i32, start: i32, width: i32) -> (i32, i32) {
        let (lo, span) = Fit::NATIVE.slant_span(num, den);
        let (to_lo, to_span) = self.slant_span(num, den);
        let along = |v: i32| (i64::from(v) * i64::from(to_span) / i64::from(span)) as i32;
        (to_lo + along(start * 16 - lo), along(width * 16))
    }
}

/// The groups the wallpapers are listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Facets,
    Gradients,
    Bubbles,
    Modern,
    Classic,
    Solids,
}

impl Family {
    pub const ALL: [Family; 6] =
        [Family::Facets, Family::Gradients, Family::Bubbles, Family::Modern, Family::Classic, Family::Solids];

    pub fn name(self) -> &'static str {
        match self {
            Family::Facets => "Facets",
            Family::Gradients => "Gradients",
            Family::Bubbles => "Bubbles",
            Family::Modern => "Modern",
            Family::Classic => "Classic",
            Family::Solids => "Solids",
        }
    }
}

/// Where a wallpaper's palette comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// The color skin's own palette.
    Skin,
    /// A dark palette (light text) derived from two colors: `primary`, of
    /// title bars, the selection, and an active call, and `surface`, the
    /// background of menus.  See [`Scheme::palette`].
    Dark { primary: Rgb, surface: Rgb },
    /// A light palette (dark text) derived from the same two colors.
    Light { primary: Rgb, surface: Rgb },
    /// Every role chosen by hand.
    Hand(Palette),
}

/// Adds `d` to every channel.
fn shift(c: Rgb, d: i32) -> Rgb {
    let ch = |s: u32| {
        let v = ((c >> s) & 255) as i32 + d;
        (v.clamp(0, 255) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// `n` eighths of the way from `a` to `b`.
fn toward(a: Rgb, b: Rgb, n: u32) -> Rgb {
    let ch = |s: u32| ((((a >> s) & 255) * (8 - n) + ((b >> s) & 255) * n + 4) / 8) << s;
    ch(16) | ch(8) | ch(0)
}

impl Scheme {
    /// The whole palette.  The rule keeps the relations the color skin's
    /// own colors have (its palette is what `Dark` gives for its blue and
    /// its gray, but for three colors chosen by hand):
    ///
    /// - Dark: the background is `surface`; raised things are `surface`
    ///   lightened (dialogs and softkeys a little, line-key tabs more,
    ///   borders and the softkey outline most); text is the skin's light
    ///   gray, secondary text a quarter of the way to the background; the
    ///   selection and an active call's card are `primary`, the title bar
    ///   three quarters of it, the accent `primary` lightened; white text
    ///   on those.
    /// - Light: the background is `surface`; raised things are lighter,
    ///   borders darker; text is a dark ink; the title bar and the
    ///   selection are as in Dark, with white text; the call cards and the
    ///   green and red softkeys are light tints, since their text is the
    ///   ink.
    ///
    /// The colors with a meaning stay what they are: green for go and
    /// good, red for stop and error, amber for a warning, gray-blue for a
    /// held call (darker shades of each in a light palette, where they are
    /// text on a light background).
    pub fn palette(self, skin: &Palette) -> Palette {
        const WHITE: Rgb = 0xFFFFFF;
        match self {
            Scheme::Skin => *skin,
            Scheme::Hand(palette) => palette,
            Scheme::Dark { primary, surface } => Palette {
                bg: surface,
                fg: skin.fg,
                title_bg: toward(primary, 0, 2),
                title_fg: WHITE,
                sel_bg: primary,
                sel_fg: WHITE,
                secondary: toward(skin.fg, surface, 2),
                border: shift(surface, 0x2C),
                modal_bg: shift(surface, 0x0D),
                sk_bg: shift(surface, 0x0D),
                sk_edge: shift(surface, 0x54),
                sk_fg: skin.fg,
                accent: toward(primary, WHITE, 4),
                card: primary,
                card_dark: surface,
                tab_bg: shift(surface, 0x21),
                tab_fg: skin.fg,
                ..*skin
            },
            Scheme::Light { primary, surface } => {
                let ink = 0x1B1E24;
                Palette {
                    bg: surface,
                    fg: ink,
                    wallpaper_fg: ink,
                    title_bg: toward(primary, 0, 2),
                    title_fg: WHITE,
                    sel_bg: primary,
                    sel_fg: WHITE,
                    secondary: toward(ink, surface, 2),
                    border: shift(surface, -0x30),
                    modal_bg: toward(surface, WHITE, 5),
                    sk_bg: toward(surface, WHITE, 5),
                    sk_edge: shift(surface, -0x58),
                    sk_fg: ink,
                    sk_go: 0xA5D6A7,
                    sk_stop: 0xEF9A9A,
                    good: 0x1B5E20,
                    warn: 0x8A4B00,
                    error: 0xB71C1C,
                    accent: primary,
                    card: toward(primary, WHITE, 6),
                    card_ringing: 0xA5D6A7,
                    card_held: 0xCFD8DC,
                    card_dark: shift(surface, -0x18),
                    tab_bg: toward(surface, WHITE, 5),
                    tab_fg: ink,
                    ..*skin
                }
            }
        }
    }
}

/// One built-in wallpaper.
pub struct Wallpaper {
    /// The name in the configuration (`dusk`, `deep-sea`).
    pub id: &'static str,
    /// The name shown.
    pub name: &'static str,
    pub family: Family,
    /// Where its palette comes from.
    pub scheme: Scheme,
    /// Roles set by hand where the rule's color is not the best one.
    tweaks: &'static [(&'static str, Rgb)],
    spec: Spec,
}

impl Wallpaper {
    /// The palette of every screen while this wallpaper is selected.
    pub fn palette(&self) -> Palette {
        let mut p = self.scheme.palette(&Theme::for_skin(opendeskphone_state::Skin::Color).palette());
        for (role, color) in self.tweaks {
            let known = p.set(role, *color);
            debug_assert!(known, "{}: no role {role}", self.id);
        }
        p
    }

    /// Draws the wallpaper: `WIDTH * HEIGHT` pixels as `0x00RRGGBB`,
    /// row-major.  The same pixels every time.
    pub fn render(&self) -> Vec<u32> {
        render(&self.spec, &Fit::NATIVE)
    }

    /// Draws the wallpaper for a screen of `w` x `h` pixels, composed for
    /// its shape ([`Fit`]): `w * h` pixels as `0x00RRGGBB`, row-major.  At
    /// the color skin's size, [`Wallpaper::render`].
    pub fn render_sized(&self, w: i32, h: i32) -> Vec<u32> {
        render(&self.spec, &Fit::new(w, h))
    }

    /// Calls `f` with the wallpaper's pixels, drawing them first unless
    /// this wallpaper was the last one used.  One picture is kept.
    pub fn with_pixels<R>(&'static self, f: impl FnOnce(&[u32]) -> R) -> R {
        static KEPT: Mutex<Option<(&'static str, Vec<u32>)>> = Mutex::new(None);
        let mut kept = KEPT.lock().unwrap_or_else(|p| p.into_inner());
        if kept.as_ref().is_none_or(|(id, _)| *id != self.id) {
            // The old picture goes before the new one is made.
            *kept = None;
            let start = std::time::Instant::now();
            *kept = Some((self.id, self.render()));
            log::debug!("wallpaper {} drawn in {} ms", self.id, start.elapsed().as_millis());
        }
        f(&kept.as_ref().expect("just set").1)
    }

    /// The time from one frame to the next, in milliseconds, of a
    /// wallpaper that moves on the idle screen; `None` for one that is
    /// still.
    pub fn frame_ms(&self) -> Option<u32> {
        self.spec.moves.as_ref().map(|m| m.every_ms)
    }

    /// [`Wallpaper::frame_ms`] on a screen of `w` x `h` pixels.  On the
    /// color skin's screen it is the same.  On a larger one the motion
    /// goes as far as the composition is large, and so farther in pixels
    /// from one frame to the next: where a frame would move something
    /// farther than a pixel, frames come as often as keeps it within one
    /// (on the touch skin's 800 x 1280, the flow's: twice as often).
    ///
    /// What a frame changes grows with the screen's area (the ring of a
    /// circle; a flow changes every pixel), and on the touch skin a frame
    /// costs 2.5 to 24 times what it costs on the color skin's screen, up
    /// to a few milliseconds in the simulator (the screen brought up to
    /// the frame as the UI does it); the D80's processor affords that.
    pub fn frame_ms_sized(&self, w: i32, h: i32) -> Option<u32> {
        motion::every_ms(&self.spec, &Fit::new(w, h))
    }

    /// The farthest anything in the picture goes from one frame of the
    /// motion to the next, over a cycle, in sixteenths of a pixel (rounded
    /// up; for a flow, a bound from the fastest its waves go); `None` for
    /// a wallpaper that is still.
    pub fn farthest_step(&'static self) -> Option<u64> {
        self.farthest_step_sized(WIDTH, HEIGHT)
    }

    /// [`Wallpaper::farthest_step`] on a screen of `w` x `h` pixels
    /// ([`Wallpaper::render_sized`]).
    pub fn farthest_step_sized(&'static self, w: i32, h: i32) -> Option<u64> {
        motion::Moving::new(self.id, &self.spec, Fit::new(w, h)).map(|m| m.farthest())
    }

    /// The picture at `t_ms` (any clock that counts milliseconds) of a
    /// wallpaper that moves; the still picture of one that does not.
    /// The same pixels for the same time, and again one [`CYCLE_MS`]
    /// later.
    pub fn render_at(&'static self, t_ms: u64) -> Vec<u32> {
        self.render_at_sized(WIDTH, HEIGHT, t_ms)
    }

    /// [`Wallpaper::render_at`] on a screen of `w` x `h` pixels
    /// ([`Wallpaper::render_sized`]).
    pub fn render_at_sized(&'static self, w: i32, h: i32, t_ms: u64) -> Vec<u32> {
        let fit = Fit::new(w, h);
        match motion::Moving::new(self.id, &self.spec, fit) {
            Some(mut moving) => moving.frame(t_ms).to_vec(),
            None => render(&self.spec, &fit),
        }
    }

    /// Calls `f` with the picture at `t_ms`, as [`Wallpaper::render_at`]
    /// has it.  What one frame needs from the frame before is kept (for
    /// one wallpaper), so that a frame draws only what moved.
    pub fn with_frame<R>(&'static self, t_ms: u64, f: impl FnOnce(&[u32]) -> R) -> R {
        self.with_frame_and_changes(t_ms, |frame, _| f(frame))
    }

    /// [`Wallpaper::with_frame`], and what the frame changed
    /// ([`Changes`]): with it, whoever has the frame before on a screen
    /// can bring the screen up to this frame without looking at the rest
    /// of the picture.  For a wallpaper that is still: the still picture,
    /// and no frame before.
    pub fn with_frame_and_changes<R>(&'static self, t_ms: u64, f: impl FnOnce(&[u32], Changes) -> R) -> R {
        self.with_frame_and_changes_sized(WIDTH, HEIGHT, t_ms, f)
    }

    /// [`Wallpaper::with_frame_and_changes`] on a screen of `w` x `h`
    /// pixels ([`Wallpaper::render_sized`]).  What is kept is kept for one
    /// wallpaper at one size.
    pub fn with_frame_and_changes_sized<R>(&'static self, w: i32, h: i32, t_ms: u64, f: impl FnOnce(&[u32], Changes) -> R) -> R {
        let fit = Fit::new(w, h);
        let mut moving = MOVING.lock().unwrap_or_else(|p| p.into_inner());
        if moving.as_ref().is_none_or(|m| m.id != self.id || m.fit != fit) {
            *moving = None;
            *moving = motion::Moving::new(self.id, &self.spec, fit);
        }
        let Some(m) = moving.as_mut() else {
            drop(moving);
            let still = Changes { serial: motion::next_serial(), before: 0, runs: &[] };
            return if fit.is_native() { self.with_pixels(|pixels| f(pixels, still)) } else { with_fitted(Backdrop::Builtin(self), w, h, |pixels| f(pixels, still)) };
        };
        let before = m.serial;
        let (frame, runs, serial) = m.frame_and_changes(t_ms);
        f(frame, Changes { serial, before, runs })
    }

    /// A screen of `w` x `h` pixels that shows frame `serial`
    /// ([`Changes::serial`]) of this wallpaper's motion draws part of
    /// itself again over that frame with this.  The frame is kept only
    /// until the next one is drawn, and after that this returns `None`.
    pub fn with_drawn_frame_sized<R>(&'static self, w: i32, h: i32, serial: u64, f: impl FnOnce(&[u32]) -> R) -> Option<R> {
        let moving = MOVING.lock().unwrap_or_else(|p| p.into_inner());
        let m = moving.as_ref().filter(|m| m.id == self.id && m.fit == Fit::new(w, h) && m.serial == serial)?;
        Some(f(m.drawn()))
    }
}

/// The wallpaper in motion keeps its last frame here, and what its next
/// frame is drawn from ([`Wallpaper::with_frame_and_changes_sized`]).
static MOVING: Mutex<Option<motion::Moving>> = Mutex::new(None);

/// What a frame of a wallpaper in motion changed
/// ([`Wallpaper::with_frame_and_changes`]).
#[derive(Clone, Copy)]
pub struct Changes<'a> {
    /// This frame's number: no other frame, of any wallpaper, has it.
    pub serial: u64,
    /// The number of the frame the changes are from; 0 when there was
    /// none (the whole picture is new).
    pub before: u64,
    /// The pixels that may differ from that frame's.
    pub runs: &'a [Run],
}

/// Pixels side by side in a row of a wallpaper: the columns `x0..x1` of
/// row `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
}

/// The palette a palette file describes: derived from its `primary` and
/// `surface` (light or dark) if it gives either, with the skin's blue or
/// the skin's (for a light palette, a light) gray for the one it does not
/// give; else the skin's own, or a light one of the skin's blue when the
/// file only says `light`.  Then each role it names is set; a name that
/// is not a role is returned.
pub fn palette_from(light: bool, primary: Option<Rgb>, surface: Option<Rgb>, roles: &[(&str, Rgb)]) -> (Palette, Vec<String>) {
    let skin = Theme::for_skin(opendeskphone_state::Skin::Color).palette();
    let scheme = if light {
        Scheme::Light { primary: primary.unwrap_or(skin.sel_bg), surface: surface.unwrap_or(0xECECEC) }
    } else if primary.is_some() || surface.is_some() {
        Scheme::Dark { primary: primary.unwrap_or(skin.sel_bg), surface: surface.unwrap_or(skin.bg) }
    } else {
        Scheme::Skin
    };
    let mut palette = scheme.palette(&skin);
    // Text on the wallpaper is the text color unless the file says
    // otherwise.
    if let Some((_, fg)) = roles.iter().find(|(role, _)| *role == "fg") {
        palette.wallpaper_fg = *fg;
    }
    let unknown = roles.iter().filter(|(role, color)| !palette.set(role, *color)).map(|(role, _)| role.to_string()).collect();
    (palette, unknown)
}

/// A wallpaper picture that is not built in (the provisioning server's
/// `wallpaper_file`), at the screen's size, with the palette that came
/// with it, if one did.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    /// `WIDTH * HEIGHT` pixels as `0x00RRGGBB`, row-major.
    pixels: Vec<u32>,
    /// The palette file's colors; `None` is the color skin's own palette.
    pub palette: Option<Palette>,
    /// The picture as it came, for a screen of another size or shape than
    /// the color skin's ([`with_fitted`]): an upright picture made for the
    /// touch skin's screen would be mostly cut away on the way to the
    /// color skin's, and a small one blurred.  (The decoded pixels are
    /// shared, not copied; they are at most `image::MAX_PIXELS`.)
    source: crate::image::Image,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture").field("palette", &self.palette).finish_non_exhaustive()
    }
}

impl Picture {
    /// From decoded RGBA pixels of any size: scaled to cover the screen
    /// (the excess cut off evenly), transparent parts over black.
    pub fn new(image: &crate::image::Image, palette: Option<Palette>) -> Picture {
        Picture { pixels: cover(image, WIDTH, HEIGHT, false), palette, source: image.clone() }
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// The picture scaled to cover a screen of `w` x `h` pixels, the
    /// excess cut off evenly, from the picture as it came: an upright
    /// picture fills an upright screen whole.  A picture is never turned.
    /// Where the screen has more pixels than the part of the picture shown
    /// on it, they are between the picture's in proportion; where it has
    /// fewer, each is the mean of those it covers.
    pub fn pixels_sized(&self, w: i32, h: i32) -> Vec<u32> {
        if (w, h) == (WIDTH, HEIGHT) {
            return self.pixels.clone();
        }
        cover(&self.source, w, h, true)
    }
}

/// `image` scaled to cover `sw` x `sh` pixels, the excess cut off evenly,
/// transparent parts over black: each pixel the mean of the image's that
/// it covers (where it covers less than one, that one), or with `smooth`,
/// where the image is enlarged, between the image's in proportion.
fn cover(image: &crate::image::Image, sw: i32, sh: i32, smooth: bool) -> Vec<u32> {
    let (w, h) = (i64::from(image.width.max(1)), i64::from(image.height.max(1)));
    let (sw, sh) = (i64::from(sw.max(1)), i64::from(sh.max(1)));
    // A channel of a pixel of the image, over black.
    let channel = |sx: i64, sy: i64, i: usize| {
        let p = &image.pixels[((sy * w + sx) * 4) as usize..];
        u64::from(p[i]) * u64::from(p[3]) / 255
    };
    // The source rectangle with the screen's shape, in 1/65536 pixel.
    let (cw, ch) = if w * sh >= h * sw { (h * sw * 65536 / sh, h * 65536) } else { (w * 65536, w * sh * 65536 / sw) };
    let (x0, y0) = ((w * 65536 - cw) / 2, (h * 65536 - ch) / 2);
    let mut pixels = Vec::with_capacity((sw * sh) as usize);
    if smooth && cw < sw * 65536 {
        // Enlarged: the four pixels around each one's center, by how near.
        let (step_x, step_y) = (cw / sw, ch / sh);
        for y in 0..sh {
            let fy = (y0 + y * step_y + step_y / 2 - 32768).clamp(0, (h - 1) * 65536);
            let (ya, ty) = (fy >> 16, ((fy & 0xFFFF) >> 8) as u64);
            let yb = (ya + 1).min(h - 1);
            for x in 0..sw {
                let fx = (x0 + x * step_x + step_x / 2 - 32768).clamp(0, (w - 1) * 65536);
                let (xa, tx) = (fx >> 16, ((fx & 0xFFFF) >> 8) as u64);
                let xb = (xa + 1).min(w - 1);
                let ch = |i: usize| {
                    let top = channel(xa, ya, i) * (256 - tx) + channel(xb, ya, i) * tx;
                    let bottom = channel(xa, yb, i) * (256 - tx) + channel(xb, yb, i) * tx;
                    ((top * (256 - ty) + bottom * ty + 32768) >> 16) as u32
                };
                pixels.push(ch(0) << 16 | ch(1) << 8 | ch(2));
            }
        }
        return pixels;
    }
    for y in 0..sh {
        let (ya, yb) = ((y0 + y * ch / sh) >> 16, ((y0 + (y + 1) * ch / sh - 1) >> 16).min(h - 1));
        for x in 0..sw {
            let (xa, xb) = ((x0 + x * cw / sw) >> 16, ((x0 + (x + 1) * cw / sw - 1) >> 16).min(w - 1));
            // The mean of the source pixels this one covers.
            let (mut sum, mut n) = ([0u64; 3], 0u64);
            for sy in ya..=yb.max(ya) {
                for sx in xa..=xb.max(xa) {
                    for (i, s) in sum.iter_mut().enumerate() {
                        *s += channel(sx, sy, i);
                    }
                    n += 1;
                }
            }
            pixels.push(((sum[0] / n) as u32) << 16 | ((sum[1] / n) as u32) << 8 | (sum[2] / n) as u32);
        }
    }
    pixels
}

/// What is behind the idle and call screens.
#[derive(Clone, Copy)]
pub enum Backdrop<'a> {
    Builtin(&'static Wallpaper),
    /// A built-in wallpaper in motion, at a time in milliseconds
    /// ([`Wallpaper::with_frame`]).
    Moving(&'static Wallpaper, u64),
    Picture(&'a Picture),
}

/// Calls `f` with the still picture of `backdrop` for a screen of `w` x
/// `h` pixels that is not the size the wallpapers are described at (the
/// touch skin's 800 x 1280): a built-in wallpaper drawn from its
/// description for that screen ([`Wallpaper::render_sized`]), or a
/// picture scaled to cover it ([`Picture::pixels_sized`]).  One such
/// picture is kept.
pub fn with_fitted<R>(backdrop: Backdrop, w: i32, h: i32, f: impl FnOnce(&[u32]) -> R) -> R {
    static KEPT: Mutex<Option<(String, i32, i32, Vec<u32>)>> = Mutex::new(None);
    let key = match backdrop {
        Backdrop::Builtin(wallpaper) | Backdrop::Moving(wallpaper, _) => format!("builtin {}", wallpaper.id),
        Backdrop::Picture(picture) => {
            // Which picture: where it is, and a sample of what it is.
            let sample = picture.pixels().iter().step_by(97).fold(0u32, |a, p| a.rotate_left(5) ^ p);
            format!("picture {:p} {sample:08x}", picture)
        }
    };
    let mut kept = KEPT.lock().unwrap_or_else(|p| p.into_inner());
    if kept.as_ref().is_none_or(|(k, kw, kh, _)| *k != key || (*kw, *kh) != (w, h)) {
        // The old picture goes before the new one is made.
        *kept = None;
        let start = std::time::Instant::now();
        let fitted = match backdrop {
            Backdrop::Builtin(wallpaper) | Backdrop::Moving(wallpaper, _) => wallpaper.render_sized(w, h),
            Backdrop::Picture(picture) => picture.pixels_sized(w, h),
        };
        log::debug!("wallpaper {key} drawn for {w} x {h} in {} ms", start.elapsed().as_millis());
        *kept = Some((key, w, h, fitted));
    }
    f(&kept.as_ref().expect("just set").3)
}

/// Every wallpaper, grouped by family in the order of [`Family::ALL`].
pub fn all() -> &'static [Wallpaper] {
    &ALL
}

/// The wallpaper used when none, or an unknown one, is named
/// ([`opendeskphone_state::DEFAULT_THEME`]).
pub fn default() -> &'static Wallpaper {
    static DEFAULT: std::sync::OnceLock<&'static Wallpaper> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(|| find(opendeskphone_state::DEFAULT_THEME).unwrap_or(&ALL[0]))
}

/// The first wallpaper this firmware had, whose palette is the color
/// skin's own: the start-up screen's artwork is drawn for it.
pub fn first() -> &'static Wallpaper {
    &ALL[0]
}

/// The wallpaper with configuration name `id` (case does not matter).
pub fn find(id: &str) -> Option<&'static Wallpaper> {
    let id = id.trim();
    ALL.iter().find(|w| w.id.eq_ignore_ascii_case(id))
}

/// [`find`], or the default.
pub fn find_or_default(id: &str) -> &'static Wallpaper {
    find(id).unwrap_or(default())
}

// ------------------------------------------------------------ description

/// Gradient stops: (position in percent, color).
type Stops = &'static [(i32, Rgb)];

enum Grad {
    Solid(Rgb),
    /// Along the direction (`ax`, `ay`), from one corner of the screen to
    /// the opposite one.  `|ax| + |ay|` is at most 8.
    Linear { ax: i32, ay: i32, stops: Stops },
    /// Outward from (`cx`, `cy`) to radius `r`.
    Radial { cx: i32, cy: i32, r: i32, stops: Stops },
}

enum Base {
    Grad(Grad),
    /// A field of flat triangles on a jittered grid of `cell` pixels, each
    /// in the gradient's color at its center, lighter or darker by up to
    /// `amp`.
    Poly { grad: Grad, cell: i32, jitter: i32, seed: u32, amp: i32 },
    /// A flowing field of color: every point has a position along the
    /// stops, and the flow bends the lines of equal position.
    Flow { flow: Flow, stops: Stops },
    /// The first wallpaper this firmware had, pixel for pixel.
    Original,
}

/// How the colors of [`Base::Flow`] run: from a straight run across the
/// screen along (`ax`, `ay`) (as [`Grad::Linear`], neither negative),
/// bent.  Amounts are out of 4096, the whole run.
enum Flow {
    /// Bent by waves: (amount, the wave's length in pixels along x and
    /// along y (0: it does not vary that way), phase in 1/256 turn).
    Ribbons { ax: i32, ay: i32, waves: &'static [(i32, i32, i32, i32)] },
}

/// A slanted band: (start, width, opacity at its leading edge, opacity at
/// its trailing edge), opacities out of 256.
type Band = (i32, i32, i32, i32);

/// A stripe of a repeating set: (offset, width, color, opacity of 256).
type Stripe = (i32, i32, Rgb, i32);

/// One circle: (center x, center y, radius, color, opacity of 256).
type Circle = (i32, i32, i32, Rgb, i32);

struct Wave {
    y: i32,
    amp: i32,
    period: i32,
    /// In 1/256 turn.
    phase: i32,
    amp2: i32,
    period2: i32,
    color: Rgb,
    alpha: i32,
    /// Pixels over which the fill fades below the crest; 0 fills to the
    /// bottom.
    fade: i32,
}

enum Weave {
    /// Four dark and four light threads each way in a 2/2 twill; a thread
    /// is `unit` pixels.
    Houndstooth { unit: i32 },
    /// Columns `col` wide of diagonal stripes, leaning alternately.
    Herringbone { col: i32, period: i32 },
    /// Zigzag bands: `px` across one zigzag, `py` from band to band.
    Chevron { px: i32, py: i32 },
    /// Diamonds `w` wide and `h` high, alternately filled, crossed by
    /// diagonal lines `line` sixteenths of a pixel wide (the second color).
    Argyle { w: i32, h: i32, line: i32 },
}

enum Layer {
    /// Bands along `x + y * num / den`.  `add`: the color is added, scaled
    /// by the opacity, instead of blended in.
    Bands { num: i32, den: i32, add: bool, color: Rgb, bands: &'static [Band] },
    /// A soft-edged spot of color: full `alpha` (of 256) at the center,
    /// none at radius `r`.
    Glow { cx: i32, cy: i32, r: i32, color: Rgb, alpha: i32 },
    /// Circles.  `soft`: the width of the edge in percent of the radius (0
    /// is a sharp edge).  `rim`: opacity of a thin ring just inside the
    /// edge.
    Circles { soft: i32, rim: i32, circles: &'static [Circle] },
    /// `count` circles placed by a generator seeded with `seed`, radii and
    /// opacities drawn from the ranges, colors from the list.
    Bubbles { seed: u32, count: u32, r: (i32, i32), alpha: (i32, i32), soft: i32, rim: i32, colors: &'static [Rgb] },
    Waves(&'static [Wave]),
    /// Contour lines of a smooth random terrain: `scale` pixels per
    /// feature, `levels` lines from the lowest point to the highest, every
    /// `major`th one drawn with `major_alpha`.
    Contours { seed: u32, scale: i32, levels: i32, color: Rgb, alpha: i32, major: i32, major_alpha: i32 },
    /// A grid of dots `px` by `py` apart with radius `r` in sixteenths of
    /// a pixel; `stagger` shifts every other row by half a step.
    Dots { px: i32, py: i32, r: i32, stagger: bool, color: Rgb, alpha: i32 },
    /// Vertical or horizontal stripes repeating every `repeat` pixels.
    Stripes { vertical: bool, repeat: i32, stripes: &'static [Stripe] },
    Weave { kind: Weave, color: Rgb, alpha: i32, color2: Rgb, alpha2: i32 },
}

/// How a wallpaper moves on the idle screen.  Each motion comes back to
/// where it began: its speed is a whole number of turns in one cycle
/// ([`CYCLE_MS`]).
enum Motion {
    /// The waves of a [`Layer::Waves`] rise and fall.  For each wave:
    /// (how far each way in sixteenths of a pixel, turns in a cycle, where
    /// in its turn it begins in 1/256 turn).
    Bob(&'static [(i32, i32, i32)]),
    /// The circles of [`Layer::Circles`] and [`Layer::Bubbles`] wander
    /// around their places, each on a path of its own: at most `reach`
    /// pixels to either side (across, up and down), `turns.0` to
    /// `turns.1` turns in a cycle.
    Wander { reach: (i32, i32), turns: (i32, i32) },
    /// The waves of a [`Flow::Ribbons`] run on: for each wave, the turns
    /// in a cycle (negative: the other way).
    Run(&'static [i32]),
}

/// A wallpaper's motion, and the time from one frame to the next.
struct Moves {
    every_ms: u32,
    motion: Motion,
}

struct Spec {
    base: Base,
    layers: &'static [Layer],
    dither: bool,
    /// The motion on the idle screen, for the wallpapers that have one.
    moves: Option<Moves>,
}

// ---------------------------------------------------------------- drawing

/// A color while drawing: each channel in quarter steps of the frame
/// buffer's (0..=1020).
type Px = [i32; 3];

const fn px(c: Rgb) -> Px {
    [((c >> 16) & 255) as i32 * 4, ((c >> 8) & 255) as i32 * 4, (c & 255) as i32 * 4]
}

#[inline]
fn unpack(p: u32) -> Px {
    [(p >> 20) as i32, ((p >> 10) & 1023) as i32, (p & 1023) as i32]
}

#[inline]
fn pack(c: Px) -> u32 {
    ((c[0] as u32) << 20) | ((c[1] as u32) << 10) | c[2] as u32
}

/// One: full opacity, and the unit of gradient positions.
const ONE: i32 = 4096;

fn mix(a: Px, b: Px, t: i32) -> Px {
    [0, 1, 2].map(|i| a[i] + (((b[i] - a[i]) * t + ONE / 2) >> 12))
}

/// Blends `c` into the pixel with opacity `t` of [`ONE`].
#[inline]
fn over(p: &mut u32, c: Px, t: i32) {
    if t > 0 {
        *p = pack(mix(unpack(*p), c, t.min(ONE)));
    }
}

/// Adds `t` of [`ONE`] of `c` to the pixel.
#[inline]
fn add(p: &mut u32, c: Px, t: i32) {
    if t > 0 {
        let d = unpack(*p);
        *p = pack([0, 1, 2].map(|i| (d[i] + ((c[i] * t + ONE / 2) >> 12)).clamp(0, 1020)));
    }
}

/// A well-mixed 32-bit value from a lattice point and a seed.
fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

impl Flow {
    /// The position along the stops (0..=[`ONE`]) of pixel (`x`, `y`).
    fn at(&self, f: &Fit, x: i32, y: i32) -> i32 {
        let straight = |ax: i32, ay: i32| (x * ax + y * ay) * ONE / (f.w * ax + f.h * ay);
        let t = match *self {
            Flow::Ribbons { ax, ay, waves } => {
                let mut t = straight(ax, ay);
                for &(amount, px, py, phase) in waves {
                    let (px, py) = (f.size(px), f.size(py));
                    let turn = if px != 0 { x * 65536 / px } else { 0 } + if py != 0 { y * 65536 / py } else { 0 } + phase * 256;
                    t += (amount * sin(turn)) >> 12;
                }
                t
            }
        };
        t.clamp(0, ONE)
    }
}

/// The sine of `turn` / 65536 of a full turn, times [`ONE`].
fn sin(turn: i32) -> i32 {
    // sin(pi z) for z in -1..1 is close to 4z(1 - |z|); one correction
    // step brings the error under a tenth of a percent.
    let z = (turn & 0xFFFF) - 32768;
    let s = (z * (32768 - z.abs())) >> 13;
    let s = s + (((s * s.abs()) >> 15) - s) * 225 / 1000;
    -s >> 3
}

fn stops_at(stops: Stops, t: i32) -> Px {
    let pos = |i: usize| stops[i].0 * ONE / 100;
    if t <= pos(0) {
        return px(stops[0].1);
    }
    for i in 1..stops.len() {
        if t <= pos(i) {
            let (a, z) = (pos(i - 1), pos(i));
            return mix(px(stops[i - 1].1), px(stops[i].1), (t - a) * ONE / (z - a).max(1));
        }
    }
    px(stops[stops.len() - 1].1)
}

impl Grad {
    /// The color at a point given in sixteenths of a pixel.
    fn at(&self, f: &Fit, sx: i32, sy: i32) -> Px {
        match *self {
            Grad::Solid(c) => px(c),
            Grad::Linear { ax, ay, stops } => {
                let (w, h) = (f.w * 16 * ax, f.h * 16 * ay);
                let lo = 0.min(w).min(h).min(w + h);
                let hi = 0.max(w).max(h).max(w + h);
                let p = sx * ax + sy * ay;
                stops_at(stops, ((i64::from((p - lo).clamp(0, hi - lo)) * i64::from(ONE)) / i64::from(hi - lo)) as i32)
            }
            Grad::Radial { cx, cy, r, stops } => {
                let (dx, dy) = (sx - f.x(cx * 16), sy - f.y(cy * 16));
                let d = ((dx * dx + dy * dy) as u32).isqrt() as i32;
                stops_at(stops, (d * ONE / f.size(r * 16)).min(ONE))
            }
        }
    }
}

/// The circles of a [`Layer::Bubbles`], in the order they are drawn: the
/// same circles every time (a xorshift generator).
fn bubbles(seed: u32, count: u32, r: (i32, i32), alpha: (i32, i32), colors: &[Rgb]) -> Vec<Circle> {
    let mut s = seed.wrapping_mul(0x9E37_79B9) | 1;
    let mut next = |n: i32| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        ((s >> 8) % n.max(1) as u32) as i32
    };
    (0..count)
        .map(|_| {
            let rad = r.0 + next(r.1 - r.0 + 1);
            let (cx, cy) = (next(WIDTH + rad) - rad / 2, next(HEIGHT + rad) - rad / 2);
            let color = colors[next(colors.len() as i32) as usize];
            let a = alpha.0 + next(alpha.1 - alpha.0 + 1);
            (cx, cy, rad, color, a)
        })
        .collect()
}

/// The base of a wallpaper, under its layers, as [`Px`] colors.
fn render_base(base: &Base, f: &Fit) -> Vec<u32> {
    let mut buf = vec![0u32; (f.w * f.h) as usize];
    match base {
        Base::Grad(g) => {
            for y in 0..f.h {
                for x in 0..f.w {
                    buf[(y * f.w + x) as usize] = pack(g.at(f, x * 16 + 8, y * 16 + 8));
                }
            }
        }
        Base::Poly { grad, cell, jitter, seed, amp } => poly(&mut buf, f, grad, f.grain(*cell), f.grain(*jitter), *seed, *amp),
        Base::Flow { flow, stops } => {
            for y in 0..f.h {
                for x in 0..f.w {
                    buf[(y * f.w + x) as usize] = pack(stops_at(stops, flow.at(f, x, y)));
                }
            }
        }
        Base::Original if f.is_native() => {
            for (o, p) in buf.iter_mut().zip(original()) {
                *o = pack(px(p));
            }
        }
        // Elsewhere, its gradient (its facets are layers: `render`).
        Base::Original => return render_base(&ORIGINAL.base, f),
    }
    buf
}

/// A [`Px`] color (packed) as the frame buffer's eight bits a channel.
/// With `dither`, noise that spans one step of a six-bit panel (four of
/// the frame buffer's, sixteen of ours) is added first, from a
/// low-discrepancy sequence so that it has no visible grain.
#[inline]
fn to_frame(p: u32, x: i32, y: i32, dither: bool) -> u32 {
    let n = if dither { (((x * 49471 + y * 37345) & 0xFFFF) >> 12) - 8 } else { 0 };
    let c = unpack(p).map(|v| ((v + n + 2) >> 2).clamp(0, 255) as u32);
    (c[0] << 16) | (c[1] << 8) | c[2]
}

fn render(spec: &Spec, f: &Fit) -> Vec<u32> {
    // The first wallpaper is kept pixel for pixel on the color skin's
    // screen; elsewhere it is drawn from its description.
    if matches!(spec.base, Base::Original) && !f.is_native() {
        return render(&ORIGINAL, f);
    }
    let mut buf = render_base(&spec.base, f);
    let (w, h) = (f.w, f.h);
    for layer in spec.layers {
        match layer {
            Layer::Bands { num, den, add, color, bands } => draw_bands(&mut buf, f, *num, *den, *add, px(*color), bands),
            Layer::Glow { cx, cy, r, color, alpha } => glow(&mut buf, f, f.x(*cx), f.y(*cy), f.size(*r), px(*color), *alpha),
            Layer::Circles { soft, rim, circles } => {
                for &circle in *circles {
                    let (cx, cy, r) = f.circle(circle);
                    disc(&mut buf, f, cx, cy, r, soft_edge(r, *soft), px(circle.3), circle.4, *rim);
                }
            }
            Layer::Bubbles { seed, count, r, alpha, soft, rim, colors } => {
                for circle in bubbles(*seed, *count, *r, *alpha, colors) {
                    let (cx, cy, r) = f.circle(circle);
                    disc(&mut buf, f, cx, cy, r, soft_edge(r, *soft), px(circle.3), circle.4, *rim);
                }
            }
            Layer::Waves(waves) => {
                for wv in *waves {
                    wave(&mut buf, f, wv);
                }
            }
            Layer::Contours { seed, scale, levels, color, alpha, major, major_alpha } => {
                contours(&mut buf, f, *seed, f.grain(*scale), *levels, px(*color), *alpha, *major, *major_alpha)
            }
            Layer::Dots { px: step_x, py: step_y, r, stagger, color, alpha } => {
                let (step_x, step_y, r) = (f.grain(*step_x), f.grain(*step_y), f.grain(*r));
                let mut row = 0;
                while row * step_y < h + step_y {
                    let shift = if *stagger && row % 2 == 1 { step_x / 2 } else { 0 };
                    let mut col = -1;
                    while col * step_x < w + step_x {
                        let (cx, cy) = (col * step_x + step_x / 2 + shift, row * step_y + step_y / 2);
                        disc(&mut buf, f, cx * 16, cy * 16, r, 20, px(*color), *alpha, 0);
                        col += 1;
                    }
                    row += 1;
                }
            }
            Layer::Stripes { vertical, repeat, stripes } => {
                let repeat = f.grain(*repeat);
                for y in 0..h {
                    for x in 0..w {
                        let v = if *vertical { x } else { y } % repeat;
                        for &(off, width, color, alpha) in *stripes {
                            if (f.grain(off)..f.grain(off + width)).contains(&v) {
                                over(&mut buf[(y * w + x) as usize], px(color), alpha * 16);
                            }
                        }
                    }
                }
            }
            Layer::Weave { kind, color, alpha, color2, alpha2 } => {
                let kind = kind.grained(f);
                for y in 0..h {
                    for x in 0..w {
                        // Four samples a pixel smooth the slanted edges.
                        let mut n = [0, 0, 0];
                        for (dx, dy) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
                            n[weave(&kind, x * 4 + dx, y * 4 + dy)] += 1;
                        }
                        let p = &mut buf[(y * w + x) as usize];
                        over(p, px(*color), alpha * 4 * n[1]);
                        over(p, px(*color2), alpha2 * 4 * n[2]);
                    }
                }
            }
        }
    }
    // To the frame buffer's eight bits.
    for y in 0..h {
        for x in 0..w {
            let p = &mut buf[(y * w + x) as usize];
            *p = to_frame(*p, x, y, spec.dither);
        }
    }
    buf
}

/// The width of the edge of a circle of radius `r`, both in sixteenths of
/// a pixel, when the edge is `percent` of the radius.
fn soft_edge(r: i32, percent: i32) -> i32 {
    (r * percent / 100).max(20)
}

/// A circle at (`cx`, `cy`) with radius `r`, all in sixteenths of a pixel;
/// its opacity falls from `alpha` to nothing across `soft`, centered on
/// the radius.
/// The opacity (of [`ONE`]) of a circle at distance `d` from its center
/// (see [`disc`]).
#[inline]
fn disc_opacity(d: i32, r: i32, soft: i32, alpha: i32, rim: i32) -> i32 {
    let mut cov = ((r + soft / 2 - d) * ONE / soft).clamp(0, ONE);
    if soft > 32 {
        // A smooth step across a wide edge.
        cov = (((cov * cov) >> 12) * (3 * ONE - 2 * cov)) >> 12;
    }
    let mut t = (cov * alpha) >> 8;
    if rim > 0 {
        let ring = (20 - (d - (r - 16)).abs()).clamp(0, 20) * ONE / 20;
        t += (ring * rim) >> 8;
    }
    t
}

fn disc(buf: &mut [u32], f: &Fit, cx: i32, cy: i32, r: i32, soft: i32, color: Px, alpha: i32, rim: i32) {
    let reach = r + soft / 2 + 16;
    let (x0, x1) = (((cx - reach) >> 4).max(0), ((cx + reach) >> 4).min(f.w - 1));
    let (y0, y1) = (((cy - reach) >> 4).max(0), ((cy + reach) >> 4).min(f.h - 1));
    for y in y0..=y1 {
        let dy = y * 16 + 8 - cy;
        for x in x0..=x1 {
            let dx = x * 16 + 8 - cx;
            let d = ((dx * dx + dy * dy) as u32).isqrt() as i32;
            over(&mut buf[(y * f.w + x) as usize], color, disc_opacity(d, r, soft, alpha, rim));
        }
    }
}

fn glow(buf: &mut [u32], f: &Fit, cx: i32, cy: i32, r: i32, color: Px, alpha: i32) {
    let r2 = ((r * 16) * (r * 16)) >> 8;
    let (x0, x1) = ((cx - r).max(0), (cx + r).min(f.w - 1));
    let (y0, y1) = ((cy - r).max(0), (cy + r).min(f.h - 1));
    for y in y0..=y1 {
        let dy = y * 16 + 8 - cy * 16;
        for x in x0..=x1 {
            let dx = x * 16 + 8 - cx * 16;
            let d2 = (dx * dx + dy * dy) >> 8;
            if d2 < r2 {
                // (1 - (d / r)^2)^2: level at the center, and at the edge.
                let q = ONE - (i64::from(d2) * i64::from(ONE) / i64::from(r2)) as i32;
                over(&mut buf[(y * f.w + x) as usize], color, (((q * q) >> 12) * alpha) >> 8);
            }
        }
    }
}

fn draw_bands(buf: &mut [u32], f: &Fit, num: i32, den: i32, additive: bool, color: Px, bands: &[Band]) {
    // Where each band is, and how wide, in sixteenths of a pixel.
    let placed: Vec<(i32, i32, i32, i32)> = bands
        .iter()
        .map(|&(start, width, a0, a1)| {
            let (s, w) = f.band(num, den, start, width);
            (s, w, a0, a1)
        })
        .collect();
    for y in 0..f.h {
        let shift = (y * 16 + 8) * num / den;
        for x in 0..f.w {
            let u = x * 16 + 8 + shift;
            for &(s, w, a0, a1) in &placed {
                if u <= s - 8 || u >= s + w + 8 {
                    continue;
                }
                // How much of the pixel is inside, in sixteenths.
                let cov = (u - s + 8).min(s + w + 8 - u).min(16);
                let a = a0 + (a1 - a0) * (u - s).clamp(0, w) / w;
                let p = &mut buf[(y * f.w + x) as usize];
                if additive {
                    add(p, color, a * cov);
                } else {
                    over(p, color, a * cov);
                }
            }
        }
    }
}

/// The crest of wave `w` in column `x`, in sixteenths of a pixel from the
/// top of the picture, where the wave rests.
#[inline]
fn crest(w: &Wave, f: &Fit, x: i32) -> i32 {
    let mut edge = f.y(w.y * 16) + ((f.size(w.amp * 16) * sin(x * 65536 / f.size(w.period) + w.phase * 256)) >> 12);
    if w.period2 > 0 {
        edge += (f.size(w.amp2 * 16) * sin(x * 65536 / f.size(w.period2) + w.phase * 613)) >> 12;
    }
    edge
}

fn wave(buf: &mut [u32], f: &Fit, w: &Wave) {
    let color = px(w.color);
    let fade = f.size(w.fade * 16);
    for x in 0..f.w {
        let edge = crest(w, f, x);
        for y in ((edge - 8) >> 4).max(0)..f.h {
            let depth = y * 16 + 8 - edge;
            let cov = (depth + 8).clamp(0, 16);
            let mut a = w.alpha;
            if fade > 0 {
                a = a * (fade - depth).max(0) / fade;
                if a == 0 && depth > 0 {
                    break;
                }
            }
            over(&mut buf[(y * f.w + x) as usize], color, a * cov);
        }
    }
}

/// Smooth random terrain, 0..65536, at a point in sixteenths of a pixel.
fn terrain(seed: u32, sx: i32, sy: i32, scale: i32) -> i32 {
    let octave = |seed: u32, cell: i32| {
        let (xi, yi) = (sx.div_euclid(cell), sy.div_euclid(cell));
        let ease = |f: i32| {
            let t = f * ONE / cell;
            (((t * t) >> 12) * (3 * ONE - 2 * t)) >> 12
        };
        let (tx, ty) = (ease(sx.rem_euclid(cell)), ease(sy.rem_euclid(cell)));
        let v = |i: i32, j: i32| (hash(xi + i, yi + j, seed) & 0xFFFF) as i32;
        let top = v(0, 0) + (((v(1, 0) - v(0, 0)) * (tx >> 1)) >> 11);
        let bottom = v(0, 1) + (((v(1, 1) - v(0, 1)) * (tx >> 1)) >> 11);
        top + (((bottom - top) * (ty >> 1)) >> 11)
    };
    (octave(seed, scale * 16) * 3 + octave(seed ^ 0x5bd1, scale * 7)) / 4
}

fn contours(buf: &mut [u32], f: &Fit, seed: u32, scale: i32, levels: i32, color: Px, alpha: i32, major: i32, major_alpha: i32) {
    // The terrain's height in levels (65536 to a level) at the centers of
    // this row's pixels and the next row's, one more than the width.
    let row = |y: i32| -> Vec<i32> { (0..=f.w).map(|x| terrain(seed, x * 16 + 8, y * 16 + 8, scale) * levels).collect() };
    // The lines are as wide as the pattern's pixels.
    let grain = f.grain(1);
    let mut here = row(0);
    for y in 0..f.h {
        let below = row(y + 1);
        for x in 0..f.w as usize {
            let v = here[x];
            // How far the nearest level is, in height and then (by the
            // slope) in sixteenths of a pixel of the pattern.
            let frac = v & 0xFFFF;
            let off = frac.min(65536 - frac);
            let (gx, gy) = (here[x + 1] - v, below[x] - v);
            let slope = (((gx >> 2) * (gx >> 2) + (gy >> 2) * (gy >> 2)) as u32).isqrt() as i32 * 4;
            let dist = off * 16 / (slope.max(1) * grain);
            let cov = (16 - dist).clamp(0, 16);
            if cov > 0 {
                let line = (v + 32768) >> 16;
                let a = if major > 0 && line % major == 0 { major_alpha } else { alpha };
                over(&mut buf[y as usize * f.w as usize + x], color, a * cov);
            }
        }
        here = below;
    }
}

impl Weave {
    /// The weave with each of its sizes in pixels of the picture
    /// ([`Fit::grain`]).
    fn grained(&self, f: &Fit) -> Weave {
        match *self {
            Weave::Houndstooth { unit } => Weave::Houndstooth { unit: f.grain(unit) },
            Weave::Herringbone { col, period } => Weave::Herringbone { col: f.grain(col), period: f.grain(period) },
            Weave::Chevron { px, py } => Weave::Chevron { px: f.grain(px), py: f.grain(py) },
            Weave::Argyle { w, h, line } => Weave::Argyle { w: f.grain(w), h: f.grain(h), line: f.grain(line) },
        }
    }
}

/// Which color of a weave a point (in quarter pixels) has: 0 none, 1 the
/// first, 2 the second.
fn weave(kind: &Weave, x: i32, y: i32) -> usize {
    match *kind {
        Weave::Houndstooth { unit } => {
            let (i, j) = (x / (4 * unit), y / (4 * unit));
            let over_warp = (i + j + 1) & 3 < 2;
            let dark = if over_warp { i & 7 < 4 } else { j & 7 < 4 };
            dark as usize
        }
        Weave::Herringbone { col, period } => {
            let (c, l) = (x / (4 * col), x % (4 * col));
            let s = if c & 1 == 0 { l } else { 4 * col - l };
            ((y + s) % (4 * period) < 2 * period) as usize
        }
        Weave::Chevron { px, py } => {
            let t = (x % (4 * px) - 2 * px).abs();
            ((y + t) % (4 * py) < 2 * py) as usize
        }
        Weave::Argyle { w, h, line } => {
            let (w, h) = (4 * w, 4 * h);
            let (u, v) = (x * h + y * w, x * h - y * w);
            let m = w * h;
            let half = line * ((w * w + h * h) as u32).isqrt() as i32 / 8;
            if (u.rem_euclid(m) - m / 2).abs() < half || (v.rem_euclid(m) - m / 2).abs() < half {
                2
            } else {
                ((u.div_euclid(m) + v.div_euclid(m)) & 1 == 0) as usize
            }
        }
    }
}

/// The triangle field ([`Base::Poly`]).
fn poly(buf: &mut [u32], f: &Fit, grad: &Grad, cell: i32, jitter: i32, seed: u32, amp: i32) {
    // Everything here is in quarter pixels.  Grid point (i, j) is moved
    // from its place by up to `jitter` pixels each way.
    let vertex = |i: i32, j: i32| {
        let h = hash(i, j, seed);
        let span = 8 * jitter + 1;
        (i * cell * 4 + (h % span as u32) as i32 - 4 * jitter, j * cell * 4 + ((h >> 12) % span as u32) as i32 - 4 * jitter)
    };
    // The two triangles of grid square (i, j): `k` picks one.
    let triangle = |i: i32, j: i32, k: i32| {
        let (a, b, c, d) = (vertex(i, j), vertex(i + 1, j), vertex(i, j + 1), vertex(i + 1, j + 1));
        match (hash(i, j, seed ^ 0x7f4a) & 1, k) {
            (0, 0) => [a, b, d],
            (0, _) => [a, d, c],
            (_, 0) => [a, b, c],
            (_, _) => [b, d, c],
        }
    };
    let inside = |t: &[(i32, i32); 3], x: i32, y: i32| {
        let edge = |p: (i32, i32), q: (i32, i32)| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
        let (e0, e1, e2) = (edge(t[0], t[1]), edge(t[1], t[2]), edge(t[2], t[0]));
        (e0 >= 0 && e1 >= 0 && e2 >= 0) || (e0 <= 0 && e1 <= 0 && e2 <= 0)
    };
    let color = |i: i32, j: i32, k: i32, t: &[(i32, i32); 3]| {
        let (cx, cy) = ((t[0].0 + t[1].0 + t[2].0) * 4 / 3, (t[0].1 + t[1].1 + t[2].1) * 4 / 3);
        let shade = ((hash(i * 2 + k, j, seed ^ 0x1b3) % (2 * amp as u32 + 1)) as i32 - amp) * 4;
        grad.at(f, cx, cy).map(|v| (v + shade).clamp(0, 1020))
    };
    // The triangle the last sample was in: most samples are in it too.
    let mut last: Option<((i32, i32, i32), [(i32, i32); 3], Px)> = None;
    let mut sample = |x: i32, y: i32| -> ((i32, i32, i32), Px) {
        if let Some((id, t, c)) = &last
            && inside(t, x, y)
        {
            return (*id, *c);
        }
        let step = cell * 4;
        for j in (y - 4 * jitter).div_euclid(step)..=(y + 4 * jitter).div_euclid(step) {
            for i in (x - 4 * jitter).div_euclid(step)..=(x + 4 * jitter).div_euclid(step) {
                for k in 0..2 {
                    let t = triangle(i, j, k);
                    if inside(&t, x, y) {
                        let c = color(i, j, k, &t);
                        last = Some(((i, j, k), t, c));
                        return ((i, j, k), c);
                    }
                }
            }
        }
        ((i32::MIN, 0, 0), grad.at(f, x * 4, y * 4))
    };
    for y in 0..f.h {
        for x in 0..f.w {
            // Four samples a pixel smooth the edges between triangles.
            let s = [(1, 1), (3, 1), (1, 3), (3, 3)].map(|(dx, dy)| sample(x * 4 + dx, y * 4 + dy));
            let c = if s.iter().all(|t| t.0 == s[0].0) {
                s[0].1
            } else {
                [0, 1, 2].map(|i| (s[0].1[i] + s[1].1[i] + s[2].1[i] + s[3].1[i] + 2) / 4)
            };
            buf[(y * f.w + x) as usize] = pack(c);
        }
    }
}

/// The first wallpaper: a deep blue vertical gradient crossed by broad
/// diagonal facets (our own design in the stock firmware's palette).  It
/// is kept as it was first drawn, in whole steps of the frame buffer.
fn original() -> impl Iterator<Item = Rgb> {
    // Vertical gradient stops: (row, r, g, b).
    const STOPS: [(i32, i32, i32, i32); 4] =
        [(0, 0x15, 0x1E, 0x2C), (56, 0x10, 0x24, 0x44), (150, 0x0D, 0x20, 0x3E), (268, 0x0A, 0x14, 0x26)];
    // Facets: (slant: +1 leans right going down, -1 left; start and
    // width along the slanted axis; strength in 1/64).  Each facet is
    // brightest at its leading edge and fades across its width.
    const FACETS: [(i32, i32, i32, i32); 6] =
        [(1, 60, 170, 30), (1, 330, 150, 22), (1, 560, 190, 32), (-1, -120, 160, 24), (-1, 130, 140, 18), (-1, 300, 170, 28)];
    (0..HEIGHT).flat_map(|y| {
        let (mut r, mut g, mut b) = (0x20, 0x21, 0x22);
        if y <= 268 {
            let i = STOPS.iter().rposition(|s| s.0 <= y).unwrap_or(0).min(STOPS.len() - 2);
            let (a, z) = (STOPS[i], STOPS[i + 1]);
            let (t, span) = (y - a.0, z.0 - a.0);
            r = a.1 + (z.1 - a.1) * t / span;
            g = a.2 + (z.2 - a.2) * t / span;
            b = a.3 + (z.3 - a.3) * t / span;
        }
        (0..WIDTH).map(move |x| {
            let (mut pr, mut pg, mut pb) = (r, g, b);
            if y <= 268 {
                let mut light = 0;
                for (slant, start, width, strength) in FACETS {
                    // Slope 3 across to 5 down.
                    let u = x + slant * y * 3 / 5 - start;
                    if (0..width).contains(&u) {
                        light += strength * (width - u) / width;
                    }
                }
                // Lighten toward a brighter blue.
                pr += 0x06 * light / 64;
                pg += 0x16 * light / 64;
                pb += 0x38 * light / 64;
            }
            crate::gfx::rgb(pr.clamp(0, 255) as u8, pg.clamp(0, 255) as u8, pb.clamp(0, 255) as u8)
        })
    })
}

// ------------------------------------------------------------- the motion

mod motion;
pub use motion::CYCLE_MS;

// ---------------------------------------------------------- the wallpapers

include!("wallpapers.rs");
