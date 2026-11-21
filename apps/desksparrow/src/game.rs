//! The game: a sparrow that falls, a key that makes it flap, and pipes
//! with gaps that come toward it.  Nothing here talks to the phone, and
//! there is no clock: the app calls [`Game::step`] once for every tick of
//! its timer, so the same seed and the same flaps on the same ticks give
//! the same game.

/// Heights and vertical speeds are in sixteenths of a pixel.
pub const SUB: i32 = 16;

/// The measures of the game on one skin.  Lengths are in pixels unless
/// stated; the field is the part of the screen above the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub width: i32,
    pub height: i32,
    pub bird_w: i32,
    pub bird_h: i32,
    /// The sparrow's left edge; it never moves sideways.
    pub bird_x: i32,
    /// The sparrow's edges are this much inside its picture when it is
    /// tested against a pipe: a wing tip that brushes a pipe is no crash.
    pub grace: i32,
    /// Downward speed gained each tick, in sixteenths.
    pub gravity: i32,
    /// Upward speed right after a flap, in sixteenths a tick.
    pub flap: i32,
    /// Fastest fall, in sixteenths a tick.
    pub max_fall: i32,
    /// How far the pipes come closer each tick.
    pub speed: i32,
    pub pipe_w: i32,
    /// Height of the opening between an upper and a lower pipe.
    pub gap: i32,
    /// From one pipe's left edge to the next one's.
    pub spacing: i32,
    /// Least distance of an opening from the top and from the floor.
    pub margin: i32,
    /// The most an opening is higher or lower than the one before.
    pub max_shift: i32,
    /// Length of a tick.
    pub tick_ms: u32,
}

impl Rules {
    /// 480x272: twenty ticks a second, which is as fast as an app's timer
    /// goes.
    pub const COLOR: Rules = Rules {
        width: 480,
        height: 264,
        bird_w: 32,
        bird_h: 24,
        bird_x: 96,
        grace: 3,
        gravity: 22,
        flap: 150,
        max_fall: 176,
        speed: 4,
        pipe_w: 44,
        gap: 96,
        spacing: 176,
        margin: 24,
        max_shift: 100,
        tick_ms: 50,
    };
    /// 320x160, gray.  The gray panels are slow to change and each repaint
    /// costs the processor the whole transfer, so the game runs at ten
    /// ticks a second there, with bigger steps.
    pub const LARGE_GRAY: Rules = Rules {
        width: 320,
        height: 156,
        bird_w: 16,
        bird_h: 12,
        bird_x: 60,
        grace: 1,
        gravity: 30,
        flap: 112,
        max_fall: 104,
        speed: 4,
        pipe_w: 24,
        gap: 64,
        spacing: 124,
        margin: 12,
        max_shift: 50,
        tick_ms: 100,
    };
    /// 240x120, gray.
    pub const SMALL_GRAY: Rules = Rules {
        width: 240,
        height: 116,
        bird_w: 16,
        bird_h: 12,
        bird_x: 44,
        grace: 1,
        gravity: 26,
        flap: 96,
        max_fall: 88,
        speed: 3,
        pipe_w: 20,
        gap: 50,
        spacing: 96,
        margin: 10,
        max_shift: 36,
        tick_ms: 100,
    };

    /// 800x1280, the touchscreen, upright: the color screens' game at
    /// twice the size, with the same time to cross from pipe to pipe, in a
    /// field that is much higher than it is wide.
    pub const TOUCH: Rules = Rules {
        width: 800,
        height: 1248,
        bird_w: 64,
        bird_h: 48,
        bird_x: 160,
        grace: 6,
        gravity: 44,
        flap: 300,
        max_fall: 352,
        speed: 8,
        pipe_w: 88,
        gap: 192,
        spacing: 352,
        margin: 48,
        max_shift: 200,
        tick_ms: 50,
    };

    /// The highest and lowest top edge an opening can have.
    pub fn gap_range(&self) -> (i32, i32) {
        (self.margin, self.height - self.margin - self.gap)
    }
}

