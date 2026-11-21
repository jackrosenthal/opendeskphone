//! The pictures: where the board goes on a display, and the squares as
//! filled rectangles and one-color bitmaps (what the phone's app canvas
//! draws).  The pictures are computed for the size of a square, so each
//! display gets them at its own size; they are this project's own.
//!
//! The level is drawn in two lists.  [`Painter::scenery`] is what never
//! changes while a level is played (floor, walls, targets), and
//! [`Painter::pieces`] is the boxes and the figure, a handful of
//! operations to send after each step.

use crate::{Game, Level, MAX_HEIGHT, MAX_WIDTH};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// One drawing operation, in pixels from the top left of the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Fill {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        color: Rgb,
    },
    /// A square picture of one color: one bit per pixel, rows from the
    /// top, each row padded to whole bytes, the leftmost pixel in the
    /// highest bit.  Clear bits leave what is underneath.
    Bitmap {
        x: i16,
        y: i16,
        size: u16,
        bits: Rc<[u8]>,
        color: Rgb,
    },
}

/// Where things go in an app window: the board on the left, and a panel
/// for the level number and the counts on the right; in an upright window
/// ([`Layout::upright`]), the panel across the top and the board under
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The side of a square, in pixels.
    pub tile: u16,
    pub board_x: i16,
    pub board_y: i16,
    /// Room for the largest level.
    pub board_width: u16,
    pub board_height: u16,
    pub panel_x: i16,
    pub panel_width: u16,
}

impl Layout {
    /// The layout for a window of this size.  The panel gets at least a
    /// quarter of the width, and the squares are as large as the largest
    /// level then allows: 14 pixels in the smallest window (240x102), 20
    /// in 320x140, and 32 in 480x243.
    pub fn new(width: u16, height: u16) -> Layout {
        let tile = ((width - width / 4) / MAX_WIDTH as u16).min(height / MAX_HEIGHT as u16);
        let (board_width, board_height) = (tile * MAX_WIDTH as u16, tile * MAX_HEIGHT as u16);
        Layout {
            tile,
            board_x: 0,
            board_y: ((height - board_height) / 2) as i16,
            board_width,
            board_height,
            panel_x: board_width as i16,
            panel_width: width - board_width,
        }
    }

    /// The layout for an upright window of this size (a touchscreen's):
    /// the panel is the `header` pixels across the top, and the board is
    /// in the middle of what is below, its squares as large as the
    /// width allows for the largest level: 72 pixels in 800x1160.
    pub fn upright(width: u16, height: u16, header: u16) -> Layout {
        let below = height.saturating_sub(header);
        let tile = (width / MAX_WIDTH as u16).min(below / MAX_HEIGHT as u16);
        let (board_width, board_height) = (tile * MAX_WIDTH as u16, tile * MAX_HEIGHT as u16);
        Layout {
            tile,
            board_x: ((width - board_width) / 2) as i16,
            board_y: (header + (below - board_height) / 2) as i16,
            board_width,
            board_height,
            panel_x: 0,
            panel_width: width,
        }
    }
}

/// The colors of the pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The floor inside the walls (`None`: the window's background).
    pub floor: Option<Rgb>,
    pub wall: Rgb,
    pub wall_line: Rgb,
    pub target: Rgb,
    pub box_fill: Rgb,
    pub box_line: Rgb,
    /// A box on a target.
    pub home_fill: Rgb,
    pub home_line: Rgb,
    pub home_mark: Rgb,
    pub body: Rgb,
    pub head: Rgb,
    pub eye: Rgb,
}

impl Palette {
    /// For a display of grays, from its foreground and background colors.
    /// The pictures differ in shape, not in shade: a box is an outline
    /// with a cross, a box on a target is solid with a diamond cut out, a
    /// target is a small hollow diamond, and a wall is a pattern of
    /// bricks.
    pub fn gray(fg: Rgb, bg: Rgb) -> Palette {
        Palette {
            floor: None,
            wall: fg,
            wall_line: bg,
            target: fg,
            box_fill: bg,
            box_line: fg,
            home_fill: fg,
            home_line: fg,
            home_mark: bg,
            body: fg,
            head: fg,
            eye: bg,
        }
    }

