//! Integer-only drawing: fills, lines, glyph and icon blits into a back
//! buffer, and damage-tracked presentation to a HAL display.

use crate::font::FontData;
use crate::icons::{Icon, FMT_GRAY2, FMT_MASK4};
use opendeskphone_hal::{Display, PixelFormat, Screen};

/// A color as `0x00RRGGBB`.
pub type Rgb = u32;

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// The four gray levels as colors (0 = black .. 3 = white).
pub const GRAY: [Rgb; 4] = [0x000000, 0x555555, 0xAAAAAA, 0xFFFFFF];

/// A rectangle in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        IRect { x, y, w, h }
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        IRect { x, y, w: (r - x).max(0), h: (b - y).max(0) }
    }
    /// Shrinks by `d` on every side.
    pub fn inset(&self, d: i32) -> IRect {
        IRect { x: self.x + d, y: self.y + d, w: (self.w - 2 * d).max(0), h: (self.h - 2 * d).max(0) }
    }
    pub fn inset_xy(&self, dx: i32, dy: i32) -> IRect {
        IRect { x: self.x + dx, y: self.y + dy, w: (self.w - 2 * dx).max(0), h: (self.h - 2 * dy).max(0) }
    }
    /// Splits off the top `h` pixels: (top, rest).
    pub fn split_top(&self, h: i32) -> (IRect, IRect) {
        let h = h.clamp(0, self.h);
        (IRect { h, ..*self }, IRect { y: self.y + h, h: self.h - h, ..*self })
    }
    /// Splits off the bottom `h` pixels: (rest, bottom).
    pub fn split_bottom(&self, h: i32) -> (IRect, IRect) {
        let h = h.clamp(0, self.h);
        (IRect { h: self.h - h, ..*self }, IRect { y: self.bottom() - h, h, ..*self })
    }
    /// Splits off the left `w` pixels: (left, rest).
    pub fn split_left(&self, w: i32) -> (IRect, IRect) {
        let w = w.clamp(0, self.w);
        (IRect { w, ..*self }, IRect { x: self.x + w, w: self.w - w, ..*self })
    }
    /// Splits off the right `w` pixels: (rest, right).
    pub fn split_right(&self, w: i32) -> (IRect, IRect) {
        let w = w.clamp(0, self.w);
        (IRect { w: self.w - w, ..*self }, IRect { x: self.right() - w, w, ..*self })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Writes pixels to a frame buffer of [`PixelFormat::Xrgb8888`]: four
/// bytes each, the blue first.
#[inline]
fn put_xrgb(dst: &mut [u8], src: &[u32]) {
    // Where the processor keeps a word's low byte first, the pixels are
    // the bytes as they are, and one copy of them is five times as fast
    // on the phone as a pixel at a time (measured on a D60: 1.5 ms for
    // 285 kB, where four single bytes for each pixel took 7.8 ms).
    #[cfg(target_endian = "little")]
    dst.copy_from_slice(bytemuck::cast_slice(src));
    #[cfg(not(target_endian = "little"))]
    for (o, p) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
        *o = p.to_le_bytes();
    }
}

/// What a screen draws over its wallpaper, apart from the wallpaper: for
/// each pixel, how much of the wallpaper shows there and the color that
/// is added to it.  With it, the picture over another frame of the
/// wallpaper is arithmetic on the pixels that frame changed, and the
/// screen need not be drawn again ([`Canvas::patch`]).
///
/// It is made from the screen drawn twice, over black and over white:
/// everything the screens draw either replaces a pixel or blends a color
/// into it, so a pixel of the result is the wallpaper's times a factor
/// plus a color, and the two drawings give both.  Where the screen left
/// the wallpaper alone, and where it hid it, the result is exact; where
/// something is blended (the edges of text and of shapes), it is within
/// one step of a channel of what drawing would give.
#[derive(Default)]
pub struct Overlay {
    /// The part of the wallpaper that shows (of 255) in the top byte; the
    /// color added below it.
    px: Vec<u32>,
}

impl Overlay {
    /// This is the first step, for which the canvas holds the screen drawn
    /// over black.  It takes the pixels within `r` and leaves the rest as
    /// they were.
    pub fn over_black(&mut self, c: &Canvas, r: IRect) {
        self.px.resize(c.pix.len(), 0);
        for row in c.rows(r) {
            self.px[row.clone()].copy_from_slice(&c.pix[row]);
        }
    }

    /// The second step: the canvas holds the same drawn over white.
    pub fn over_white(&mut self, c: &Canvas, r: IRect) {
        for row in c.rows(r) {
            for (o, white) in self.px[row.clone()].iter_mut().zip(&c.pix[row]) {
                let black = *o & 0xFF_FFFF;
                let shows = if black == 0 && *white == 0xFF_FFFF {
                    255
                } else {
                    // The difference is the same in every channel, but for
                    // the rounding.
                    let ch = |shift: u32| ((white >> shift) & 255).saturating_sub((black >> shift) & 255);
                    (ch(16) + ch(8) + ch(0) + 1) / 3
                };
                *o = (shows << 24) | black;
            }
        }
    }

    /// A pixel of the screen, whose pixel of the overlay is `o`, over a
    /// wallpaper whose pixel is `wall`.
    #[inline]
    fn over(o: u32, wall: Rgb) -> Rgb {
        match o >> 24 {
            255 => wall,
            0 => o,
            shows => {
                let ch = |shift: u32| {
                    let v = ((wall >> shift) & 255) * shows + 127;
                    (((o >> shift) & 255) + ((v + 1 + (v >> 8)) >> 8)).min(255) << shift
                };
                ch(16) | ch(8) | ch(0)
            }
        }
    }
}

