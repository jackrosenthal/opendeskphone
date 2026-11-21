//! The screen mirror of one watcher: the picture it was last sent, and
//! what to send it for the next frame ([`Mirror::next`]).
//!
//! A watcher that asks for patches gets the whole screen first, and then
//! only the rectangles that changed.  The frames are compared in squares
//! of [`TILE`] pixels; the runs of changed squares in each row of squares
//! make rectangles, which grow downward while the run below spans the
//! same squares, and are merged where a patch more would cost more than
//! the pixels between them ([`PATCH_COST`]).  Each rectangle is then
//! narrowed to the pixels that changed in it.  On the D80, the waves of
//! a moving wallpaper change about 150 of the 4000 squares from one
//! frame to the next, and a call's timer two or three.

use crate::png::{self, Rect};
use crate::MirrorFrame;

/// The side of the squares the frames are compared in, in pixels.
const TILE: usize = 16;
/// What a patch costs besides its pixels (its PNG's chunks and its place
/// in the message, about 80 bytes, and its decoding), in squares: two
/// rectangles become one when that costs at most this many squares more.
/// Of 1 to 32 (and squares of 8 and 32 pixels), 8 sent the fewest bytes
/// for the D80's moving wallpaper and its menus, in about 18 patches a
/// change.
const PATCH_COST: usize = 8;
/// The most patches a message has: more rectangles are merged into fewer.
const MAX_PATCHES: usize = 32;
/// With more rectangles than this before merging (the merge takes the
/// cube of their number), the change is one rectangle around them all.
const MAX_RUNS: usize = 128;

/// What to send for a frame.
#[derive(Debug)]
pub enum Picture {
    /// The whole screen, as a PNG.
    Whole(Vec<u8>),
    /// The rectangles that changed, each with a PNG of it, to be drawn in
    /// order over the picture last sent.
    Patches(Vec<(Rect, Vec<u8>)>),
}

pub struct Mirror {
    patches: bool,
    /// The frame last sent, for a watcher that takes patches.
    last: Option<MirrorFrame>,
}

impl Mirror {
    /// A watcher's mirror; with `patches`, a change after the first
    /// picture is sent as the rectangles that changed.
    pub fn new(patches: bool) -> Mirror {
        Mirror { patches, last: None }
    }

    /// What to send for `frame`: `None` when it is the picture last sent,
    /// or its buffer is too short for its geometry.
    pub fn next(&mut self, frame: MirrorFrame) -> Option<Picture> {
        if !png::sound(&frame) {
            return None;
        }
        if !self.patches {
            return png::encode(&frame).map(Picture::Whole);
        }
        let last = self.last.replace(frame);
        let frame = self.last.as_ref().expect("just kept");
        let rects = match &last {
            Some(last) if (last.width, last.height, last.format) == (frame.width, frame.height, frame.format) => changed(last, frame),
            _ => return png::encode(frame).map(Picture::Whole),
        };
        if rects.is_empty() {
            return None;
        }
        // Most of the screen: the whole of it, which is one picture to
        // encode and decode, and stands on its own.
        let area: usize = rects.iter().map(Rect::area).sum();
        if 2 * area > frame.width as usize * frame.height as usize {
            return png::encode(frame).map(Picture::Whole);
        }
        let patches = rects.into_iter().map(|r| png::encode_rect(frame, r).map(|png| (r, png))).collect::<Option<Vec<_>>>();
        if patches.is_none() {
            // Cannot happen (the rectangles are within the frame); if it
            // did, the watcher would be left without the change, so the
            // next picture is whole.
            self.last = None;
        }
        patches.map(Picture::Patches)
    }
}

/// Row `y` of a frame: its pixels, without the padding.
fn row(f: &MirrorFrame, y: usize) -> &[u8] {
    let len = f.width as usize * png::bytes_per_pixel(f.format);
    &f.pixels[y * f.stride..y * f.stride + len]
}

