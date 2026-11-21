//! The motion of the wallpapers that move on the idle screen: waves that
//! rise and fall, circles that wander, and a flow that runs on.
//!
//! A frame is the picture at a time, and the time is all it depends on:
//! every motion is a whole number of turns in [`CYCLE_MS`], so the
//! picture of a time is the picture one cycle later, and a frame may be
//! skipped or drawn late.
//!
//! The phone draws several frames a second on a 500 MHz ARM1176 without
//! floating point, beside everything else it does, so a frame draws only
//! what the motion changed since the frame before, and what it draws
//! comes from tables wherever a table can stand for arithmetic:
//!
//! - Waves: the pixels a crest passed over.
//! - Circles: the ring at each circle's edge, where its opacity changes
//!   (inside the ring it is the same wherever the circle goes), as wide
//!   as the edge and the way the circle went.  A circle's opacity comes
//!   from a table by the square of the distance from its center, so no
//!   pixel needs a square root.  A circle moves in steps of as many
//!   frames as leave the change in any pixel too small to see: the
//!   sharper its edge and the faster it goes, the more often.
//! - A flow: every pixel changes, so every other row is drawn in a frame,
//!   from tables for the straight run, the waves, and the colors.
//!
//! The parts that do not move (the gradient under the waves and the
//! circles) are drawn once and kept, with two more bits a channel than
//! the frame buffer has, and the frame is dithered with the noise of the
//! still picture.  One difference from the still picture: a channel is
//! kept a step and a half (of the frame buffer's 255) away from 0 and
//! from 255, so that the noise can be added to all three channels of a
//! pixel at once.

use super::*;

/// The time after which every motion is where it began.
pub const CYCLE_MS: u64 = 120_000;

/// Where a motion of `turns` turns a cycle is at `t_ms`, in 1/65536 turn.
fn turn_at(t_ms: u64, turns: i32) -> i32 {
    let phase = (t_ms % CYCLE_MS) as i64 * 65536 / CYCLE_MS as i64;
    ((phase * i64::from(turns)) & 0xFFFF) as i32
}

/// The farthest a turn of `reach` (in any parts of a pixel) at `turns`
/// turns a cycle goes in `ms` milliseconds, in the same parts, rounded up:
/// a turn's fastest is 2 pi times its reach.
fn fastest(reach: i32, turns: i32, ms: u64) -> u64 {
    (u64::from(reach.unsigned_abs()) * u64::from(turns.unsigned_abs()) * 6_284 * ms).div_ceil(CYCLE_MS * 1000)
}

/// The farthest the colors of a flow go between two of a row's turns
/// (frames `every` milliseconds apart), in sixteenths of a pixel, rounded
/// up: as far as its waves move the positions, over how fast the
/// positions run across the screen (`whole` along `ax`).  A bound: its
/// waves at their fastest, all at once.
fn flow_step(waves: impl Iterator<Item = (i32, i32)>, whole: i32, ax: i32, every: u64) -> u64 {
    let positions: u64 = waves.map(|(amount, turns)| fastest(amount * 16, turns, FIELDS * every)).sum();
    (positions * u64::from(whole.unsigned_abs())).div_ceil(u64::from((ONE * ax.max(1)).unsigned_abs()))
}

/// The shortest time from one frame to the next, in milliseconds: twenty
/// frames a second.
const SHORTEST: u32 = 50;

/// The time from one frame of `spec`'s motion to the next on a screen of
/// `fit`, in milliseconds; `None` for a wallpaper that does not move
/// there.
///
/// On the color skin's screen it is the description's.  On a larger
/// screen the motion goes as far as the composition is large
/// ([`Fit::size`]), so a frame moves things farther in pixels: there it is
/// the longest whole part of the description's time (no shorter than
/// [`SHORTEST`]) with which nothing goes farther than a pixel from one
/// frame to the next (a flow's colors: from one of a row's turns to the
/// next).  The circles move in steps of whole frames, chosen by this time
/// ([`Disc::new`]).
pub(super) fn every_ms(spec: &Spec, fit: &Fit) -> Option<u32> {
    let moves = spec.moves.as_ref()?;
    if fit.is_native() {
        return Some(moves.every_ms);
    }
    // The farthest anything goes in a frame of `every` milliseconds, in
    // sixteenths of a pixel (a bound, as `Moving::farthest` has it).
    let step = |every: u64| -> Option<u64> {
        match (&moves.motion, &spec.base) {
            (Motion::Bob(bob), _) => {
                let reach = |r: i32| fit.size(r).abs();
                if bob.iter().any(|&(r, ..)| reach(r) + 8 > ABOVE) {
                    return None;
                }
                Some(bob.iter().map(|&(r, turns, _)| fastest(reach(r), turns, every)).max().unwrap_or(0))
            }
            (Motion::Wander { reach, turns }, _) => {
                // As `Disc::new` has its speed, at the most reach and the
                // most turns.
                let (a, b) = (fastest(fit.size(reach.0 * 16), turns.1, every), fastest(fit.size(reach.1 * 16), turns.1, every));
                Some(a.max(b) + a.min(b).div_ceil(2))
            }
            (Motion::Run(turns), Base::Flow { flow: Flow::Ribbons { ax, ay, waves }, .. }) if (fit.w as usize).is_multiple_of(KNOT) => {
                let waves = waves.iter().enumerate().map(|(i, &(amount, ..))| (amount, turns.get(i).copied().unwrap_or(0)));
                Some(flow_step(waves, fit.w * ax + fit.h * ay, *ax, every))
            }
            (Motion::Run(_), _) => None,
        }
    };
    // The whole parts of the description's time that a cycle has a whole
    // number of pairs of (the rows of a flow take turns), longest first.
    let every = moves.every_ms;
    let times = (1..=every / SHORTEST.min(every)).filter(|k| every % k == 0).map(|k| every / k);
    let mut chosen = every;
    for time in times.filter(|t| CYCLE_MS.is_multiple_of(2 * u64::from(*t))) {
        chosen = time;
        if step(u64::from(time))? <= 16 {
            break;
        }
    }
    Some(chosen)
}

