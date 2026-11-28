//! The Timer app's large digits: the digits, `:`, and `.` of the faces of
//! the idle clock, made into tables as the crate builds.  [`ROBOTO`]
//! (Roboto Thin) is for the color displays: rasterized once, large, and
//! shrunk here to the size it is drawn at.  [`HELVETICA`] (a Helvetica
//! bitmap font) is for the gray displays, drawn pixel for pixel, at its one
//! size.
//!
//! Each character is a cell, as wide as the character's advance and as
//! tall as the size asked for, with room above and below the digits.  A
//! cell is drawn as layers of one-color bitmaps, one for each shade, so
//! that Roboto's edges are smooth; the bitmap font has one shade.

pub struct Face {
    /// The face is shrunk to the size it is drawn at, or else drawn as it
    /// is.
    scales: bool,
    /// The height of a cell, as large as the face is.
    height: u16,
    /// How much of each pixel the characters cover, 0 to 255, row by row.
    coverage: &'static [u8],
    glyphs: &'static [Glyph],
}

struct Glyph {
    ch: char,
    /// The cell's width.
    advance: u16,
    /// The character's place in its cell, and its size.
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    /// Where its coverage begins.
    at: usize,
}

include!(concat!(env!("OUT_DIR"), "/faces.rs"));

/// How many shades a cell is drawn in.
pub const LEVELS: u8 = 6;

/// A character's cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub width: u16,
    pub height: u16,
    /// How much of each pixel the character covers, 0 to 255, row by row.
    pub coverage: Vec<u8>,
}

impl Face {
    fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.iter().find(|g| g.ch == c)
    }

    /// The height of cells asked to be `height` pixels tall.
    fn size(&self, height: u16) -> u16 {
        if self.scales {
            height.clamp(1, self.height)
        } else {
            self.height
        }
    }

    /// The width of a cell of `g`, `height` pixels tall.
    fn cell_width(&self, g: &Glyph, height: u16) -> u16 {
        let (advance, full) = (u32::from(g.advance), u32::from(self.height));
        ((advance * u32::from(height) + full / 2) / full).max(1) as u16
    }

    /// The width of `text` in cells `height` pixels tall.
    pub fn width(&self, text: &str, height: u16) -> u32 {
        let height = self.size(height);
        text.chars()
            .filter_map(|c| self.glyph(c))
            .map(|g| u32::from(self.cell_width(g, height)))
            .sum()
    }

    /// The tallest cells in which `text` fits in `w` by `h`; for a face
    /// that is drawn as it is, its one size.
    pub fn fit(&self, text: &str, w: u16, h: u16) -> u16 {
        let mut height = self.size(h);
        while self.scales && height > 1 && self.width(text, height) > u32::from(w) {
            height -= 1;
        }
        height
    }

    /// The cell of `c`, `height` pixels tall (at most as large as the
    /// face is, and for a face drawn as it is, that size), or `None` for a
    /// character other than a digit, `:`, or `.`.
    pub fn cell(&self, c: char, height: u16) -> Option<Cell> {
        let g = self.glyph(c)?;
        let height = self.size(height);
        let (sw, sh) = (usize::from(g.advance), usize::from(self.height));
        let mut src = vec![0u8; sw * sh];
        let (x, y, w) = (usize::from(g.x), usize::from(g.y), usize::from(g.w));
        for row in 0..usize::from(g.h) {
            let from = g.at + row * w;
            src[(y + row) * sw + x..][..w].copy_from_slice(&self.coverage[from..from + w]);
        }
        let width = self.cell_width(g, height);
        let coverage = shrink(&src, sw, sh, usize::from(width), usize::from(height));
        Some(Cell {
            width,
            height,
            coverage,
        })
    }
}

/// The pixels of a `sw` by `sh` picture averaged into `tw` by `th`, which
/// is no larger: each pixel of the result is the mean of the area of the
/// picture it covers.
fn shrink(src: &[u8], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<u8> {
    // Source pixel i spans [i * t, (i + 1) * t), and result pixel j spans
    // [j * s, (j + 1) * s), for sizes s and t along one axis.
    let spans = |s: usize, t: usize, i: usize| {
        let (from, to) = (i * t, (i + 1) * t);
        (from / s..(to.div_ceil(s)).min(t)).map(move |j| (j, to.min((j + 1) * s) - from.max(j * s)))
    };
    let mut rows = vec![0u64; tw * sh];
    for y in 0..sh {
        for i in 0..sw {
            let v = u64::from(src[y * sw + i]);
            if v != 0 {
                for (j, part) in spans(sw, tw, i) {
                    rows[y * tw + j] += v * part as u64;
                }
            }
        }
    }
    let mut sums = vec![0u64; tw * th];
    for y in 0..sh {
        for (k, part) in spans(sh, th, y) {
            for x in 0..tw {
                sums[k * tw + x] += rows[y * tw + x] * part as u64;
            }
        }
    }
    let whole = (sw * sh) as u64;
    sums.into_iter()
        .map(|s| ((s + whole / 2) / whole) as u8)
        .collect()
}

/// The bitmaps that draw `cell` in [`LEVELS`] shades: each shade (from
/// 1, the faintest, to `LEVELS`, the character's color) with its pixels,
/// one bit each, rows from the top, each row padded to whole bytes, the
/// leftmost pixel in the high bit.  A pixel is in the shade nearest to its
/// coverage, and a shade that has no pixels is left out.
pub fn layers(cell: &Cell) -> Vec<(u8, Vec<u8>)> {
    let (w, h) = (usize::from(cell.width), usize::from(cell.height));
    let stride = w.div_ceil(8);
    let level = |coverage: u8| (u32::from(coverage) * u32::from(LEVELS) + 127) / 255;
    (1..=LEVELS)
        .filter_map(|shade| {
            let mut bits = vec![0u8; stride * h];
            let mut any = false;
            for y in 0..h {
                for x in 0..w {
                    if level(cell.coverage[y * w + x]) == u32::from(shade) {
                        bits[y * stride + x / 8] |= 0x80 >> (x % 8);
                        any = true;
                    }
                }
            }
            any.then_some((shade, bits))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_keeps_the_mean() {
        // Two by two of 0 and 255 is half covered; three to two splits
        // the middle pixel.
        assert_eq!(shrink(&[0, 255, 255, 0], 2, 2, 1, 1), [128]);
        assert_eq!(shrink(&[255, 0, 0], 3, 1, 2, 1), [170, 0]);
    }

    #[test]
    fn the_text_fits_where_it_is_put() {
        for (w, h) in [(240, 70), (480, 160), (768, 300), (100, 400)] {
            let height = ROBOTO.fit("00:00:00", w, h);
            let width = |height| ROBOTO.width("00:00:00", height);
            assert!(
                height <= h && width(height) <= u32::from(w),
                "{w}x{h}: {height}"
            );
            assert!(
                width(height + 1) > u32::from(w) || height == h.min(ROBOTO.height),
                "{w}x{h}: {height}"
            );
        }
    }

    #[test]
    fn the_bitmap_font_is_drawn_as_it_is_in_one_shade() {
        let height = HELVETICA.fit("00:00:00", 240, 400);
        let cell = HELVETICA.cell('0', 400).unwrap();
        assert_eq!(cell.height, height);
        assert!(cell.coverage.iter().all(|c| *c == 0 || *c == 255));
        assert_eq!(
            layers(&cell)
                .into_iter()
                .map(|(shade, _)| shade)
                .collect::<Vec<_>>(),
            [LEVELS]
        );
    }
}