/// The rectangles in which `b` differs from `a`, which have the same size
/// and format; none if they are the same.
fn changed(a: &MirrorFrame, b: &MirrorFrame) -> Vec<Rect> {
    let (w, h) = (b.width as usize, b.height as usize);
    let bpp = png::bytes_per_pixel(b.format);
    let (cols, rows) = (w.div_ceil(TILE), h.div_ceil(TILE));
    // The squares that changed.
    let mut dirty = vec![false; cols * rows];
    for y in 0..h {
        let (ra, rb) = (row(a, y), row(b, y));
        if ra == rb {
            continue;
        }
        let line = &mut dirty[y / TILE * cols..][..cols];
        for (d, (sa, sb)) in line.iter_mut().zip(ra.chunks(TILE * bpp).zip(rb.chunks(TILE * bpp))) {
            *d = *d || sa != sb;
        }
    }
    // Their runs in each row of squares, as rectangles of squares; a run
    // that spans the same squares as one in the row above extends it.
    let mut boxes: Vec<Rect> = Vec::new();
    let mut above: Vec<usize> = Vec::new();
    for ty in 0..rows {
        let line = &dirty[ty * cols..][..cols];
        let mut here = Vec::new();
        let mut tx = 0;
        while tx < cols {
            if !line[tx] {
                tx += 1;
                continue;
            }
            let start = tx;
            while tx < cols && line[tx] {
                tx += 1;
            }
            match above.iter().find(|&&i| boxes[i].x == start && boxes[i].w == tx - start) {
                Some(&i) => {
                    boxes[i].h += 1;
                    here.push(i);
                }
                None => {
                    boxes.push(Rect { x: start, y: ty, w: tx - start, h: 1 });
                    here.push(boxes.len() - 1);
                }
            }
        }
        above = here;
    }
    if boxes.len() > MAX_RUNS {
        boxes = vec![boxes.iter().copied().reduce(|u, r| union(&u, &r)).expect("more than none")];
    }
    merge(&mut boxes);
    boxes
        .into_iter()
        .map(|s| {
            let (x, y) = (s.x * TILE, s.y * TILE);
            let r = Rect { x, y, w: (s.w * TILE).min(w - x), h: (s.h * TILE).min(h - y) };
            narrow(a, b, r)
        })
        .collect()
}

fn union(a: &Rect, b: &Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect { x, y, w: (a.x + a.w).max(b.x + b.w) - x, h: (a.y + a.h).max(b.y + b.h) - y }
}

/// Merges the rectangles, the pair that costs least first, while a merge
/// costs at most [`PATCH_COST`] or there are more than [`MAX_PATCHES`].
fn merge(boxes: &mut Vec<Rect>) {
    loop {
        let mut best: Option<(isize, usize, usize)> = None;
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let extra = union(&boxes[i], &boxes[j]).area() as isize - (boxes[i].area() + boxes[j].area()) as isize;
                if best.is_none_or(|(e, ..)| extra < e) {
                    best = Some((extra, i, j));
                }
            }
        }
        match best {
            Some((extra, i, j)) if extra <= PATCH_COST as isize || boxes.len() > MAX_PATCHES => {
                boxes[i] = union(&boxes[i], &boxes[j]);
                boxes.swap_remove(j);
            }
            _ => return,
        }
    }
}