    /// For a color display.
    pub fn color() -> Palette {
        Palette {
            floor: Some(Rgb(0xD9, 0xD0, 0xBA)),
            wall: Rgb(0x5F, 0x6E, 0x8C),
            wall_line: Rgb(0x2F, 0x38, 0x4C),
            target: Rgb(0xC6, 0x3A, 0x32),
            box_fill: Rgb(0xE0, 0x9A, 0x38),
            box_line: Rgb(0x7A, 0x45, 0x12),
            home_fill: Rgb(0x3E, 0xA6, 0x55),
            home_line: Rgb(0x1C, 0x5E, 0x2C),
            home_mark: Rgb(0xE8, 0xF7, 0xD8),
            body: Rgb(0x1F, 0x6F, 0xD0),
            head: Rgb(0xF2, 0xC0, 0x8E),
            eye: Rgb(0x20, 0x20, 0x20),
        }
    }
}

/// A square one-bit picture being drawn.
struct Bits {
    size: i32,
    stride: usize,
    data: Vec<u8>,
}

impl Bits {
    fn new(size: u16) -> Bits {
        let stride = (size as usize).div_ceil(8);
        Bits {
            size: size as i32,
            stride,
            data: vec![0; stride * size as usize],
        }
    }

    /// Sets or clears the pixels for which `shape` holds.
    fn paint(&mut self, on: bool, shape: impl Fn(i32, i32) -> bool) {
        for y in 0..self.size {
            for x in 0..self.size {
                if shape(x, y) {
                    let (byte, bit) = (y as usize * self.stride + x as usize / 8, 0x80 >> (x % 8));
                    if on {
                        self.data[byte] |= bit;
                    } else {
                        self.data[byte] &= !bit;
                    }
                }
            }
        }
    }

    /// The pixels from (`x0`, `y0`) up to but not including (`x1`, `y1`).
    fn rect(&mut self, on: bool, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.paint(on, |x, y| (x0..x1).contains(&x) && (y0..y1).contains(&y));
    }

    fn done(self) -> Rc<[u8]> {
        self.data.into()
    }
}

/// The pictures of the squares at one size.
struct Sprites {
    bricks: Rc<[u8]>,
    target: Rc<[u8]>,
    /// The outline of a box and the cross on it.
    crate_lines: Rc<[u8]>,
    /// The outline of a box on a target.
    home_lines: Rc<[u8]>,
    home_mark: Rc<[u8]>,
    body: Rc<[u8]>,
    head: Rc<[u8]>,
    eyes: Rc<[u8]>,
}

/// How far a box is from the edges of its square.
fn inset(tile: u16) -> i32 {
    (tile as i32 / 16).max(1)
}