// ----------------------------------------------------- to the frame buffer

/// A channel's least and greatest value here: with the noise and the
/// rounding (-6 to 9) it stays within its ten bits.
const LEAST: i32 = 6;
const MOST: i32 = 1014;

fn limit(c: Px) -> Px {
    c.map(|v| v.clamp(LEAST, MOST))
}

/// One in each channel of a packed color.
const EACH: u32 = 0x0010_0401;

/// What is added to a packed color before its low two bits are dropped:
/// the rounding and, by the top four bits of the noise, the dither (as
/// [`to_frame`] has it for one channel).
const NOISE: [u32; 16] = {
    let mut table = [0; 16];
    let mut i = 0;
    while i < 16 {
        table[i] = ((i as i32 - 8 + 2) as u32).wrapping_mul(EACH);
        i += 1;
    }
    table
};

/// The noise of pixel (`x`, `y`); the next pixel's is [`NOISE_STEP`] more.
#[inline]
fn noise_at(x: i32, y: i32) -> u32 {
    ((x * 49471 + y * 37345) & 0xFFFF) as u32
}
const NOISE_STEP: u32 = 49471;

/// What is added to a packed color before its low two bits are dropped,
/// where it is not dithered: the rounding.
const FLAT: [u32; 16] = [2 * EACH; 16];

/// A packed color within the limits as the frame buffer's `0x00RRGGBB`.
#[inline]
fn shown(p: u32, noise: u32, dither: bool) -> u32 {
    to_frame_buffer(p.wrapping_add(if dither { NOISE[((noise >> 12) & 15) as usize] } else { FLAT[0] }))
}

/// A packed color, its rounding and dither added, as the frame buffer's
/// `0x00RRGGBB`: the low two bits of each channel dropped.
#[inline]
fn to_frame_buffer(q: u32) -> u32 {
    ((q >> 6) & 0xFF_0000) | ((q >> 4) & 0xFF00) | ((q >> 2) & 0xFF)
}

/// Blends `c` into the packed color with opacity `t` of [`ONE`].
#[inline]
fn blend(p: u32, c: Px, t: i32) -> u32 {
    let (r, g, b) = ((p >> 20) as i32, ((p >> 10) & 1023) as i32, (p & 1023) as i32);
    let r = r + (((c[0] - r) * t + ONE / 2) >> 12);
    let g = g + (((c[1] - g) * t + ONE / 2) >> 12);
    let b = b + (((c[2] - b) * t + ONE / 2) >> 12);
    ((r as u32) << 20) | ((g as u32) << 10) | b as u32
}

/// A wallpaper in motion: what is kept from one frame to the next.
pub(super) struct Moving {
    pub(super) id: &'static str,
    /// The screen it moves on: its size, and how the description is laid
    /// out on it (the motion's reach grows as the sizes do).
    pub(super) fit: Fit,
    /// The frame, as the canvas takes it (`0x00RRGGBB`).
    out: Vec<u32>,
    /// The parts that do not move, as packed [`Px`] colors within the
    /// limits (empty for a flow, which has none).
    base: Vec<u32>,
    dither: bool,
    kind: Kind,
    /// The time from one frame to the next, in milliseconds.
    every: u64,
    /// A frame was drawn: `out` holds the frame before this one.
    drawn: bool,
    /// The pixels the last frame drew.
    changed: Vec<Run>,
    /// The number of the frame in `out` ([`next_serial`]); 0 before the
    /// first.
    pub(super) serial: u64,
}