/// A back buffer for one screen.
pub struct Canvas {
    w: i32,
    h: i32,
    gray: bool,
    pix: Vec<u32>,
    front: Vec<u32>,
    /// False until the first present: everything is damaged.
    presented: bool,
    /// The screen sets this clip, and a region is cut to it
    /// ([`Canvas::hit`]).
    clip: IRect,
    limit: IRect,
    /// Drawing changes pixels only within this rectangle, which is the clip
    /// within the limit.
    paint: IRect,
    /// Where the picture can be touched ([`Canvas::hit`]), in the order
    /// registered.
    hits: Vec<(IRect, crate::touch::Hit)>,
    /// Which of them show nothing under a finger ([`Canvas::hit_quiet`]):
    /// their places in `hits`.
    quiet: Vec<usize>,
    /// The shapes with round corners drawn since the regions were last
    /// forgotten, each with its radius: a region with the rectangle of
    /// one is of that shape ([`Canvas::show_press`]).
    rounds: Vec<(IRect, i32)>,
    /// Where a finger is down on the picture ([`Canvas::set_press`]).
    press: Option<(i32, i32)>,
    /// What the screen being drawn is told of its place among the
    /// screens ([`Canvas::set_nav`]).
    nav: NavCx,
}

/// What a screen is told, as it draws, of its place among the screens:
/// for the title bar ([`crate::widgets::title_bar`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavCx {
    /// There is a screen to go back to: the screen is not the first.
    pub back: bool,
}

fn quantize(c: Rgb) -> Rgb {
    let lum = (((c >> 16) & 255) * 77 + ((c >> 8) & 255) * 150 + (c & 255) * 29) >> 8;
    GRAY[((lum * 3 + 127) / 255) as usize]
}