impl Sprites {
    fn new(tile: u16) -> Sprites {
        let t = tile as i32;
        // A hundredth of the square, to the nearest pixel.
        let at = |percent: i32| (t * percent + 50) / 100;
        // A diamond around the middle of the square: `reach` is twice the
        // distance from the middle to a point, so that odd and even sizes
        // both come out symmetric.
        let diamond = |reach: i32| move |x: i32, y: i32| (2 * x + 1 - t).abs() + (2 * y + 1 - t).abs() <= reach;
        let line = (t / 16).max(1);

        // Two courses of bricks, the joints of one over the middle of the
        // other's bricks.
        let mut bricks = Bits::new(tile);
        bricks.rect(true, 0, t / 2 - line, t, t / 2);
        bricks.rect(true, 0, t - line, t, t);
        bricks.rect(true, t / 4, 0, t / 4 + line, t / 2);
        bricks.rect(true, 3 * t / 4, t / 2, 3 * t / 4 + line, t);

        let reach = t * 6 / 10;
        let mut target = Bits::new(tile);
        target.paint(true, diamond(reach));
        target.paint(false, diamond(reach - 4 * ((t + 4) / 12)));

        let m = inset(tile);
        let border = (t + 6) / 10;
        let outline = |bits: &mut Bits| {
            bits.rect(true, m, m, t - m, t - m);
            bits.rect(false, m + border, m + border, t - m - border, t - m - border);
        };
        let mut crate_lines = Bits::new(tile);
        let arm = t / 20;
        crate_lines.paint(true, |x, y| (x - y).abs() <= arm || (x + y - (t - 1)).abs() <= arm);
        crate_lines.paint(false, |x, y| x < m || y < m || x >= t - m || y >= t - m);
        let mut frame = Bits::new(tile);
        outline(&mut frame);
        for (to, from) in crate_lines.data.iter_mut().zip(&frame.data) {
            *to |= from;
        }
        // Smaller than the target's diamond, so that the box around it
        // stays mostly solid.
        let mut home_mark = Bits::new(tile);
        home_mark.paint(true, diamond(t / 2));

        // The figure, seen from the front: a round head, a body with the
        // arms out, and two legs.
        let mut head = Bits::new(tile);
        let (middle, radius) = (at(30), at(21));
        head.paint(true, |x, y| {
            let (dx, dy) = (2 * x + 1 - t, 2 * y + 1 - 2 * middle);
            dx * dx + dy * dy <= 4 * radius * radius
        });
        let mut body = Bits::new(tile);
        body.rect(true, at(24), at(52), t - at(24), at(82));
        body.rect(true, at(8), at(52), t - at(8), at(66));
        body.rect(true, at(27), at(82), at(44), at(96));
        body.rect(true, t - at(44), at(82), t - at(27), at(96));
        let mut eyes = Bits::new(tile);
        let (dot, gap) = ((t / 14).max(1), (t / 14).max(1));
        let top = middle - (dot + 1) / 2;
        eyes.rect(true, t / 2 - gap - dot, top, t / 2 - gap, top + dot);
        eyes.rect(true, t - t / 2 + gap, top, t - t / 2 + gap + dot, top + dot);

        Sprites {
            bricks: bricks.done(),
            target: target.done(),
            crate_lines: crate_lines.done(),
            home_lines: frame.done(),
            home_mark: home_mark.done(),
            body: body.done(),
            head: head.done(),
            eyes: eyes.done(),
        }
    }
}

/// Draws levels at one size in one set of colors.
pub struct Painter {
    tile: u16,
    palette: Palette,
    sprites: Sprites,
}

impl Painter {
    pub fn new(tile: u16, palette: Palette) -> Painter {
        Painter {
            tile,
            palette,
            sprites: Sprites::new(tile),
        }
    }

    /// The top left corner of a square of the level, on the board; a
    /// level smaller than the largest is in the middle of the board.
    pub fn corner(&self, level: &Level, i: usize) -> (i16, i16) {
        let t = self.tile as usize;
        let x = (MAX_WIDTH - level.width) * t / 2 + (i % level.width) * t;
        let y = (MAX_HEIGHT - level.height) * t / 2 + (i / level.width) * t;
        (x as i16, y as i16)
    }

    fn fill(&self, (x, y): (i16, i16), margin: i32, color: Rgb) -> Op {
        Op::Fill {
            x: x + margin as i16,
            y: y + margin as i16,
            width: self.tile - 2 * margin as u16,
            height: self.tile - 2 * margin as u16,
            color,
        }
    }

    fn bitmap(&self, (x, y): (i16, i16), bits: &Rc<[u8]>, color: Rgb) -> Op {
        Op::Bitmap {
            x,
            y,
            size: self.tile,
            bits: bits.clone(),
            color,
        }
    }

    /// The floor, the walls, and the targets.  Wall that touches no floor
    /// of the level, even at a corner, is not drawn.
    pub fn scenery(&self, level: &Level) -> Vec<Op> {
        let inside = level.inside();
        let (w, h) = (level.width, level.height);
        let mut ops = Vec::new();
        for i in 0..w * h {
            let at = self.corner(level, i);
            if inside[i] {
                if let Some(floor) = self.palette.floor {
                    ops.push(self.fill(at, 0, floor));
                }
                if level.targets[i] {
                    ops.push(self.bitmap(at, &self.sprites.target, self.palette.target));
                }
                continue;
            }
            let (x, y) = (i % w, i / w);
            let touches = (y.saturating_sub(1)..(y + 2).min(h))
                .any(|ny| (x.saturating_sub(1)..(x + 2).min(w)).any(|nx| inside[ny * w + nx]));
            if touches {
                ops.push(self.fill(at, 0, self.palette.wall));
                ops.push(self.bitmap(at, &self.sprites.bricks, self.palette.wall_line));
            }
        }
        ops
    }