/// Xorshift: small, fast, and the same on every machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        // The generator stays at zero once it is there.
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }
    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// A number from 0 to `n - 1`.
    pub fn below(&mut self, n: u32) -> u32 {
        // The high bits of xorshift are its better ones.
        ((u64::from(self.next()) * u64::from(n.max(1))) >> 32) as u32
    }
}

/// A pair of pipes: one from the top and one from the floor, with an
/// opening between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pipe {
    /// Left edge; it may be off either side of the field.
    pub x: i32,
    /// Top edge of the opening.
    pub gap_y: i32,
    /// The sparrow is past it (and it has been counted).
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub rules: Rules,
    rng: Rng,
    /// The sparrow's top edge, in sixteenths.
    pub y: i32,
    /// Its downward speed, in sixteenths a tick.
    pub vy: i32,
    /// Left to right.
    pub pipes: Vec<Pipe>,
    /// Openings flown through.
    pub score: u32,
    /// The sparrow hit a pipe or the floor.
    pub over: bool,
    /// Ticks played.
    pub ticks: u32,
    /// The tick of the last flap (for the wing).
    pub flapped_at: Option<u32>,
}

impl Game {
    /// A new game: the sparrow in the middle of the field, the first pipe
    /// just off the right edge.
    pub fn new(rules: Rules, seed: u32) -> Game {
        let mut g = Game {
            rules,
            rng: Rng::new(seed),
            y: (rules.height - rules.bird_h) / 2 * SUB,
            vy: 0,
            pipes: Vec::new(),
            score: 0,
            over: false,
            ticks: 0,
            flapped_at: None,
        };
        let (lo, hi) = rules.gap_range();
        // The first opening is level with the sparrow, give or take.
        let first = (rules.height - rules.gap) / 2;
        g.add_pipe(rules.width + rules.pipe_w, first.clamp(lo, hi));
        g.fill();
        g
    }

    /// The sparrow's top edge in pixels.
    pub fn bird_y(&self) -> i32 {
        self.y / SUB
    }

    fn add_pipe(&mut self, x: i32, near: i32) {
        let r = &self.rules;
        let (lo, hi) = r.gap_range();
        let (lo, hi) = (lo.max(near - r.max_shift), hi.min(near + r.max_shift));
        let gap_y = lo + self.rng.below((hi - lo + 1).max(1) as u32) as i32;
        self.pipes.push(Pipe {
            x,
            gap_y,
            passed: false,
        });
    }

    /// Keeps a pipe waiting beyond the right edge.
    fn fill(&mut self) {
        while let Some(last) = self.pipes.last().copied() {
            if last.x > self.rules.width {
                break;
            }
            self.add_pipe(last.x + self.rules.spacing, last.gap_y);
        }
    }

    /// The key: the sparrow goes up, whatever it was doing.
    pub fn flap(&mut self) {
        if !self.over {
            self.vy = -self.rules.flap;
            self.flapped_at = Some(self.ticks);
        }
    }

    /// Whether the sparrow touches a pipe.
    fn hits_pipe(&self) -> bool {
        let r = &self.rules;
        let (left, right) = (r.bird_x + r.grace, r.bird_x + r.bird_w - r.grace);
        let (top, bottom) = (self.bird_y() + r.grace, self.bird_y() + r.bird_h - r.grace);
        self.pipes.iter().any(|p| {
            left < p.x + r.pipe_w && right > p.x && (top < p.gap_y || bottom > p.gap_y + r.gap)
        })
    }