#[inline]
fn blend(dst: Rgb, src: Rgb, a: u32, max: u32) -> Rgb {
    if a >= max {
        return src;
    }
    let na = max - a;
    // The division by `max`, without dividing where it is 255 (glyphs)
    // or a power of two (shapes): the phone's processor has no divide
    // instruction, and this is done for every blended pixel.  The same
    // quotients either way.
    let over = |v: u32| match max {
        255 => (v + 1 + (v >> 8)) >> 8,
        _ if max.is_power_of_two() => v >> max.trailing_zeros(),
        _ => v / max,
    };
    let ch = |shift: u32| {
        let d = (dst >> shift) & 255;
        let s = (src >> shift) & 255;
        over(d * na + s * a + max / 2) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// Iterates over the pixels of a picture that has four bits a pixel, as
/// (column, row, value).
fn nibbles(w: i32, h: i32, data: &[u8]) -> impl Iterator<Item = (i32, i32, u32)> + '_ {
    let pixels = (0..h).flat_map(move |row| (0..w).map(move |col| (col, row)));
    pixels.zip(0usize..).map(move |((col, row), n)| {
        let byte = data[n / 2];
        (col, row, u32::from(if n.is_multiple_of(2) { byte >> 4 } else { byte & 15 }))
    })
}

impl Canvas {
    /// A canvas for a `width` x `height` screen; `gray` reduces every pixel
    /// to four levels.
    pub fn new(width: u16, height: u16, gray: bool) -> Canvas {
        let n = width as usize * height as usize;
        Canvas {
            w: width as i32,
            h: height as i32,
            gray,
            pix: vec![0; n],
            front: vec![0; n],
            presented: false,
            clip: IRect::new(0, 0, width as i32, height as i32),
            limit: IRect::new(0, 0, width as i32, height as i32),
            paint: IRect::new(0, 0, width as i32, height as i32),
            hits: Vec::new(),
            quiet: Vec::new(),
            rounds: Vec::new(),
            press: None,
            nav: NavCx::default(),
        }
    }

    pub fn width(&self) -> i32 {
        self.w
    }
    pub fn height(&self) -> i32 {
        self.h
    }
    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.w, self.h)
    }
    pub fn is_gray(&self) -> bool {
        self.gray
    }
    /// The pixels as `0x00RRGGBB`, row-major.
    pub fn pixels(&self) -> &[u32] {
        &self.pix
    }
    pub fn pixel(&self, x: i32, y: i32) -> Rgb {
        self.pix[(y * self.w + x) as usize]
    }

    /// Restricts drawing to `r` (intersected with the screen); returns the
    /// previous clip for [`Canvas::restore_clip`].
    pub fn set_clip(&mut self, r: IRect) -> IRect {
        let old = self.clip;
        self.restore_clip(r.intersect(&self.bounds()));
        old
    }
    /// Narrows the clip to its intersection with `r`.
    pub fn push_clip(&mut self, r: IRect) -> IRect {
        let old = self.clip;
        self.restore_clip(r.intersect(&old));
        old
    }
    pub fn restore_clip(&mut self, old: IRect) {
        self.clip = old;
        self.paint = old.intersect(&self.limit);
    }
    pub fn clip(&self) -> IRect {
        self.clip
    }

    /// From now on drawing changes no pixel outside `r`, or with `None` it
    /// changes pixels anywhere again.  The clip a screen sees and the
    /// regions it registers do not change.
    pub fn set_limit(&mut self, r: Option<IRect>) {
        self.limit = r.map_or(self.bounds(), |r| r.intersect(&self.bounds()));
        self.restore_clip(self.clip);
    }

    /// Each item is the range of places in [`Canvas::pixels`] that a row of
    /// `r` covers within the canvas.
    fn rows(&self, r: IRect) -> impl Iterator<Item = std::ops::Range<usize>> + use<> {
        let r = r.intersect(&self.bounds());
        let w = self.w as usize;
        (r.y as usize..r.bottom() as usize).map(move |y| y * w + r.x as usize..y * w + r.right() as usize)
    }

    /// The pixels of `r` become what `over` puts over `frame`, a frame of
    /// the wallpaper in motion, and the screen is not drawn over the
    /// wallpaper again.
    pub fn compose(&mut self, r: IRect, frame: &[u32], over: &Overlay) {
        if frame.len() != self.pix.len() || over.px.len() != self.pix.len() {
            return;
        }
        for row in self.rows(r) {
            for i in row {
                self.pix[i] = Overlay::over(over.px[i], frame[i]);
            }
        }
    }

    fn color(&self, c: Rgb) -> Rgb {
        if self.gray { quantize(c) } else { c & 0xFF_FFFF }
    }

    pub fn fill(&mut self, r: IRect, c: Rgb) {
        let r = r.intersect(&self.paint);
        if r.is_empty() {
            return;
        }
        let c = self.color(c);
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            self.pix[row + r.x as usize..row + r.right() as usize].fill(c);
        }
    }

    pub fn clear(&mut self, c: Rgb) {
        self.fill(self.bounds(), c);
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, c: Rgb) {
        self.fill(IRect::new(x, y, w, 1), c);
    }
    pub fn vline(&mut self, x: i32, y: i32, h: i32, c: Rgb) {
        self.fill(IRect::new(x, y, 1, h), c);
    }

    /// A one-pixel outline.
    pub fn frame(&mut self, r: IRect, c: Rgb) {
        if r.is_empty() {
            return;
        }
        self.hline(r.x, r.y, r.w, c);
        self.hline(r.x, r.bottom() - 1, r.w, c);
        self.vline(r.x, r.y, r.h, c);
        self.vline(r.right() - 1, r.y, r.h, c);
    }

    /// A filled rectangle with rounded corners of radius `rad`.
    pub fn fill_round(&mut self, r: IRect, rad: i32, c: Rgb) {
        let rad = rad.min(r.w / 2).min(r.h / 2).max(0);
        if rad == 0 {
            return self.fill(r, c);
        }
        self.rounds.push((r, rad));
        self.fill(IRect::new(r.x, r.y + rad, r.w, r.h - 2 * rad), c);
        for i in 0..rad {
            // Row i from the top/bottom edge: inset by the circle.
            let dy = rad - i;
            let mut dx = 0;
            while (dx + 1) * (dx + 1) + dy * dy <= rad * rad + rad / 2 {
                dx += 1;
            }
            let inset = rad - dx;
            self.hline(r.x + inset, r.y + i, r.w - 2 * inset, c);
            self.hline(r.x + inset, r.bottom() - 1 - i, r.w - 2 * inset, c);
        }
    }

    /// A filled rectangle with rounded corners of radius `rad`, its edge
    /// anti-aliased by pixel coverage.  (A gray canvas shows the coverage
    /// through its two middle levels.)  A radius of half the height makes
    /// a bar with round ends.
    pub fn fill_round_aa(&mut self, r: IRect, rad: i32, c: Rgb) {
        /// One pixel in the fixed-point unit of the distances below.
        const ONE: i32 = 256;
        if r.is_empty() {
            return;
        }
        let rad = rad.min(r.w / 2).min(r.h / 2).max(0);
        if rad > 0 {
            self.rounds.push((r, rad));
        }
        let c = self.color(c);
        // The straight middle needs no blending.
        self.fill(IRect::new(r.x + rad, r.y, r.w - 2 * rad, r.h), c);
        // Half the size, and half the size less the radius: the distance
        // from the center to the centers of the corner circles.
        let (hw, hh) = (r.w * ONE / 2, r.h * ONE / 2);
        let (iw, ih) = (hw - rad * ONE, hh - rad * ONE);
        for j in 0..r.h {
            let qy = (j * ONE + ONE / 2 - hh).abs() - ih;
            for i in (0..rad).chain(r.w - rad..r.w) {
                let qx = (i * ONE + ONE / 2 - hw).abs() - iw;
                // The signed distance from the pixel's center to the edge.
                let d = if qx > 0 && qy > 0 {
                    ((qx * qx + qy * qy) as u32).isqrt() as i32 - rad * ONE
                } else {
                    qx.max(qy) - rad * ONE
                };
                let coverage = (ONE / 2 - d).clamp(0, ONE) as u32;
                self.put_blend(r.x + i, r.y + j, c, coverage, ONE as u32);
            }
        }
    }

    #[inline]
    fn put_blend(&mut self, x: i32, y: i32, c: Rgb, a: u32, max: u32) {
        if a == 0 || x < self.paint.x || y < self.paint.y || x >= self.paint.right() || y >= self.paint.bottom() {
            return;
        }
        let i = (y * self.w + x) as usize;
        let v = blend(self.pix[i], c, a, max);
        self.pix[i] = if self.gray { quantize(v) } else { v };
    }

    /// Draws `s` with its baseline at `y`, starting at `x`.  Returns the
    /// advance.
    pub fn text(&mut self, font: &FontData, x: i32, y: i32, s: &str, c: Rgb) -> i32 {
        let c = self.color(c);
        let mut pen = x;
        for ch in s.chars() {
            let Some((font, g)) = font.glyph_or_fallback(ch) else { continue };
            let gx = pen + g.xoff as i32;
            let gy = y - g.top as i32;
            pen += g.adv as i32;
            if g.w == 0
                || gx >= self.paint.right()
                || gx + g.w as i32 <= self.paint.x
                || gy >= self.paint.bottom()
                || gy + g.h as i32 <= self.paint.y
            {
                continue;
            }
            let data = &font.data[g.off as usize..];
            if font.bits == 1 {
                // A bitmap font: its pixels as they are, in one solid
                // level.
                let row_bytes = (g.w as usize).div_ceil(8);
                for row in 0..g.h as usize {
                    let bits = &data[row * row_bytes..];
                    for col in 0..g.w as usize {
                        if bits[col / 8] & (0x80 >> (col % 8)) != 0 {
                            self.put_blend(gx + col as i32, gy + row as i32, c, 1, 1);
                        }
                    }
                }
                continue;
            }
            let mut n = 0usize;
            for row in 0..g.h as i32 {
                for col in 0..g.w as i32 {
                    let byte = data[n / 2];
                    let a = if n.is_multiple_of(2) { byte >> 4 } else { byte & 15 };
                    n += 1;
                    self.put_blend(gx + col, gy + row, c, a as u32, 15);
                }
            }
        }
        pen - x
    }

    /// Draws one line of text inside `r`, vertically centered, aligned
    /// horizontally, truncated with an ellipsis when too long.
    pub fn text_in(&mut self, font: &FontData, r: IRect, s: &str, c: Rgb, align: HAlign) {
        if r.is_empty() {
            return;
        }
        let mut buf;
        let mut s = s;
        let mut w = font.width(s);
        if w > r.w {
            let ell = font.width("\u{2026}");
            let n = font.fit(s, r.w - ell);
            buf = String::with_capacity(n + 3);
            buf.push_str(s[..n].trim_end());
            buf.push('\u{2026}');
            w = font.width(&buf);
            s = &buf;
        }
        let x = match align {
            HAlign::Left => r.x,
            HAlign::Center => r.x + (r.w - w) / 2,
            HAlign::Right => r.right() - w,
        };
        let y = r.y + (r.h - font.height()) / 2 + font.ascent as i32;
        let old = self.push_clip(r);
        self.text(font, x, y, s, c);
        self.restore_clip(old);
    }

    /// Word-wrapped text from the top of `r`; returns the height used.
    pub fn text_wrapped(&mut self, font: &FontData, r: IRect, s: &str, c: Rgb, align: HAlign, line_h: i32) -> i32 {
        let lines = font.wrap(s, r.w);
        let mut y = r.y;
        for line in &lines {
            if y + line_h > r.bottom() + line_h / 2 {
                break;
            }
            self.text_in(font, IRect::new(r.x, y, r.w, line_h), line, c, align);
            y += line_h;
        }
        y - r.y
    }

    /// Draws an icon with its top-left at (`x`, `y`).  `tint` colors
    /// alpha-mask icons; it is ignored by color and gray-level icons.
    pub fn icon(&mut self, icon: Icon, x: i32, y: i32, tint: Rgb) {
        let (w, h) = (icon.width(), icon.height());
        let data = icon.bytes();
        match icon.rec.fmt {
            FMT_MASK4 => self.mask4(x, y, w, h, data, tint),
            FMT_GRAY2 => {
                for (col, row, v) in nibbles(w, h, data) {
                    if v < 4 {
                        self.put_blend(x + col, y + row, GRAY[v as usize], 1, 1);
                    }
                }
            }
            _ => {
                let (mut col, mut row) = (0, 0);
                for run in data.as_chunks::<5>().0 {
                    let c = rgb(run[1], run[2], run[3]);
                    let a = run[4] as u32;
                    for _ in 0..run[0] {
                        if a != 0 {
                            self.put_blend(x + col, y + row, c, a, 255);
                        }
                        col += 1;
                        if col == w {
                            col = 0;
                            row += 1;
                        }
                    }
                }
            }
        }
    }

    /// Draws an icon centered in `r`.
    pub fn icon_in(&mut self, icon: Icon, r: IRect, tint: Rgb) {
        self.icon(icon, r.x + (r.w - icon.width()) / 2, r.y + (r.h - icon.height()) / 2, tint);
    }

    /// Draws a mask of `w` by `h` pixels in `tint`, with its top-left at
    /// (`x`, `y`).  `alpha` has the format of `opendeskphone_state::Mask`,
    /// and nothing is drawn if it is too short for the size.
    pub fn mask4(&mut self, x: i32, y: i32, w: i32, h: i32, alpha: &[u8], tint: Rgb) {
        if w <= 0 || h <= 0 || alpha.len() < (w as usize * h as usize).div_ceil(2) {
            return;
        }
        let tint = self.color(tint);
        for (col, row, v) in nibbles(w, h, alpha) {
            self.put_blend(x + col, y + row, tint, v, 15);
        }
    }

    /// Draws raw RGBA pixels (app images) with their top-left at (`x`, `y`).
    pub fn rgba(&mut self, x: i32, y: i32, w: i32, h: i32, pixels: &[u8]) {
        if pixels.len() < (w * h * 4) as usize {
            return;
        }
        for row in 0..h {
            for col in 0..w {
                let p = &pixels[((row * w + col) * 4) as usize..];
                self.put_blend(x + col, y + row, rgb(p[0], p[1], p[2]), p[3] as u32, 255);
            }
        }
    }

    /// Draws a one-bit picture (an app canvas): `bits` holds rows from the
    /// top, each padded to whole bytes, the leftmost pixel in the highest
    /// bit.  Each set bit becomes a `scale` x `scale` square of color `c`.
    pub fn bitmap(&mut self, x: i32, y: i32, w: i32, h: i32, scale: i32, bits: &[u8], c: Rgb) {
        let stride = (w.max(0) as usize).div_ceil(8);
        if w <= 0 || h <= 0 || scale <= 0 || bits.len() < stride * h as usize {
            return;
        }
        // Rows and columns outside the clip are not visited.
        let clip = self.paint.intersect(&IRect::new(x, y, w * scale, h * scale));
        if clip.is_empty() {
            return;
        }
        let (row0, row1) = ((clip.y - y) / scale, (clip.bottom() - y - 1) / scale);
        let (col0, col1) = ((clip.x - x) / scale, (clip.right() - x - 1) / scale);
        for row in row0..=row1 {
            let line = &bits[row as usize * stride..][..stride];
            let mut col = col0;
            while col <= col1 {
                if line[col as usize / 8] & (0x80 >> (col % 8)) == 0 {
                    col += 1;
                    continue;
                }
                // A run of set bits is one rectangle.
                let start = col;
                while col <= col1 && line[col as usize / 8] & (0x80 >> (col % 8)) != 0 {
                    col += 1;
                }
                self.fill(IRect::new(x + start * scale, y + row * scale, (col - start) * scale, scale), c);
            }
        }
    }

    /// The bounding box of the pixels that differ from what was last
    /// presented.
    pub fn damage(&self) -> Option<IRect> {
        if !self.presented {
            return Some(self.bounds());
        }
        let w = self.w as usize;
        // Nothing outside the limit was drawn.
        let lim = self.limit;
        let (x0, x1) = (lim.x as usize, lim.right() as usize);
        let rows = lim.y as usize..lim.bottom() as usize;
        // Pixel by pixel (a comparison of the slices is one of their
        // bytes, one at a time, in the phone's C library).
        let differs = |y: usize| self.pix[y * w + x0..y * w + x1].iter().zip(&self.front[y * w + x0..y * w + x1]).any(|(a, b)| a != b);
        let top = rows.clone().find(|&y| differs(y))?;
        let bottom = (top..rows.end).rev().find(|&y| differs(y)).unwrap_or(top);
        let (mut left, mut right) = (x1, x0);
        for y in top..=bottom {
            let (a, b) = (&self.pix[y * w..(y + 1) * w], &self.front[y * w..(y + 1) * w]);
            if let Some(l) = (x0..left).find(|&x| a[x] != b[x]) {
                left = l;
            }
            if let Some(r) = (right..x1).rev().find(|&x| a[x] != b[x]) {
                right = r + 1;
            }
        }
        Some(IRect::new(left as i32, top as i32, (right.max(left + 1) - left) as i32, (bottom - top + 1) as i32))
    }

    /// Copies the damaged area to the display and flushes it.  Returns the
    /// damage, or `None` when nothing changed (no flush happens).
    pub fn present(&mut self, display: &mut dyn Display, screen: Screen) -> opendeskphone_hal::Result<Option<IRect>> {
        let Some(all) = self.damage() else { return Ok(None) };
        let Some(info) = display.info(screen) else { return Ok(None) };
        let d = all.intersect(&IRect::new(0, 0, info.width as i32, info.height as i32));
        if d.is_empty() {
            return Ok(None);
        }
        {
            let frame = display.frame(screen)?;
            let w = self.w as usize;
            for y in d.y as usize..d.bottom() as usize {
                let src = &self.pix[y * w + d.x as usize..y * w + d.right() as usize];
                let row = y * info.stride;
                match info.format {
                    PixelFormat::Xrgb8888 => put_xrgb(&mut frame[row + d.x as usize * 4..row + d.right() as usize * 4], src),
                    PixelFormat::R8 => {
                        let dst = &mut frame[row + d.x as usize..row + d.right() as usize];
                        for (o, p) in dst.iter_mut().zip(src) {
                            // Gray canvases hold r == g == b.
                            *o = (*p & 255) as u8;
                        }
                    }
                }
            }
        }
        display.flush(
            screen,
            &[opendeskphone_hal::Rect::new(d.x as u16, d.y as u16, d.w as u16, d.h as u16)],
        )?;
        if self.presented {
            // Nothing outside the damage differs.
            let w = self.w as usize;
            for y in all.y as usize..all.bottom() as usize {
                let row = y * w + all.x as usize..y * w + all.right() as usize;
                self.front[row.clone()].copy_from_slice(&self.pix[row]);
            }
        } else {
            self.front.copy_from_slice(&self.pix);
        }
        self.presented = true;
        Ok(Some(d))
    }

    /// Brings the screen from one frame of a moving wallpaper to the
    /// next without drawing the screen again: `frame` is the wallpaper's
    /// new frame, `runs` the pixels of it that may differ from the frame
    /// before, and `over` what the screen draws over the wallpaper.  The
    /// pixels that changed go to the canvas and straight to the display.
    ///
    /// Returns false, with nothing done, where this cannot be (a gray
    /// canvas, nothing presented yet, a display of another size or
    /// format): the screen is then to be drawn and presented whole.
    pub fn patch(
        &mut self,
        frame: &[u32],
        runs: &[crate::wallpaper::Run],
        over: &Overlay,
        display: &mut dyn Display,
        screen: Screen,
    ) -> opendeskphone_hal::Result<bool> {
        let n = self.pix.len();
        if self.gray || !self.presented || frame.len() != n || over.px.len() != n {
            return Ok(false);
        }
        let Some(info) = display.info(screen) else { return Ok(false) };
        if info.format != PixelFormat::Xrgb8888 || info.width as i32 != self.w || info.height as i32 != self.h {
            return Ok(false);
        }
        let w = self.w as usize;
        // The rows and columns with a pixel that changed.
        let (mut top, mut bottom, mut left, mut right) = (usize::MAX, 0, usize::MAX, 0);
        {
            let fb = display.frame(screen)?;
            for run in runs {
                let (y, x0, x1) = (usize::from(run.y), usize::from(run.x0), usize::from(run.x1).min(w));
                if y >= self.h as usize || x0 >= x1 {
                    continue;
                }
                let row = y * w;
                if x1 - x0 == 1 {
                    // One pixel (many runs are): without the copies' calls.
                    let i = row + x0;
                    let p = Overlay::over(over.px[i], frame[i]);
                    if p != self.pix[i] {
                        (self.pix[i], self.front[i]) = (p, p);
                        let at = y * info.stride + x0 * 4;
                        fb[at..at + 4].copy_from_slice(&p.to_le_bytes());
                        (top, bottom) = (top.min(y), bottom.max(y));
                        (left, right) = (left.min(x0), right.max(x0));
                    }
                    continue;
                }
                let (mut first, mut last) = (usize::MAX, 0);
                let span = row + x0..row + x1;
                let pixels = self.pix[span.clone()].iter_mut().zip(&frame[span.clone()]).zip(&over.px[span]);
                for (x, ((pix, &wall), &o)) in pixels.enumerate() {
                    let p = Overlay::over(o, wall);
                    if p != *pix {
                        *pix = p;
                        first = first.min(x);
                        last = x;
                    }
                }
                if first == usize::MAX {
                    continue;
                }
                let (first, last) = (row + x0 + first, row + x0 + last);
                self.front[first..=last].copy_from_slice(&self.pix[first..=last]);
                let at = y * info.stride + (first - row) * 4;
                put_xrgb(&mut fb[at..at + (last + 1 - first) * 4], &self.pix[first..=last]);
                (top, bottom) = (top.min(y), bottom.max(y));
                (left, right) = (left.min(first - row), right.max(last - row));
            }
        }
        if top != usize::MAX {
            display.flush(
                screen,
                &[opendeskphone_hal::Rect::new(left as u16, top as u16, (right + 1 - left) as u16, (bottom + 1 - top) as u16)],
            )?;
        }
        Ok(true)
    }

    /// Forgets what was presented, so the next present repaints everything
    /// (after the panel was powered off, for example).
    pub fn invalidate(&mut self) {
        self.presented = false;
    }
}