    /// The boxes and the figure, where they are now.
    pub fn pieces(&self, game: &Game) -> Vec<Op> {
        let level = game.level();
        let p = &self.palette;
        let m = inset(self.tile);
        let mut ops = Vec::new();
        for i in (0..level.walls.len()).filter(|i| game.has_box(*i)) {
            let at = self.corner(level, i);
            if level.targets[i] {
                ops.push(self.fill(at, m, p.home_fill));
                if p.home_line != p.home_fill {
                    ops.push(self.bitmap(at, &self.sprites.home_lines, p.home_line));
                }
                ops.push(self.bitmap(at, &self.sprites.home_mark, p.home_mark));
            } else {
                ops.push(self.fill(at, m, p.box_fill));
                ops.push(self.bitmap(at, &self.sprites.crate_lines, p.box_line));
            }
        }
        let at = self.corner(level, game.player());
        ops.push(self.bitmap(at, &self.sprites.body, p.body));
        ops.push(self.bitmap(at, &self.sprites.head, p.head));
        ops.push(self.bitmap(at, &self.sprites.eyes, p.eye));
        ops
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dir, LEVELS};

    const BLACK: Rgb = Rgb(0, 0, 0);
    const WHITE: Rgb = Rgb(255, 255, 255);

    /// Paints operations as the phone does.
    struct Picture {
        width: usize,
        pixels: Vec<Rgb>,
    }

    impl Picture {
        fn new(layout: &Layout, background: Rgb) -> Picture {
            let (w, h) = (layout.board_width as usize, layout.board_height as usize);
            Picture {
                width: w,
                pixels: vec![background; w * h],
            }
        }

        fn set(&mut self, x: i32, y: i32, color: Rgb) {
            let height = (self.pixels.len() / self.width) as i32;
            assert!(x >= 0 && y >= 0 && x < self.width as i32 && y < height, "drawn outside the board");
            self.pixels[y as usize * self.width + x as usize] = color;
        }

        fn paint(&mut self, ops: &[Op]) {
            for op in ops {
                match op {
                    Op::Fill { x, y, width, height, color } => {
                        for py in 0..*height as i32 {
                            for px in 0..*width as i32 {
                                self.set(*x as i32 + px, *y as i32 + py, *color);
                            }
                        }
                    }
                    Op::Bitmap { x, y, size, bits, color } => {
                        let stride = (*size as usize).div_ceil(8);
                        assert_eq!(bits.len(), stride * *size as usize);
                        for py in 0..*size as usize {
                            for px in 0..*size as usize {
                                if bits[py * stride + px / 8] & (0x80 >> (px % 8)) != 0 {
                                    self.set(*x as i32 + px as i32, *y as i32 + py as i32, *color);
                                }
                            }
                        }
                    }
                }
            }
        }

        /// The pixels of one square of the largest level's grid.
        fn square(&self, tile: u16, column: usize, row: usize) -> Vec<Rgb> {
            let t = tile as usize;
            (0..t * t)
                .map(|i| self.pixels[(row * t + i / t) * self.width + column * t + i % t])
                .collect()
        }
    }

    const WINDOWS: [(u16, u16); 3] = [(240, 102), (320, 140), (480, 243)];

    #[test]
    fn the_board_and_the_panel_share_the_window() {
        let tiles: Vec<u16> = WINDOWS.iter().map(|(w, h)| Layout::new(*w, *h).tile).collect();
        assert_eq!(tiles, [14, 20, 32]);
        for (w, h) in WINDOWS {
            let l = Layout::new(w, h);
            assert_eq!(l.board_width, l.tile * MAX_WIDTH as u16);
            assert_eq!(l.board_height, l.tile * MAX_HEIGHT as u16);
            assert!(l.board_y >= 0 && l.board_y as u16 + l.board_height <= h);
            assert_eq!(l.panel_x as u16, l.board_x as u16 + l.board_width);
            assert_eq!(l.panel_x as u16 + l.panel_width, w);
            assert!(l.panel_width >= w / 4);
        }
    }