    /// One tick: the sparrow falls, the pipes come closer, and the game
    /// may end.
    pub fn step(&mut self) {
        if self.over {
            return;
        }
        let r = self.rules;
        self.ticks += 1;
        self.vy = (self.vy + r.gravity).min(r.max_fall);
        self.y += self.vy;
        // The top of the field is a ceiling, not an obstacle.
        if self.y < 0 {
            self.y = 0;
            self.vy = 0;
        }
        for p in &mut self.pipes {
            p.x -= r.speed;
            if !p.passed && p.x + r.pipe_w <= r.bird_x {
                p.passed = true;
                self.score += 1;
            }
        }
        self.pipes.retain(|p| p.x + r.pipe_w > 0);
        self.fill();
        let floor = (r.height - r.bird_h) * SUB;
        if self.y >= floor {
            self.y = floor;
            self.over = true;
        } else if self.hits_pipe() {
            self.over = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Rules; 4] = [Rules::COLOR, Rules::LARGE_GRAY, Rules::SMALL_GRAY, Rules::TOUCH];

    /// A player that flaps when the sparrow is about to sink below the
    /// next opening.
    fn autopilot(g: &Game) -> bool {
        let r = &g.rules;
        let next = g.pipes.iter().find(|p| p.x + r.pipe_w > r.bird_x).unwrap();
        let after = g.y + (g.vy + r.gravity).min(r.max_fall);
        after / SUB + r.bird_h > next.gap_y + r.gap - r.gap / 6
    }

    fn play(rules: Rules, seed: u32, ticks: u32) -> Game {
        let mut g = Game::new(rules, seed);
        for _ in 0..ticks {
            if autopilot(&g) {
                g.flap();
            }
            g.step();
        }
        g
    }

    #[test]
    fn the_sparrow_falls_faster_and_faster_to_a_limit() {
        for r in ALL {
            let mut g = Game::new(r, 1);
            let y0 = g.y;
            g.step();
            assert_eq!(g.vy, r.gravity);
            assert_eq!(g.y, y0 + r.gravity);
            g.step();
            assert_eq!(g.vy, 2 * r.gravity);
            assert_eq!(g.y, y0 + 3 * r.gravity);
            for _ in 0..3 {
                g.step();
            }
            assert!(g.vy <= r.max_fall);
            let mut g = Game::new(r, 1);
            g.vy = r.max_fall;
            g.step();
            assert_eq!(g.vy, r.max_fall);
        }
    }

    #[test]
    fn a_flap_sends_it_up_whatever_its_speed() {
        for r in ALL {
            let mut g = Game::new(r, 1);
            g.step();
            g.step();
            let y = g.y;
            g.flap();
            assert_eq!(g.vy, -r.flap);
            assert_eq!(g.flapped_at, Some(2));
            g.step();
            assert_eq!(g.y, y - r.flap + r.gravity);
            // It rises until gravity has used the flap up, by more than a
            // third of its own height and less than an opening.
            let mut top = g.y;
            while g.vy < 0 {
                g.step();
                top = top.min(g.y);
            }
            let rise = (y - top) / SUB;
            assert!(rise > r.bird_h / 3 && rise < r.gap - r.bird_h, "{rise}");
        }
    }

    #[test]
    fn the_top_of_the_field_is_a_ceiling() {
        for r in ALL {
            let mut g = Game::new(r, 1);
            // Flapping on every tick: up to the top and no farther.
            for _ in 0..40 {
                g.flap();
                g.step();
                assert!(g.y >= 0 && !g.over);
            }
            assert_eq!((g.y, g.vy), (0, 0));
        }
    }

    #[test]
    fn the_floor_ends_the_game() {
        for r in ALL {
            let mut g = Game::new(r, 1);
            let mut n = 0;
            while !g.over {
                g.step();
                n += 1;
                assert!(n < 100);
            }
            assert_eq!(g.bird_y(), r.height - r.bird_h);
            assert_eq!(g.score, 0);
            // Nothing moves after the end.
            let end = g.clone();
            g.flap();
            g.step();
            assert_eq!(g, end);
        }
    }

    #[test]
    fn a_pipe_ends_the_game_and_its_opening_does_not() {
        for r in ALL {
            let pipe = |gap_y| Pipe {
                x: r.bird_x + r.bird_w - r.grace + r.speed - 1,
                gap_y,
                passed: false,
            };
            let level = |gap_y| {
                let mut g = Game::new(r, 1);
                g.pipes = vec![pipe(gap_y)];
                // Hold the sparrow level for the tick.
                g.vy = -r.gravity;
                g.step();
                g
            };
            let y = Game::new(r, 1).bird_y();
            // The opening around the sparrow: no crash.
            assert!(!level(y - 4).over);
            // The opening below it: into the upper pipe.
            assert!(level(y + r.bird_h).over);
            // The opening above it: into the lower pipe.
            assert!(level(y - r.gap - 4).over);
            // One step short of the pipe: still flying.
            let mut g = Game::new(r, 1);
            g.pipes = vec![Pipe {
                x: pipe(0).x + 1,
                ..pipe(y + r.bird_h)
            }];
            g.vy = -r.gravity;
            g.step();
            assert!(!g.over);
            // The edges of the opening, to the pixel.
            let mut g = level(y + r.grace);
            assert!(!g.over);
            g.pipes[0].gap_y += 1;
            assert!(g.hits_pipe());
            let mut g = level(y + r.bird_h - r.grace - r.gap);
            assert!(!g.over);
            g.pipes[0].gap_y -= 1;
            assert!(g.hits_pipe());
        }
    }

    #[test]
    fn each_opening_passed_is_one_point() {
        for r in ALL {
            let mut g = Game::new(r, 7);
            let first = g.pipes[0].x;
            for tick in 1..=3000 {
                if autopilot(&g) {
                    g.flap();
                }
                g.step();
                assert!(!g.over, "the autopilot crashed at {} on {r:?}", g.score);
                // Pipe `i` started at `first + i * spacing`; it counts once
                // its right edge is no farther right than the sparrow's
                // left edge.
                let travelled = tick * r.speed - (first + r.pipe_w - r.bird_x);
                let behind = if travelled < 0 {
                    0
                } else {
                    travelled / r.spacing + 1
                };
                assert_eq!(g.score as i32, behind, "tick {tick}");
            }
            assert!(g.score > 20);
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_game() {
        for r in ALL {
            let a = play(r, 42, 500);
            let b = play(r, 42, 500);
            assert_eq!(a, b);
            let c = play(r, 43, 500);
            assert_ne!(
                a.pipes.iter().map(|p| p.gap_y).collect::<Vec<_>>(),
                c.pipes.iter().map(|p| p.gap_y).collect::<Vec<_>>()
            );
            // Seed zero is a seed like any other.
            assert_eq!(play(r, 0, 500), play(r, 0x9E37_79B9, 500));
        }
    }

    #[test]
    fn openings_stay_in_the_field_and_within_reach() {
        for r in ALL {
            let (lo, hi) = r.gap_range();
            assert!(lo < hi);
            for seed in 1..40 {
                let mut g = Game::new(r, seed);
                for _ in 0..1500 {
                    if autopilot(&g) {
                        g.flap();
                    }
                    g.step();
                    // One pipe waits beyond the right edge, and only one.
                    assert!(g.pipes.last().unwrap().x > r.width);
                    assert!(g.pipes.len() <= (r.width / r.spacing + 3) as usize);
                    for p in &g.pipes {
                        assert!((lo..=hi).contains(&p.gap_y), "{p:?}");
                    }
                    for w in g.pipes.windows(2) {
                        assert_eq!(w[1].x - w[0].x, r.spacing);
                        assert!((w[1].gap_y - w[0].gap_y).abs() <= r.max_shift);
                    }
                }
            }
        }
    }

    #[test]
    fn every_skin_can_be_played() {
        // A simple player gets through a hundred openings on every skin
        // with every seed tried: the measures leave room to fly.
        for r in ALL {
            for seed in 1..=25 {
                let ticks = (100 * r.spacing + r.width) / r.speed;
                let g = play(r, seed, ticks as u32);
                assert!(!g.over, "seed {seed} ended at {} on {r:?}", g.score);
                assert!(g.score >= 99);
            }
        }
    }
}