// ----------------------------------------------------------------- touch

impl Canvas {
    /// Registers `r` as a place the picture can be touched, and what
    /// touching it does ([`crate::touch`]).  The part of `r` within the
    /// clip rectangle and the canvas counts (so a row half scrolled out
    /// of its list is touched only where it shows).  A region registered
    /// later is on top of the ones before it.
    pub fn hit(&mut self, r: IRect, hit: crate::touch::Hit) {
        let r = r.intersect(&self.clip);
        if !r.is_empty() {
            self.hits.push((r, hit));
        }
    }

    /// What touching the picture at (`x`, `y`) does: the topmost region
    /// there.
    pub fn hit_at(&self, x: i32, y: i32) -> Option<crate::touch::Hit> {
        let within = |r: &IRect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        self.hits.iter().rev().find(|(r, _)| within(r)).map(|(_, hit)| *hit)
    }

    /// Every region, in the order registered (tests and tools).
    pub fn hits(&self) -> &[(IRect, crate::touch::Hit)] {
        &self.hits
    }

    /// The first region registered with `hit` (tests and tools).
    pub fn hit_rect(&self, hit: crate::touch::Hit) -> Option<IRect> {
        self.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)
    }

    /// Forgets every region: the picture is about to be drawn again.
    pub fn clear_hits(&mut self) {
        self.hits.clear();
        self.quiet.clear();
        self.rounds.clear();
    }

    /// [`Canvas::hit`], for a region that shows nothing while a finger is
    /// down on it ([`Canvas::show_press`]): one that is not a control (a
    /// surface that is drawn on or dragged), or whose screen shows the
    /// finger in its own way ([`Canvas::pressed_in`]).
    pub fn hit_quiet(&mut self, r: IRect, hit: crate::touch::Hit) {
        let n = self.hits.len();
        self.hit(r, hit);
        if self.hits.len() > n {
            self.quiet.push(n);
        }
    }

    /// Says where a finger is down on the picture (`None`: nowhere), for
    /// the drawing that follows.  The UI does this.
    pub fn set_press(&mut self, at: Option<(i32, i32)>) {
        self.press = at;
    }

    /// Where a finger is down on the picture, if one is and has not begun
    /// to drag.
    pub fn pressed(&self) -> Option<(i32, i32)> {
        self.press
    }

    /// Whether a finger is down within `r`: for a screen that draws what
    /// is under the finger differently itself.  (Most need not: the UI
    /// shows the region under the finger as pressed,
    /// [`Canvas::show_press`].)
    pub fn pressed_in(&self, r: IRect) -> bool {
        self.press.is_some_and(|(x, y)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
    }

    /// Says what the screen about to be drawn is told of its place among
    /// the screens.  The UI does this.
    pub fn set_nav(&mut self, nav: NavCx) {
        self.nav = nav;
    }

    /// What the screen being drawn is told of its place among the
    /// screens.
    pub fn nav(&self) -> NavCx {
        self.nav
    }

    /// Shows the region under the finger ([`Canvas::set_press`]) as
    /// pressed, when the picture is drawn: its pixels are mixed a quarter
    /// of the way to white, or to black where the region is light.  The
    /// region is the topmost at the finger; nothing is shown unless its
    /// [`crate::touch::Hit`] is `expect` (what the finger came down on:
    /// the picture may have changed under it), nor for a region that
    /// does nothing or only dismisses, one registered with
    /// [`Canvas::hit_quiet`], or one of more than a quarter of the screen
    /// (a surface, not a control).
    ///
    /// The pressed look has the shape of the control where the control
    /// is a shape with round corners drawn with the region's rectangle
    /// (as [`crate::widgets::button`] draws one); else the region's
    /// rectangle.
    ///
    /// `to_white` is the way to mix, for a picture drawn more than once
    /// that must be mixed the same way each time; `None` chooses by the
    /// region's pixels.  Returns the way taken, or `None` when nothing
    /// was shown.
    pub fn show_press(&mut self, expect: crate::touch::Hit, to_white: Option<bool>) -> Option<bool> {
        use crate::touch::Hit;
        let (x, y) = self.press?;
        let within = |r: &IRect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let (i, (r, hit)) = self.hits.iter().copied().enumerate().rev().find(|(_, (r, _))| within(r))?;
        if hit != expect
            || matches!(hit, Hit::Nothing | Hit::Dismiss)
            || self.quiet.contains(&i)
            || r.w * r.h > self.w * self.h / 4
        {
            return None;
        }
        // The shape: the last one with round corners drawn where the
        // region is (the region is the part of it within the clip).
        let (shape, rad) = self
            .rounds
            .iter()
            .rev()
            .find(|(s, _)| s.intersect(&r) == r && s.w * s.h <= r.w * r.h * 2)
            .copied()
            .unwrap_or((r, 0));
        let lum = |p: Rgb| ((p >> 16) & 255) * 77 + ((p >> 8) & 255) * 150 + (p & 255) * 29;
        let to_white = to_white.unwrap_or_else(|| {
            let mut sum: u64 = 0;
            for yy in r.y..r.bottom() {
                let row = (yy * self.w) as usize;
                sum += self.pix[row + r.x as usize..row + r.right() as usize].iter().map(|p| u64::from(lum(*p))).sum::<u64>();
            }
            sum / (r.w as u64 * r.h as u64) < 140 * 256
        });
        let target = if to_white { 0xFF_FFFF } else { 0x00_0000 };
        let paint = r.intersect(&self.paint);
        for yy in paint.y..paint.bottom() {
            // The inset of this row of the shape, by its corner's circle.
            let from_edge = (yy - shape.y).min(shape.bottom() - 1 - yy);
            let inset = if from_edge < rad {
                let dy = rad - from_edge;
                let mut dx = 0;
                while (dx + 1) * (dx + 1) + dy * dy <= rad * rad + rad / 2 {
                    dx += 1;
                }
                rad - dx
            } else {
                0
            };
            let (x0, x1) = ((shape.x + inset).max(paint.x), (shape.right() - inset).min(paint.right()));
            let row = (yy * self.w) as usize;
            for xx in x0..x1 {
                let px = &mut self.pix[row + xx as usize];
                *px = mix(*px, target, 2);
            }
        }
        Some(to_white)
    }
}