    #[test]
    fn an_upright_window_has_the_panel_over_the_board() {
        let l = Layout::upright(800, 1160, 96);
        assert_eq!(l.tile, 72);
        assert_eq!((l.board_width, l.board_height), (792, 504));
        assert_eq!((l.board_x, l.board_y), (4, 96 + (1064 - 504) / 2));
        assert_eq!((l.panel_x, l.panel_width), (0, 800));
        // A window too low for the width's squares has smaller ones.
        let l = Layout::upright(800, 446, 96);
        assert_eq!(l.tile, 50);
        assert!(l.board_y as u16 + l.board_height <= 446);
    }

    /// A row of everything there is to tell apart: floor, wall, target,
    /// box, box on a target, the figure.
    fn sampler() -> Game {
        let mut game = Game::new(Level::parse("#########\n#x.o.pox#\n#########").unwrap());
        assert_eq!(game.step(Dir::Right), crate::Step::Pushed);
        game
    }

    #[test]
    fn the_squares_can_be_told_apart_without_color() {
        for (w, h) in WINDOWS {
            let layout = Layout::new(w, h);
            let painter = Painter::new(layout.tile, Palette::gray(BLACK, WHITE));
            let game = sampler();
            let mut picture = Picture::new(&layout, WHITE);
            picture.paint(&painter.scenery(game.level()));
            picture.paint(&painter.pieces(&game));
            // The level is nine squares wide and three high, in the middle.
            let (left, row) = ((MAX_WIDTH - 9) / 2, (MAX_HEIGHT - 3) / 2 + 1);
            let names = ["wall", "target", "floor", "box", "floor", "floor", "figure", "box on a target", "wall"];
            let squares: Vec<Vec<Rgb>> = (0..9).map(|c| picture.square(layout.tile, left + c, row)).collect();
            assert_eq!(squares[2], squares[4]);
            assert!(squares[2].iter().all(|p| *p == WHITE), "floor is the background");
            let area = layout.tile as usize * layout.tile as usize;
            for a in 0..9 {
                // Every picture is black and white only, and bold: at
                // least an eighth of the square is ink (the target, the
                // smallest picture), and none is all ink.
                let ink = squares[a].iter().filter(|p| **p == BLACK).count();
                assert!(squares[a].iter().all(|p| *p == BLACK || *p == WHITE));
                if names[a] != "floor" {
                    assert!(ink * 8 >= area && ink < area, "{}: {ink} of {area} at {}", names[a], layout.tile);
                }
                for b in a + 1..9 {
                    if names[a] == names[b] {
                        continue;
                    }
                    // Different things differ in at least a seventh of
                    // their pixels.
                    let differ = squares[a].iter().zip(&squares[b]).filter(|(p, q)| p != q).count();
                    assert!(differ * 7 >= area, "{} and {}: {differ} of {area} at {}", names[a], names[b], layout.tile);
                }
            }
        }
    }

    #[test]
    fn every_level_is_drawn_inside_the_board_and_within_the_canvas_limits() {
        for (w, h) in WINDOWS {
            let layout = Layout::new(w, h);
            for palette in [Palette::gray(BLACK, WHITE), Palette::color()] {
                let painter = Painter::new(layout.tile, palette);
                for def in LEVELS {
                    let game = Game::new(Level::parse(def.text).unwrap());
                    let (scenery, pieces) = (painter.scenery(game.level()), painter.pieces(&game));
                    for ops in [&scenery, &pieces] {
                        // The limits of `canvas.draw`.
                        assert!(ops.len() <= 512);
                        let bytes: usize = ops
                            .iter()
                            .map(|op| match op {
                                Op::Bitmap { bits, .. } => bits.len(),
                                Op::Fill { .. } => 0,
                            })
                            .sum();
                        assert!(bytes <= 64 * 1024);
                    }
                    // Painting checks the bounds.
                    let mut picture = Picture::new(&layout, WHITE);
                    picture.paint(&scenery);
                    picture.paint(&pieces);
                }
            }
        }
    }

    #[test]
    fn a_step_changes_only_the_pieces() {
        let painter = Painter::new(14, Palette::color());
        let mut game = sampler();
        let (scenery, before) = (painter.scenery(game.level()), painter.pieces(&game));
        game.undo();
        assert_eq!(painter.scenery(game.level()), scenery);
        assert_ne!(painter.pieces(&game), before);
    }
}