/// `r` narrowed to the pixels in it that differ between `a` and `b`
/// (there is at least one).
fn narrow(a: &MirrorFrame, b: &MirrorFrame, r: Rect) -> Rect {
    let bpp = png::bytes_per_pixel(b.format);
    let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
    for y in r.y..r.y + r.h {
        let pa = &row(a, y)[r.x * bpp..(r.x + r.w) * bpp];
        let pb = &row(b, y)[r.x * bpp..(r.x + r.w) * bpp];
        if pa == pb {
            continue;
        }
        let differs = |i: &usize| pa[i * bpp..(i + 1) * bpp] != pb[i * bpp..(i + 1) * bpp];
        x0 = x0.min((0..r.w).find(differs).expect("a row that differs"));
        x1 = x1.max((0..r.w).rfind(differs).expect("a row that differs"));
        y0 = y0.min(y);
        y1 = y;
    }
    debug_assert!(y0 <= y1, "no pixel differs in {r:?}");
    Rect { x: r.x + x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MirrorFormat;

    fn gray(w: u16, h: u16) -> MirrorFrame {
        MirrorFrame { width: w, height: h, stride: w as usize + 3, format: MirrorFormat::Gray8, pixels: vec![7; (w as usize + 3) * h as usize] }
    }

    fn set(f: &mut MirrorFrame, x: usize, y: usize) {
        f.pixels[y * f.stride + x] ^= 0xff;
    }

    fn rects(p: Option<Picture>) -> Vec<Rect> {
        match p {
            Some(Picture::Patches(p)) => p.into_iter().map(|(r, _)| r).collect(),
            other => panic!("not patches: {other:?}"),
        }
    }

    #[test]
    fn the_first_picture_is_whole_and_a_change_is_a_patch() {
        let mut m = Mirror::new(true);
        let mut f = gray(100, 60);
        assert!(matches!(m.next(f.clone()), Some(Picture::Whole(_))));
        assert!(m.next(f.clone()).is_none(), "the same picture again");
        set(&mut f, 40, 30);
        set(&mut f, 42, 33);
        assert_eq!(rects(m.next(f.clone())), [Rect { x: 40, y: 30, w: 3, h: 4 }]);
        // The padding at the end of a row is not the picture.
        f.pixels[f.stride - 1] = 1;
        assert!(m.next(f.clone()).is_none());
    }

    #[test]
    fn changes_far_apart_are_patches_of_their_own() {
        let mut m = Mirror::new(true);
        let mut f = gray(800, 1280);
        m.next(f.clone());
        // A clock at the top and a timer at the bottom: in squares a
        // square apart, one patch; far apart, two.
        for (x, y) in [(10, 10), (30, 25), (700, 1200), (730, 1210)] {
            set(&mut f, x, y);
        }
        assert_eq!(rects(m.next(f.clone())), [Rect { x: 10, y: 10, w: 21, h: 16 }, Rect { x: 700, y: 1200, w: 31, h: 11 }]);
        // Seven squares apart in a row, one patch; eleven apart, two.
        set(&mut f, 100, 100);
        set(&mut f, 230, 101);
        assert_eq!(rects(m.next(f.clone())), [Rect { x: 100, y: 100, w: 131, h: 2 }]);
        set(&mut f, 100, 300);
        set(&mut f, 300, 301);
        assert_eq!(rects(m.next(f.clone())), [Rect { x: 100, y: 300, w: 1, h: 1 }, Rect { x: 300, y: 301, w: 1, h: 1 }]);
    }

    /// Whether every pixel of `points` is in one of `rects`.
    fn covered(rects: &[Rect], points: &[(usize, usize)]) -> bool {
        points.iter().all(|&(x, y)| rects.iter().any(|r| (r.x..r.x + r.w).contains(&x) && (r.y..r.y + r.h).contains(&y)))
    }

    #[test]
    fn many_changes_are_merged_into_few_patches() {
        let mut m = Mirror::new(true);
        let mut f = gray(800, 1280);
        m.next(f.clone());
        // A pixel in every other square of every other row of squares at
        // the top: 25 x 15 rectangles before merging, more than MAX_RUNS,
        // so one rectangle around them.
        for ty in (0..30).step_by(2) {
            for tx in (0..50).step_by(2) {
                set(&mut f, tx * TILE + 3, ty * TILE + 5);
            }
        }
        assert_eq!(rects(m.next(f.clone())), [Rect { x: 3, y: 5, w: 48 * TILE + 1, h: 28 * TILE + 1 }]);
        // 40 pixels ten squares apart: too far apart to merge for their
        // cost, but more than MAX_PATCHES.
        let points: Vec<(usize, usize)> = (0..40).map(|i| (i % 5 * 10 * TILE + 3, i / 5 * 10 * TILE + 5)).collect();
        for &(x, y) in &points {
            set(&mut f, x, y);
        }
        let r = rects(m.next(f.clone()));
        assert_eq!(r.len(), MAX_PATCHES);
        assert!(covered(&r, &points), "{r:?}");
        // A diagonal line: a rectangle for each row of squares, and merged
        // where they touch.
        let points: Vec<(usize, usize)> = (0..800).map(|i| (i, 400 + i)).collect();
        for &(x, y) in &points {
            set(&mut f, x, y);
        }
        let r = rects(m.next(f.clone()));
        assert!(r.len() <= MAX_PATCHES, "{r:?}");
        assert!(covered(&r, &points), "{r:?}");
        // Far less than the rectangle around the line.
        let area: usize = r.iter().map(Rect::area).sum();
        assert!(area * 5 < 800 * 800, "{area}");
    }

    #[test]
    fn most_of_the_screen_is_the_whole_screen() {
        let mut m = Mirror::new(true);
        let mut f = gray(64, 64);
        m.next(f.clone());
        set(&mut f, 0, 0);
        set(&mut f, 63, 40);
        assert_eq!(rects(m.next(f.clone())).len(), 2);
        // The top 33 rows of 64: more than half.
        for y in 0..33 {
            for x in 0..64 {
                set(&mut f, x, y);
            }
        }
        assert!(matches!(m.next(f.clone()), Some(Picture::Whole(_))));
        // A frame of another size: whole.
        let g = gray(32, 64);
        assert!(matches!(m.next(g), Some(Picture::Whole(_))));
    }

    #[test]
    fn without_patches_every_picture_is_whole() {
        let mut m = Mirror::new(false);
        let mut f = gray(100, 60);
        assert!(matches!(m.next(f.clone()), Some(Picture::Whole(_))));
        set(&mut f, 1, 1);
        assert!(matches!(m.next(f.clone()), Some(Picture::Whole(_))));
        assert!(m.last.is_none(), "nothing is kept");
    }

    #[test]
    fn color_frames_compare_whole_pixels() {
        let mut m = Mirror::new(true);
        let mut f = MirrorFrame { width: 40, height: 40, stride: 160, format: MirrorFormat::Xrgb8888, pixels: vec![0; 160 * 40] };
        m.next(f.clone());
        // The red byte of pixel (20, 30).
        f.pixels[30 * 160 + 20 * 4 + 2] = 200;
        assert_eq!(rects(m.next(f.clone())), [Rect { x: 20, y: 30, w: 1, h: 1 }]);
    }
}