// ------------------------------------------------------------- wallpaper

impl Canvas {
    /// Paints the idle/call background over the whole screen: the
    /// wallpaper on the color skin, the plain background on gray.  On a
    /// screen of another size than the wallpapers are described at (the
    /// touch skin's), the wallpaper drawn for the screen
    /// ([`crate::wallpaper::with_fitted`],
    /// [`crate::wallpaper::Wallpaper::with_frame_and_changes_sized`]).
    ///
    /// Returns the number of the frame drawn, of a wallpaper in motion
    /// ([`crate::wallpaper::Changes::serial`]).
    pub fn wallpaper(&mut self, plain: Rgb, backdrop: crate::wallpaper::Backdrop) -> Option<u64> {
        use crate::wallpaper::{Backdrop, HEIGHT, WIDTH};
        if self.gray {
            self.clear(plain);
            return None;
        }
        let clip = self.paint;
        if self.w != WIDTH || self.h != HEIGHT {
            let (w, h, pix) = (self.w, self.h, &mut self.pix);
            let mut copy = |src: &[u32]| {
                for y in clip.y..clip.bottom() {
                    let row = (y * w) as usize;
                    let (a, b) = (row + clip.x as usize, row + clip.right() as usize);
                    pix[a..b].copy_from_slice(&src[a..b]);
                }
            };
            if let Backdrop::Moving(wallpaper, t_ms) = backdrop {
                return Some(wallpaper.with_frame_and_changes_sized(w, h, t_ms, |frame, changes| {
                    copy(frame);
                    changes.serial
                }));
            }
            crate::wallpaper::with_fitted(backdrop, w, h, copy);
            return None;
        }
        let (w, pix) = (self.w, &mut self.pix);
        let mut copy = |src: &[u32]| {
            for y in clip.y..clip.bottom() {
                let row = (y * w) as usize;
                let (a, b) = (row + clip.x as usize, row + clip.right() as usize);
                pix[a..b].copy_from_slice(&src[a..b]);
            }
        };
        match backdrop {
            Backdrop::Builtin(wallpaper) => wallpaper.with_pixels(copy),
            Backdrop::Moving(wallpaper, t_ms) => {
                return Some(wallpaper.with_frame_and_changes(t_ms, |frame, changes| {
                    copy(frame);
                    changes.serial
                }));
            }
            Backdrop::Picture(picture) => copy(picture.pixels()),
        }
        None
    }