/// A number for a frame, unlike that of any other frame of any wallpaper.
pub(super) fn next_serial() -> u64 {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Adds the pixels `x0..=x1` of row `y` to the runs.
#[inline]
fn run(changed: &mut Vec<Run>, y: usize, x0: usize, x1: usize) {
    changed.push(Run { y: y as u16, x0: x0 as u16, x1: x1 as u16 + 1 });
}

enum Kind {
    Waves(Waves),
    Discs(Discs),
    Flow(Stream),
}

impl Moving {
    /// What `spec` needs to move on a screen of `fit`; `None` when it does
    /// not.
    pub(super) fn new(id: &'static str, spec: &'static Spec, fit: Fit) -> Option<Moving> {
        let moves = spec.moves.as_ref()?;
        let every = u64::from(every_ms(spec, &fit)?);
        let base: Vec<u32> = match spec.base {
            Base::Flow { .. } => Vec::new(),
            _ => render_base(&spec.base, &fit).into_iter().map(|p| pack(limit(unpack(p)))).collect(),
        };
        let kind = match (&moves.motion, &spec.base) {
            (Motion::Bob(bob), _) => Kind::Waves(Waves::new(spec.layers, bob, fit)?),
            (Motion::Wander { reach, turns }, _) => Kind::Discs(Discs::new(spec.layers, *reach, *turns, every, &base, fit)),
            (Motion::Run(turns), Base::Flow { flow: Flow::Ribbons { ax, ay, waves }, stops }) => {
                Kind::Flow(Stream::new(*ax, *ay, waves, turns, stops, every, fit)?)
            }
            (Motion::Run(_), _) => return None,
        };
        Some(Moving {
            id,
            fit,
            out: vec![0; (fit.w * fit.h) as usize],
            base,
            dither: spec.dither,
            kind,
            every,
            drawn: false,
            changed: Vec::new(),
            serial: 0,
        })
    }

    /// The farthest anything in the picture goes from one frame to the
    /// next, over a cycle, in sixteenths of a pixel, rounded up: where the
    /// crests and the circles are in each frame.  For a flow, a bound:
    /// its waves at their fastest, all at once.
    pub(super) fn farthest(&self) -> u64 {
        let every = self.every;
        let frames = 0..CYCLE_MS / every;
        match &self.kind {
            Kind::Waves(w) => frames
                .flat_map(|n| {
                    w.bob.iter().map(move |&(reach, turns, begin)| {
                        let off = |t: u64| (reach * sin(turn_at(t, turns) + begin * 256)) >> 12;
                        u64::from(off((n + 1) * every).abs_diff(off(n * every)))
                    })
                })
                .max()
                .unwrap_or(0),
            Kind::Discs(d) => frames
                .flat_map(|n| {
                    d.discs.iter().map(move |d| {
                        let (a, b) = (d.place(n * every), d.place((n + 1) * every));
                        let (dx, dy) = (u64::from(a.0.abs_diff(b.0)), u64::from(a.1.abs_diff(b.1)));
                        let square = dx * dx + dy * dy;
                        let root = square.isqrt();
                        root + u64::from(root * root < square)
                    })
                })
                .max()
                .unwrap_or(0),
            Kind::Flow(s) => flow_step(s.waves.iter().map(|&((amount, ..), turns)| (amount, turns)), s.whole, s.ax, every),
        }
    }

    /// Draws the frame of `t_ms` and returns it.
    pub(super) fn frame(&mut self, t_ms: u64) -> &[u32] {
        self.frame_and_changes(t_ms).0
    }

    /// Draws the frame of `t_ms`; returns it, the pixels drawn, which are
    /// all that can differ from the frame before, and its number.
    pub(super) fn frame_and_changes(&mut self, t_ms: u64) -> (&[u32], &[Run], u64) {
        let first = !std::mem::replace(&mut self.drawn, true);
        self.serial = next_serial();
        self.changed.clear();
        match &mut self.kind {
            Kind::Waves(w) => w.frame(t_ms, first, &self.base, &mut self.out, self.dither, &mut self.changed),
            Kind::Discs(d) => d.frame(t_ms, first, &self.base, &mut self.out, self.dither, &mut self.changed),
            Kind::Flow(s) => s.frame(t_ms, &mut self.out, self.dither, &mut self.changed),
        }
        (&self.out, &self.changed, self.serial)
    }
}

// ------------------------------------------------------------------ waves

/// One wave, ready to draw.
///
/// A wave's crest rises and falls; what is under the crest stays as it is
/// where the wave rests.  A pixel's opacity is the fill's at its depth
/// under the resting crest, times how much of the pixel is under the
/// crest as it is now: so a frame changes the pixels the crests passed
/// over and no others, also of a wave whose fill fades with the depth
/// (the fade is so gentle that a crest a few pixels from its rest cannot
/// be told from one that took its fade along).
struct Swell {
    color: Px,
    /// The fill's opacity (of [`ONE`]) by how far under the resting crest
    /// a pixel's center is, in sixteenths of a pixel, from [`ABOVE`]
    /// above it; farther under than the table goes, the last entry.
    fill: Vec<u16>,
    /// It hides what is under it, from 8 sixteenths below its crest on.
    hides: bool,
    /// Each column's crest where the wave rests, in sixteenths of a pixel.
    shape: Vec<i32>,
    /// The crests of the frame in `out`, and of the frame before it.
    edge: Vec<i32>,
    before: Vec<i32>,
    /// The highest and lowest crest of the frame in `out`.
    span: (i32, i32),
}

/// How far above its resting crest a wave's fill is tabled, in sixteenths
/// of a pixel: farther than any wave rises.
const ABOVE: i32 = 16 * 16;

impl Swell {
    /// The wave's opacity (of [`ONE`]) at the pixel of column `x` whose
    /// center is at height `yc`.
    #[inline]
    fn opacity(&self, x: usize, yc: i32) -> i32 {
        // How much of the pixel is under the crest, in sixteenths.
        let under = yc - self.edge[x] + 8;
        if under <= 0 {
            return 0;
        }
        let at = (yc - self.shape[x] + ABOVE).clamp(0, self.fill.len() as i32 - 1);
        (i32::from(self.fill[at as usize]) * under.min(16)) >> 4
    }
}

struct Waves {
    waves: Vec<Swell>,
    /// For each wave: how far it goes each way (in sixteenths of a pixel
    /// of the screen), its turns in a cycle, and where in a turn it begins.
    bob: Vec<(i32, i32, i32)>,
    line: Vec<u32>,
    /// The screen's size.
    w: i32,
    h: i32,
}

impl Waves {
    fn new(layers: &'static [Layer], bob: &'static [(i32, i32, i32)], fit: Fit) -> Option<Waves> {
        let [Layer::Waves(list)] = layers else { return None };
        let bob: Vec<(i32, i32, i32)> = bob.iter().map(|&(reach, turns, begin)| (fit.size(reach), turns, begin)).collect();
        if list.len() != bob.len() || bob.iter().any(|(reach, ..)| reach.abs() + 8 > ABOVE) {
            return None;
        }
        let waves = list
            .iter()
            .map(|w| {
                // As `wave` has it.
                let shape: Vec<i32> = (0..fit.w).map(|x| crest(w, &fit, x)).collect();
                let fade = fit.size(w.fade * 16);
                let deepest = if fade > 0 { fade } else { -ABOVE };
                let fill = (-ABOVE..=deepest)
                    .map(|depth| {
                        let a = if fade > 0 { w.alpha * (fade - depth).max(0) / fade } else { w.alpha };
                        (a * 16).clamp(0, ONE) as u16
                    })
                    .collect();
                Swell { color: limit(px(w.color)), fill, hides: w.alpha >= 256 && w.fade == 0, edge: shape.clone(), before: shape.clone(), shape, span: (0, 0) }
            })
            .collect();
        Some(Waves { waves, bob, line: vec![0; fit.w as usize], w: fit.w, h: fit.h })
    }

    /// The color of the pixel in column `x` whose center is at height
    /// `yc`, over `base`.
    #[inline]
    fn pixel(&self, base: u32, x: usize, yc: i32) -> u32 {
        // A wave that hides what is under it is where to begin.
        let mut p = base;
        let mut from = 0;
        for (i, wave) in self.waves.iter().enumerate().rev() {
            if wave.hides && yc - wave.edge[x] >= 8 {
                p = pack(wave.color);
                from = i + 1;
                break;
            }
        }
        for wave in &self.waves[from..] {
            let t = wave.opacity(x, yc);
            if t > 0 {
                p = blend(p, wave.color, t);
            }
        }
        p
    }

    fn frame(&mut self, t_ms: u64, first: bool, base: &[u32], out: &mut [u32], dither: bool, changed: &mut Vec<Run>) {
        let w = self.w as usize;
        let height = self.h;
        // The crests of this frame, and of the frame before.
        for (wave, &(reach, turns, begin)) in self.waves.iter_mut().zip(&self.bob) {
            let off = (reach * sin(turn_at(t_ms, turns) + begin * 256)) >> 12;
            std::mem::swap(&mut wave.edge, &mut wave.before);
            wave.span = (i32::MAX, i32::MIN);
            for (edge, rest) in wave.edge.iter_mut().zip(&wave.shape) {
                *edge = rest + off;
                wave.span = (wave.span.0.min(*edge), wave.span.1.max(*edge));
            }
        }
        if first {
            // Every row.
            let mut line = std::mem::take(&mut self.line);
            for y in 0..height {
                let yc = y * 16 + 8;
                let row = y as usize * w;
                line.copy_from_slice(&base[row..row + w]);
                for wave in &self.waves {
                    if yc + 8 <= wave.span.0 {
                        continue;
                    }
                    if wave.hides && yc - wave.span.1 >= 8 {
                        line.fill(pack(wave.color));
                        continue;
                    }
                    for (x, p) in line.iter_mut().enumerate() {
                        let t = wave.opacity(x, yc);
                        if t > 0 {
                            *p = blend(*p, wave.color, t);
                        }
                    }
                }
                let mut noise = noise_at(0, y);
                for (o, p) in out[row..row + w].iter_mut().zip(&line) {
                    *o = shown(*p, noise, dither);
                    noise = noise.wrapping_add(NOISE_STEP);
                }
                run(changed, y as usize, 0, w - 1);
            }
            self.line = line;
            return;
        }
        // The pixels each crest passed over: in each column the rows from
        // where the crest was to where it is.  A crest covers part of the
        // pixels whose centers are less than half a pixel from it, so a
        // crest that went from `old` to `now` changed the rows with
        // centers from half a pixel above the higher of the two to half a
        // pixel below the lower: one row, mostly.  They are drawn along
        // the crest (the first of those rows in every column, then the
        // second, and so on), not column by column: pixels side by side
        // are side by side in memory.
        for i in 0..self.waves.len() {
            let rows = |x: usize| {
                let (now, old) = (self.waves[i].edge[x], self.waves[i].before[x]);
                ((now.min(old) >> 4).max(0), ((now.max(old) - 1) >> 4).min(height - 1))
            };
            for nth in 0.. {
                // The run of pixels being drawn: its row, and where it began.
                let mut open: Option<(i32, usize)> = None;
                let mut any = false;
                for x in 0..w {
                    let (from, to) = rows(x);
                    let y = from + nth;
                    if open.is_some_and(|(row, _)| row != y || y > to)
                        && let Some((row, x0)) = open.take()
                    {
                        run(changed, row as usize, x0, x - 1);
                    }
                    if y > to {
                        continue;
                    }
                    any = true;
                    let at = y as usize * w + x;
                    out[at] = shown(self.pixel(base[at], x, y * 16 + 8), noise_at(x as i32, y), dither);
                    open.get_or_insert((y, x));
                }
                if let Some((row, x0)) = open {
                    run(changed, row as usize, x0, w - 1);
                }
                if !any {
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- circles

/// A circle that wanders.
struct Disc {
    /// Its place, in sixteenths of a pixel.
    home: (i32, i32),
    /// How far it goes each way, its turns in a cycle, and where in a turn
    /// it begins, across and then up and down.
    path: [(i32, i32, i32); 2],
    /// It moves once in this many milliseconds, this long after the
    /// others that do.
    step: (u64, u64),
    color: Px,
    /// The opacity (of [`ONE`]) by the square of the distance from the
    /// center in sixteenths of a pixel, `shift` bits dropped; none beyond
    /// the table.
    opacity: Vec<u16>,
    shift: u32,
    /// By how many rows a pixel is from the center: how many columns to
    /// either side the circle reaches, wherever in its pixel the center
    /// is.
    outer: Vec<u16>,
    /// The radius beyond which the circle is not there, and the radius
    /// within which its opacity is the same everywhere (not above zero
    /// where there is no such part), in sixteenths of a pixel; each with
    /// room for the steps of the opacity's table.
    edge: i32,
    level: i32,
    /// Where it is in the frame in `out`.
    at: (i32, i32),
}

/// The farthest a circle moves between two frames, in sixteenths of a
/// pixel, for its ring alone to be drawn again; one that moved farther (a
/// frame was skipped) is drawn again whole.
const MARGIN: i32 = 16;

/// A circle moves in steps of as many frames as change no channel of a
/// pixel by more than about this much (in steps of the frame buffer's
/// 255): a small circle with a sharp edge moves with every frame, a large
/// faint one with a wide, soft edge once in several, and none of them is
/// seen to move in steps.  The circles that wait take turns.
const STEP: i64 = 2;

/// The most frames between two steps of a circle.
const WAIT: u64 = 16;

impl Disc {
    /// Circle number `index` of a wallpaper, on a screen of `fit`
    /// (`base` is what is under the circles there); `reach` is in pixels
    /// of the description.
    fn new(index: u32, circle: Circle, soft: i32, rim: i32, reach: (i32, i32), turns: (i32, i32), every_ms: u64, base: &[u32], fit: &Fit) -> Disc {
        let (cx, cy, r) = fit.circle(circle);
        let (color, alpha) = (circle.3, circle.4);
        let soft = soft_edge(r, soft);
        let reach = (fit.size(reach.0 * 16), fit.size(reach.1 * 16));
        let edge = r + soft / 2 + 16;
        // A table of at most 1024 entries on the color skin's screen, and
        // on a larger one as many more as the circles are larger (an edge
        // is as wide there): the steps across the edge of the largest
        // circle are a tenth of a pixel, or less.
        let entries = 1024 * fit.size(256) / 256;
        let mut shift = 0;
        while ((edge * edge) >> shift) >= entries {
            shift += 1;
        }
        let opacity: Vec<u16> = (0..=(edge * edge) >> shift)
            .map(|i| {
                let d = (((i << shift) + ((1 << shift) >> 1)) as u32).isqrt() as i32;
                disc_opacity(d, r, soft, alpha, rim).clamp(0, ONE) as u16
            })
            .collect();
        // The radius within which the opacity is the same everywhere.
        let level = (r - soft / 2).min(if rim > 0 { r - 36 } else { i32::MAX });
        // A step of the table, as a distance, at each of the two radii.
        let table = |radius: i32| (1 << shift) / (2 * radius.max(1)) + 2;
        let far = (edge >> 4) + 2;
        let half = |radius: i32, dy: i32| if dy < radius { ((radius * radius - dy * dy) as u32).isqrt() as i32 } else { 0 };
        let outer: Vec<u16> = (0..=far).map(|dy| (half(far, dy) + 1) as u16).collect();
        // Each circle has a path of its own, the same every time: the
        // turns across and up and down differ, so the path is not a line.
        let h = [hash(index as i32, 1, 0x51ed), hash(index as i32, 2, 0x51ed)];
        let span = (turns.1 - turns.0 + 1).max(1) as u32;
        let across = turns.0 + (h[0] % span) as i32;
        let mut up = turns.0 + ((h[0] >> 8) % span) as i32;
        if up == across && span > 1 {
            up = if across == turns.1 { turns.0 } else { across + 1 };
        }
        // Between half of the reach and all of it.
        let go = |reach: i32, h: u32| reach * (128 + (h >> 20) as i32 % 129) / 256;
        let path = [(go(reach.0, h[0]), across, (h[1] & 0xFFFF) as i32), (go(reach.1, h[1]), up, ((h[1] >> 4) & 0xFFFF) as i32)];
        // How many frames to a step.  The fastest the circle goes, in
        // sixteenths of a pixel a second, rounded up (a turn's fastest is
        // 2 pi times its reach; the two ways are seldom at their fastest
        // together):
        let fastest = |(far, turns, _): (i32, i32, i32)| (u64::from(far.unsigned_abs()) * u64::from(turns.unsigned_abs()) * 6_284).div_ceil(CYCLE_MS);
        let (a, b) = (fastest(path[0]), fastest(path[1]));
        let speed = a.max(b) + a.min(b).div_ceil(2);
        // The most a channel changes where the circle moves by a pixel,
        // times sixteen: the steepest part of its edge (a smooth step is
        // half again as steep in the middle as on average), and of its
        // rim (which rises over 20 sixteenths), by how far its color is
        // from what is under its center.
        let under = unpack(base[((cy >> 4).clamp(0, fit.h - 1) * fit.w + (cx >> 4).clamp(0, fit.w - 1)) as usize]);
        let contrast = i64::from((0..3).map(|i| (px(color)[i] - under[i]).abs()).max().unwrap_or(0) / 4);
        let steep = contrast * i64::from(alpha) * if soft > 32 { 24 } else { 16 } / i64::from(soft) + contrast * i64::from(rim) * 16 / 20;
        // The change in a step of `frames` frames is `steep` / 256 a
        // sixteenth of a pixel, for `speed` * `frames` * `every_ms` / 1000
        // of them.  The steps are a whole number to a cycle, and none is
        // farther than the ring that is drawn again allows.
        let fits = |frames: u64| {
            let moved = (speed * frames * every_ms).div_ceil(1000) as i64;
            (CYCLE_MS / every_ms).is_multiple_of(frames) && moved <= i64::from(MARGIN) && steep * moved <= STEP * 256 * 16
        };
        let frames = (1..=WAIT).rev().find(|f| fits(*f)).unwrap_or(1);
        let step = (frames * every_ms, u64::from(index) % frames * every_ms);
        Disc {
            home: (cx, cy),
            path,
            step,
            color: limit(px(color)),
            opacity,
            shift,
            outer,
            edge: r + soft / 2 + table(r + soft / 2),
            level: level - table(level),
            at: (cx, cy),
        }
    }

    fn place(&self, t_ms: u64) -> (i32, i32) {
        // The time of its last step.
        let t = t_ms % CYCLE_MS + CYCLE_MS;
        let t = t - (t + self.step.1) % self.step.0;
        let go = |(far, turns, begin): (i32, i32, i32)| (far * sin(turn_at(t, turns) + begin)) >> 12;
        (self.home.0 + go(self.path[0]), self.home.1 + go(self.path[1]))
    }

    /// The columns of row `y` the circle reaches when its center is in
    /// pixel (`cx`, `cy`): the first and the last.
    #[inline]
    fn row(&self, cx: i32, cy: i32, y: i32) -> Option<(i32, i32)> {
        let half = i32::from(*self.outer.get((y - cy).unsigned_abs() as usize)?);
        Some((cx - half, cx + half))
    }

    /// The columns of row `y` in which the circle with its center at
    /// `at` (in sixteenths of a pixel) differs from the same circle up to
    /// `moved` sixteenths away: one stretch, or (`ring`, and the row
    /// crosses the part where the opacity is level) two, the second of
    /// them `None` otherwise.
    #[inline]
    fn changed(&self, at: (i32, i32), moved: i32, ring: bool, y: i32) -> Option<((i32, i32), Option<(i32, i32)>)> {
        let dy = y * 16 + 8 - at.1;
        let reach = self.edge + moved;
        if dy.abs() >= reach {
            return None;
        }
        // Pixels whose centers are within `half` of the center, across.
        let half = ((reach * reach - dy * dy) as u32).isqrt() as i32;
        let (x0, x1) = ((at.0 - half - 8) >> 4, (at.0 + half + 8) >> 4);
        let level = self.level - moved;
        if !ring || dy.abs() >= level {
            return Some(((x0, x1), None));
        }
        // Those strictly within `inside` are level.
        let inside = ((level * level - dy * dy) as u32).isqrt() as i32;
        let (l, r) = ((at.0 - inside + 8) >> 4, (at.0 + inside - 8) >> 4);
        if l + 1 >= r {
            return Some(((x0, x1), None));
        }
        Some(((x0, l), Some((r, x1))))
    }
}

struct Discs {
    discs: Vec<Disc>,
    line: Vec<u32>,
    /// The screen's size.
    w: i32,
    h: i32,
}

impl Discs {
    /// The circles of the layers, in the order they are drawn.
    fn new(layers: &'static [Layer], reach: (i32, i32), turns: (i32, i32), every_ms: u64, base: &[u32], fit: Fit) -> Discs {
        let mut discs = Vec::new();
        for layer in layers {
            let (soft, rim, circles) = match layer {
                Layer::Circles { soft, rim, circles } => (*soft, *rim, circles.to_vec()),
                Layer::Bubbles { seed, count, r, alpha, soft, rim, colors } => (*soft, *rim, bubbles(*seed, *count, *r, *alpha, colors)),
                _ => continue,
            };
            for circle in circles {
                discs.push(Disc::new(discs.len() as u32, circle, soft, rim, reach, turns, every_ms, base, &fit));
            }
        }
        Discs { discs, line: vec![0; fit.w as usize], w: fit.w, h: fit.h }
    }

    fn frame(&mut self, t_ms: u64, first: bool, base: &[u32], out: &mut [u32], dither: bool, changed: &mut Vec<Run>) {
        let (width, height) = (self.w, self.h);
        let w = width as usize;
        // The circles that moved: which, where its center is, how far it
        // moved, and whether its ring alone changed.
        let mut moved: Vec<(usize, (i32, i32), i32, bool)> = Vec::new();
        // The rows with something to draw again.
        let (mut top, mut bottom) = (height, -1);
        for (i, d) in self.discs.iter_mut().enumerate() {
            let at = d.place(t_ms);
            if at == d.at && !first {
                continue;
            }
            let (dx, dy) = ((at.0 - d.at.0).abs(), (at.1 - d.at.1).abs());
            let far = (d.edge >> 4) + 2;
            if dx.max(dy) <= MARGIN {
                // (Not farther than this, in a straight line.)
                let by = dx.max(dy) + dx.min(dy) / 2 + 1;
                moved.push((i, at, by, true));
                (top, bottom) = (top.min((at.1 >> 4) - far - 2), bottom.max((at.1 >> 4) + far + 2));
            } else {
                // It jumped (a frame was skipped): where it was, and
                // where it is, whole.
                moved.push((i, d.at, 0, false));
                moved.push((i, at, 0, false));
                (top, bottom) = (top.min((at.1.min(d.at.1) >> 4) - far), bottom.max((at.1.max(d.at.1) >> 4) + far));
            }
            d.at = at;
        }
        if first {
            (top, bottom) = (0, height - 1);
        }
        let mut line = std::mem::take(&mut self.line);
        let mut spans: Vec<(i32, i32)> = Vec::with_capacity(2 * moved.len() + 1);
        for y in top.max(0)..=bottom.min(height - 1) {
            // The stretches of this row to draw again, in order, none
            // touching the next.
            spans.clear();
            if first {
                spans.push((0, width - 1));
            } else {
                for &(i, at, by, ring) in &moved {
                    let Some((one, two)) = self.discs[i].changed(at, by, ring, y) else { continue };
                    spans.push(one);
                    spans.extend(two);
                }
                spans.retain_mut(|s| {
                    *s = (s.0.max(0), s.1.min(width - 1));
                    s.0 <= s.1
                });
                if spans.is_empty() {
                    continue;
                }
                spans.sort_unstable();
                let mut merged = 0;
                for i in 1..spans.len() {
                    if spans[i].0 <= spans[merged].1 + 1 {
                        spans[merged].1 = spans[merged].1.max(spans[i].1);
                    } else {
                        merged += 1;
                        spans[merged] = spans[i];
                    }
                }
                spans.truncate(merged + 1);
            }
            let row = y as usize * w;
            for &(x0, x1) in &spans {
                line[x0 as usize..=x1 as usize].copy_from_slice(&base[row + x0 as usize..=row + x1 as usize]);
            }
            // Every circle on this row, in order, where the row is drawn
            // again.
            let yc = y * 16 + 8;
            for d in &self.discs {
                let Some((d0, d1)) = d.row(d.at.0 >> 4, d.at.1 >> 4, y) else { continue };
                let dy = yc - d.at.1;
                let dy2 = dy * dy;
                for &(s0, s1) in &spans {
                    let (x0, x1) = (s0.max(d0), s1.min(d1));
                    if x0 > x1 {
                        continue;
                    }
                    let mut dx = x0 * 16 + 8 - d.at.0;
                    for p in &mut line[x0 as usize..=x1 as usize] {
                        if let Some(&t) = d.opacity.get(((dx * dx + dy2) >> d.shift) as usize)
                            && t > 0
                        {
                            *p = blend(*p, d.color, i32::from(t));
                        }
                        dx += 16;
                    }
                }
            }
            for &(x0, x1) in &spans {
                let mut noise = noise_at(x0, y);
                for x in x0 as usize..=x1 as usize {
                    out[row + x] = shown(line[x], noise, dither);
                    noise = noise.wrapping_add(NOISE_STEP);
                }
                run(changed, y as usize, x0 as usize, x1 as usize);
            }
        }
        self.line = line;
    }
}

// ------------------------------------------------------------------- flow

/// A flow whose waves run on ([`Flow::Ribbons`]).
///
/// Every pixel of a flow changes as it runs, and the phone draws a pixel
/// in about a tenth of a microsecond: the whole picture ten times a second
/// would take a quarter of its processor.  So:
///
/// - The rows take turns.  A frame draws every other row, the even rows
///   and the odd ones alternately ([`FIELDS`]), so a row shows a time up
///   to a frame before the rows next to it.  The colors move by a
///   fraction of a pixel in a frame, and run from one to the next over a
///   dozen pixels or more: rows a frame apart cannot be told apart.
/// - The position along the stops is worked out for one pixel in [`KNOT`]
///   across each row, and runs straight from one such pixel to the next:
///   the waves are hundreds of pixels long, so over eight pixels the
///   position leaves the straight line by less than a fifth of a pixel of
///   the picture.
///
/// A frame moves the colors by a fraction of a pixel, which is a few
/// positions of the 4096 along the stops: in whole positions the motion
/// would go in uneven steps (two positions in one frame, four in the
/// next).  So the positions here are in sixteenths ([`FINE`]), and a
/// pixel takes the color of the position below or the one above by its
/// place on the screen, as the dither has it for the colors.
struct Stream {
    ax: i32,
    ay: i32,
    /// The screen's size, and `w * ax + h * ay`: the straight run's length.
    w: i32,
    h: i32,
    whole: i32,
    /// The knots of a row: one in [`KNOT`] pixels, and one past the end.
    knot_count: usize,
    /// The time from one frame to the next, in milliseconds.
    every: u64,
    /// The waves (their lengths in pixels of the screen), with the turns
    /// each makes in a cycle.
    waves: Vec<((i32, i32, i32, i32), i32)>,
    /// The straight run by `x * ax + y * ay`, in [`FINE`] parts of a
    /// position.
    straight: Vec<i32>,
    /// The color (packed, within the limits) at each position along the
    /// stops.
    colors: Box<Colors>,
    /// For each wave, its turn at each knot.
    across: Vec<i32>,
    /// For each wave that varies both ways: what it adds, by its turn in
    /// 1/4096 turn, in [`FINE`] parts of a position.
    swing: Vec<Vec<i16>>,
    /// The positions at the knots of the row being drawn, in [`FINE`]
    /// parts of a position.
    knots: Vec<i32>,
    /// For each row, the time it shows in `out` (`u64::MAX`: none yet).
    shows: Vec<u64>,
}

/// The colors of a flow, at each position along its stops.
type Colors = [u32; ONE as usize + 1];

/// From one pixel whose position is worked out to the next.
const KNOT: usize = 8;
/// The most knots [`paint_row`] paints at once (a bit each).
const KNOTS_PAINTED: usize = 64;
/// The parts of a position along the stops that the flow is worked out
/// in.
const FINE: i32 = 16;
/// How far below the position worked out here the still picture's is, on
/// average, in [`FINE`] parts: [`Flow::at`] adds four terms in whole
/// positions, each rounded down (half a position each), where the terms
/// here are rounded down in sixteenths and the pixel's place adds half a
/// position.  Taken off, so that the flow begins as the still picture.
const LOW: i32 = 2 * FINE + 6;
/// The rows of a flow are drawn in this many turns: row `y` with the
/// frames whose number is `y` modulo this.
const FIELDS: u64 = 2;

impl Stream {
    /// `None` for a screen whose width is not a whole number of knots.
    fn new(ax: i32, ay: i32, waves: &'static [(i32, i32, i32, i32)], turns: &'static [i32], stops: Stops, every: u64, fit: Fit) -> Option<Stream> {
        if fit.w as usize % KNOT != 0 {
            return None;
        }
        let whole = fit.w * ax + fit.h * ay;
        let knot_count = fit.w as usize / KNOT + 1;
        let waves: Vec<((i32, i32, i32, i32), i32)> = waves
            .iter()
            .enumerate()
            .map(|(i, &(amount, px, py, phase))| ((amount, fit.size(px), fit.size(py), phase), turns.get(i).copied().unwrap_or(0)))
            .collect();
        let mut across = Vec::with_capacity(waves.len() * knot_count);
        let mut swing = Vec::new();
        for &((amount, px, py, _), _) in &waves {
            across.extend((0..knot_count as i32).map(|k| if px != 0 { k * KNOT as i32 * 65536 / px } else { 0 }));
            swing.push(if px != 0 && py != 0 { (0..4096).map(|k| ((amount * FINE * sin(k * 16 + 8)) >> 12) as i16).collect() } else { Vec::new() });
        }
        Some(Stream {
            ax,
            ay,
            w: fit.w,
            h: fit.h,
            whole,
            knot_count,
            every,
            waves,
            straight: (0..=whole).map(|v| v * ONE * FINE / whole).collect(),
            colors: Box::new(std::array::from_fn(|t| pack(limit(stops_at(stops, t as i32))))),
            across,
            swing,
            knots: vec![0; knot_count],
            shows: vec![u64::MAX; fit.h as usize],
        })
    }

    /// Every pixel as [`Flow::at`] has it, with each wave moved on by the
    /// time: the time of the last frame in which the pixel's row had its
    /// turn.
    fn frame(&mut self, t_ms: u64, out: &mut [u32], dither: bool, changed: &mut Vec<Run>) {
        let w = self.w as usize;
        let knots = self.knot_count;
        // The time each of the rows' turns shows: turn `n` is drawn every
        // `FIELDS` frames, `n` frames after the first.
        let t = t_ms % CYCLE_MS + CYCLE_MS;
        let times: [u64; FIELDS as usize] = std::array::from_fn(|n| (t - (t - n as u64 * self.every) % (FIELDS * self.every)) % CYCLE_MS);
        let table = if dither { &NOISE } else { &FLAT };
        // Of the waves that vary both ways: the table, the turns at the
        // knots, and the turn at the start of the row.
        let mut bent: Vec<(&[i16], &[i32], i32)> = Vec::new();
        // What the waves that do not vary down the screen add at each
        // knot, at each of the times, when a row first needs it.
        let mut columns: [Vec<i32>; FIELDS as usize] = std::array::from_fn(|_| Vec::new());
        for y in 0..self.h {
            let turn_of_rows = y as usize % FIELDS as usize;
            let time = times[turn_of_rows];
            if std::mem::replace(&mut self.shows[y as usize], time) == time {
                continue;
            }
            // What the waves that do not vary across the screen add in
            // this row.
            let mut rows = 0;
            bent.clear();
            let columns = &mut columns[turn_of_rows];
            let first = columns.is_empty();
            if first {
                columns.resize(knots, 0);
            }
            for (i, &((amount, px, py, phase), turns)) in self.waves.iter().enumerate() {
                let turn = if py != 0 { y * 65536 / py } else { 0 } + phase * 256 + turn_at(time, turns);
                let across = &self.across[i * knots..(i + 1) * knots];
                if px == 0 {
                    rows += (amount * FINE * sin(turn)) >> 12;
                } else if py == 0 {
                    if first {
                        for (c, a) in columns.iter_mut().zip(across) {
                            *c += (amount * FINE * sin(a + turn)) >> 12;
                        }
                    }
                } else {
                    bent.push((&self.swing[i], across, turn));
                }
            }
            let at = (y * self.ay) as usize;
            for (k, knot) in self.knots.iter_mut().enumerate() {
                let mut t = self.straight[at + k * KNOT * self.ax as usize] + rows + columns[k];
                for &(table, across, turn) in &bent {
                    t += i32::from(table[(((across[k] + turn) >> 4) & 4095) as usize]);
                }
                *knot = (t - LOW).clamp(0, ONE * FINE);
            }
            let row = y as usize * w;
            // As many knots at a time as `paint_row` paints (on the color
            // skin's screen, the whole row).
            for start in (0..knots - 1).step_by(KNOTS_PAINTED) {
                let end = (start + KNOTS_PAINTED).min(knots - 1);
                let (x0, x1) = (start * KNOT, end * KNOT);
                let mut stretches =
                    paint_row(&mut out[row + x0..row + x1], &self.knots[start..=end], &self.colors, table, noise_at(x0 as i32, y));
                // The pixels that changed, in runs of stretches.  (Most
                // pixels are what they were when the row was last drawn:
                // the colors move slowly, and there are stretches of one
                // color.)
                while stretches != 0 {
                    let first = stretches.trailing_zeros();
                    let count = (!(stretches >> first)).trailing_zeros();
                    run(changed, y as usize, x0 + first as usize * KNOT, x0 + (first + count) as usize * KNOT - 1);
                    stretches &= !((u64::MAX >> (64 - count)) << first);
                }
            }
        }
    }
}

/// Paints a row of a flow from the positions at its knots (in [`FINE`]
/// parts of a position), with the dither `table` ([`NOISE`] or [`FLAT`])
/// and the noise of its first pixel: as [`shown`] has it.  Returns the
/// stretches of [`KNOT`] pixels in which a pixel changed, a bit each.
///
/// The loop is most of what a frame of a flow costs.  It is kept apart
/// from [`Stream::frame`], whose other values would take the processor's
/// registers from it, and it is given the dither's table, chosen once a
/// frame, where [`shown`] chooses for each pixel: together a fifth fewer
/// instructions a frame (counted under `qemu-arm` for an ARM1176).
#[inline(never)]
fn paint_row(out: &mut [u32], knots: &[i32], colors: &Colors, table: &[u32; 16], mut noise: u32) -> u64 {
    debug_assert!(out.len() / KNOT <= KNOTS_PAINTED);
    let mut changed = 0;
    for (k, (o, ends)) in out.as_chunks_mut::<KNOT>().0.iter_mut().zip(knots.windows(2)).enumerate() {
        // In 1/256 of a position.
        let mut t = ends[0] * (256 / FINE);
        let step = (ends[1] - ends[0]) * (256 / FINE) / KNOT as i32;
        let mut differ = 0;
        for o in o {
            // The position below or the one above: by other bits of the
            // noise than the dither of the color has.  (Never beyond the
            // last: the positions are within the stops.)
            let at = ((t + ((noise >> 4) & 255) as i32) >> 8) as u32;
            let p = to_frame_buffer(colors[at.min(ONE as u32) as usize].wrapping_add(table[((noise >> 12) & 15) as usize]));
            differ |= p ^ *o;
            *o = p;
            t += step;
            noise = noise.wrapping_add(NOISE_STEP);
        }
        changed |= u64::from(differ != 0) << k;
    }
    changed
}