    /// Fills one row span `x0..x1` (exclusive) at `y`.
    pub fn span(&mut self, y: i32, x0: i32, x1: i32, c: Rgb) {
        self.fill(IRect::new(x0, y, x1 - x0, 1), c);
    }

    /// A softkey shape in `r`.  On the color skin: a rectangle whose
    /// bottom edge comes to a shallow point at the center, aimed at the
    /// physical key below (the last five rows of `r` are the point), with
    /// rounded corners and an optional one-pixel outline, all
    /// anti-aliased by coverage.  On gray: `r` filled, with its two top
    /// corners cut.
    pub fn softkey_shape(&mut self, r: IRect, fill: Rgb, edge: Option<Rgb>) {
        if self.gray {
            // The two top corners are cut.
            for j in 0..r.h {
                let inset = (2 - j).max(0);
                self.span(r.y + j, r.x + inset, r.right() - inset, fill);
            }
            return;
        }
        /// Rows of the point.
        const POINT: i32 = 5;
        /// One pixel in the fixed-point unit of the distances below.
        const ONE: i32 = 256;
        /// The radius of the outline's corners.
        const RADIUS: i32 = 3 * ONE;
        if r.w < 2 || r.h <= POINT {
            return;
        }
        let body = r.h - POINT;
        let edge = edge.unwrap_or(fill);
        // The outline is the top edge, the two sides, and two slanted
        // edges from the lower corners (at `body`) to the center of the
        // bottom of `r`.  The length of a slanted edge's direction vector
        // (w, 2 * POINT), in fixed point, turns its line equation into a
        // distance.
        let slant_len = ((r.w * r.w + 4 * POINT * POINT) as u32 * (ONE * ONE) as u32).isqrt() as i32;
        // Two edges that meet at a corner, rounded: where both distances
        // are within the radius, the distance is from the corner's circle.
        let round = |a: i32, b: i32| {
            if a > -RADIUS && b > -RADIUS {
                (((a + RADIUS) * (a + RADIUS) + (b + RADIUS) * (b + RADIUS)) as u32).isqrt() as i32 - RADIUS
            } else {
                a.max(b)
            }
        };
        // How much of a pixel lies inside an outline whose edge is `d`
        // from the pixel's center (negative inside), out of `ONE`.
        let coverage = |d: i32| (ONE / 2 - d).clamp(0, ONE) as u32;
        // How much of each pixel is inside the outline, and inside its
        // border: worked out once for a size (the softkeys are all one
        // size, and they are drawn with every frame).
        static COVERAGE: std::sync::Mutex<Option<((i32, i32), Vec<(u16, u16)>)>> = std::sync::Mutex::new(None);
        let mut kept = COVERAGE.lock().unwrap_or_else(|p| p.into_inner());
        if kept.as_ref().is_none_or(|(size, _)| *size != (r.w, r.h)) {
            let mut table = Vec::with_capacity((r.w * r.h) as usize);
            for j in 0..r.h {
                let py = j * ONE + ONE / 2;
                for x in 0..r.w {
                    // The shape is symmetric: fold onto the left half.
                    let px = x.min(r.w - 1 - x) * ONE + ONE / 2;
                    let side = -px;
                    let top = -py;
                    let slant = ((py - body * ONE) * r.w - 2 * POINT * px) * ONE / slant_len;
                    // The signed distance from the pixel's center to the
                    // outline, and to the inner edge of the one-pixel
                    // border.
                    let d = round(side, top).max(round(side, slant));
                    table.push((coverage(d) as u16, coverage(d + ONE) as u16));
                }
            }
            *kept = Some(((r.w, r.h), table));
        }
        let table = &kept.as_ref().expect("just set").1;
        for j in 0..r.h {
            for x in 0..r.w {
                let (outer, inner) = table[(j * r.w + x) as usize];
                if outer != 0 {
                    self.put_blend(r.x + x, r.y + j, blend(edge, fill, u32::from(inner), u32::from(outer)), u32::from(outer), ONE as u32);
                }
            }
        }
    }
}

/// Mixes `n` eighths of `b` into `a`.
pub fn mix(a: Rgb, b: Rgb, n: u32) -> Rgb {
    blend(a, b, n.min(8), 8)
}
