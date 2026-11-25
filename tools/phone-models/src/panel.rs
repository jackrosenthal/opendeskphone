//! The front panels of the D40, D50, D60, D65, D70, and D80, as data.
//!
//! The layouts are those of `web/src/lib/phones.ts` (the drawing the web
//! interface's Screen page uses, corrected against front photographs).
//! Two families:
//!
//! - New (D60, D65): `newFamily()` in phones.ts.  Units are that drawing's
//!   units: the display is 303 units wide and 95.2 mm wide (a 4.3-inch
//!   480x272 panel), so one unit is 0.3142 mm.  The D65's paging control is
//!   one rocker as wide as the two columns of line keys (phones.ts draws
//!   two keys).
//! - Old (D40, D50, D70): `oldFamily()` in phones.ts.  The drawing has no
//!   physical scale; one unit is taken as 0.36 mm, which gives the D40, D50,
//!   and D70 the datasheet's widths (8.25, 10.5, and 12.5 inches).
//! - Touch (D80): measured from a retailer's front photograph, whose pixels
//!   are the units.  One unit is 0.229 mm, which gives the datasheet's
//!   width and height (22.5 and 20.8 cm).  The D80 has no keys.

pub type P2 = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelId {
    D40,
    D50,
    D60,
    D65,
    D70,
    D80,
}

impl ModelId {
    pub const ALL: [ModelId; 6] = [ModelId::D40, ModelId::D50, ModelId::D60, ModelId::D65, ModelId::D70, ModelId::D80];

    pub fn name(self) -> &'static str {
        match self {
            ModelId::D40 => "D40",
            ModelId::D50 => "D50",
            ModelId::D60 => "D60",
            ModelId::D65 => "D65",
            ModelId::D70 => "D70",
            ModelId::D80 => "D80",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    Silver,
    Black,
    Teal,
    Housing,
    OldSilver,
}

#[derive(Clone, Debug)]
pub enum Legend {
    Icon(&'static str),
    Text(&'static str),
    /// A digit, and the row of letters above the digit: `Some("")` for the
    /// 1 key (a row with nothing in the row), `None` for no row.
    Digit(&'static str, Option<&'static str>),
    /// An icon toward each end of a wide key (the page rocker).
    Ends(&'static str, &'static str),
}

#[derive(Clone, Debug)]
pub struct Key {
    pub id: String,
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
    pub r: f64,
    pub fill: Fill,
    pub ink: &'static str,
    pub legend: Option<Legend>,
    pub size: Option<f64>,
    /// Drawn as part of a bar (the volume keys, the navigation keys): no
    /// key of the key's own.
    pub bare: bool,
    /// The top and bottom edges bow outward by this much at the middle (the
    /// dial keys).
    pub arch: f64,
}

pub struct NewParts {
    /// The two panels' bounding box; the panels meet at `split`.
    pub face: [f64; 4],
    pub split: f64,
    pub mwi: [f64; 4],
    pub nav_disc: [f64; 3],
}

pub struct OldParts {
    pub body: Vec<P2>,
    pub body_top: f64,
    pub body_bottom: f64,
    pub housing: Vec<P2>,
    pub housing_box: [f64; 4],
    pub bezel: [f64; 5],
    pub side_frame: Option<[f64; 5]>,
    pub paper: Vec<([f64; 4], &'static str)>,
    pub hook: [f64; 5],
    pub ring: [f64; 3],
    pub mwi: Vec<P2>,
}

pub struct TouchParts {
    /// The body's box (x0, y0, x1, y1) and the corners' radius.
    pub face: [f64; 5],
    /// The glass's inset from the body's edge.
    pub glass_inset: f64,
    /// The cradle's outline: left of the body, and under the body's left
    /// edge.
    pub cradle: Vec<P2>,
    /// The speaker's slots in the cradle: x, y, w, h.
    pub grille: Vec<[f64; 4]>,
    pub mwi: [f64; 4],
}

pub enum Parts {
    New(NewParts),
    Old(OldParts),
    Touch(TouchParts),
}

pub struct Panel {
    pub parts: Parts,
    pub unit_mm: f64,
    pub center: P2,
    pub keys: Vec<Key>,
    pub display: [f64; 4],
    pub side: Option<[f64; 4]>,
    /// The cradle's box: x0, y0, x1, y1.
    pub wing: [f64; 4],
    /// The cradle's cups: box, and the corners' radii (top left, top right,
    /// bottom right, bottom left).
    pub cups: Vec<([f64; 4], [f64; 4])>,
    pub handset: [f64; 4],
    /// The joined bars (x, y, w, h, r) and the lines across them.
    pub bars: Vec<[f64; 5]>,
    pub bar_splits: Vec<[f64; 3]>,
    /// Printed lines: x0, y0, x1, y1.
    pub lines: Vec<[f64; 4]>,
}

impl Panel {
    /// Black, on the tent stand: the new family, and the D80.
    pub fn is_new(&self) -> bool {
        matches!(self.parts, Parts::New(_) | Parts::Touch(_))
    }

    /// The box the legends cover (units): the whole outline of the phone's
    /// face.
    pub fn extent(&self) -> [f64; 4] {
        match &self.parts {
            Parts::New(n) => n.face,
            Parts::Touch(t) => [t.face[0], t.face[1], t.face[2], t.face[3]],
            Parts::Old(o) => {
                let (lo, hi) = x_range(&o.body);
                [lo, o.housing_box[1], hi, o.body_bottom]
            }
        }
    }
}

pub fn x_range(pts: &[P2]) -> (f64, f64) {
    pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)))
}

pub const WHITE: &str = "#f2f3f4";
const KEY_INK: &str = "#69727d";
const DIGITS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#"];

fn letters(d: &str) -> Option<&'static str> {
    Some(match d {
        "1" => "",
        "2" => "ABC",
        "3" => "DEF",
        "4" => "GHI",
        "5" => "JKL",
        "6" => "MNO",
        "7" => "PQRS",
        "8" => "TUV",
        "9" => "WXYZ",
        _ => return None,
    })
}

fn word(id: &str) -> &'static str {
    match id {
        "msgs" => "Msgs",
        "xfer" => "Transfer",
        "redial" => "Redial",
        "hold" => "Hold",
        "conf" => "Conf",
        "info" => "Info",
        _ => unreachable!("{id}"),
    }
}

/// The star is drawn, centered; the 1 has a row for letters with none in
/// the row, so that the 1 stands where the other digits stand; the pound
/// sign, and the new family's 0, have no row and are centered.  The old
/// family prints OPER above the 0.
fn dial_legend(d: &'static str, old: bool) -> Legend {
    if d == "*" {
        return Legend::Icon("asterisk");
    }
    Legend::Digit(d, if old && d == "0" { Some("OPER") } else { letters(d) })
}

#[allow(clippy::too_many_arguments)]
fn key<'a>(out: &'a mut Vec<Key>, id: &str, cx: f64, cy: f64, w: f64, h: f64, r: f64, fill: Fill) -> &'a mut Key {
    out.push(Key {
        id: id.into(),
        cx,
        cy,
        w,
        h,
        r,
        fill,
        ink: WHITE,
        legend: None,
        size: None,
        bare: false,
        arch: 0.0,
    });
    out.last_mut().unwrap()
}

impl Key {
    fn legend(&mut self, ink: &'static str, legend: Legend, size: f64) -> &mut Key {
        self.ink = ink;
        self.legend = Some(legend);
        self.size = Some(size);
        self
    }

    fn bare(&mut self) -> &mut Key {
        self.bare = true;
        self
    }

    fn arch(&mut self, a: f64) -> &mut Key {
        self.arch = a;
        self
    }
}

/// Points of a quadratic Bezier curve, without the first.
fn quad(p0: P2, c: P2, p1: P2, n: usize) -> Vec<P2> {
    (1..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let (a, b, d) = ((1.0 - t) * (1.0 - t), 2.0 * (1.0 - t) * t, t * t);
            (a * p0.0 + b * c.0 + d * p1.0, a * p0.1 + b * c.1 + d * p1.1)
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Seg {
    L(P2),
    Q(P2, P2),
}

/// An outline from a start point and segments.
fn path(start: P2, segs: &[Seg]) -> Vec<P2> {
    let mut pts = vec![start];
    for s in segs {
        match *s {
            Seg::L(p) => pts.push(p),
            Seg::Q(c, p) => {
                let last = *pts.last().unwrap();
                pts.extend(quad(last, c, p, 16));
            }
        }
    }
    let (first, last) = (pts[0], *pts.last().unwrap());
    if (last.0 - first.0).abs() < 1e-6 && (last.1 - first.1).abs() < 1e-6 {
        pts.pop();
    }
    pts
}

// ------------------------------------------------------------ new family

/// The new family's face (x0, y0, x1, y1), and one unit in millimeters.
pub const NEW_FACE: [f64; 4] = [268.0, 112.0, 758.0, 715.0];
pub const NEW_UNIT_MM: f64 = 95.2 / 303.0;

fn new_family(line_keys: usize, rocker: bool) -> Panel {
    let mut keys = Vec::new();
    let line_pos = [(655.0, 168.0), (697.0, 190.0), (655.0, 215.0), (697.0, 240.0), (655.0, 265.0), (697.0, 289.0)];
    for (i, &(cx, cy)) in line_pos[..line_keys].iter().enumerate() {
        key(&mut keys, &format!("line{}", i + 1), cx, cy, 30.0, 16.0, 8.0, Fill::Silver);
    }
    if rocker {
        // The page rocker: one key as wide as the two columns of line keys
        // together, with an arrow at each end.
        let left = line_pos.iter().map(|p| p.0).fold(f64::MAX, f64::min) - 15.0;
        let right = line_pos.iter().map(|p| p.0).fold(f64::MIN, f64::max) + 15.0;
        key(&mut keys, "page", (left + right) / 2.0, 327.0, right - left, 16.0, 6.0, Fill::Silver)
            .legend(KEY_INK, Legend::Ends("page-left", "page-right"), 11.0)
            .bare();
    }
    let soft_x = [355.0, 427.0, 500.0, 573.0];
    for (i, &cx) in soft_x.iter().enumerate() {
        key(&mut keys, &format!("sk{}", i + 1), cx, 378.0, 42.0, 15.0, 7.5, Fill::Silver);
    }
    for (id, icon, cy) in [("hdst", "headset", 462.0), ("spkr", "speaker", 514.0), ("mute", "mute", 566.0)] {
        key(&mut keys, id, 327.0, cy, 46.0, 22.0, 11.0, Fill::Silver).legend(KEY_INK, Legend::Icon(icon), 18.0);
    }
    for (i, d) in DIGITS.iter().enumerate() {
        let (cx, cy) = (409.0 + (i % 3) as f64 * 59.0, 462.0 + (i / 3) as f64 * 51.5);
        key(&mut keys, d, cx, cy, 46.0, 35.0, 3.0, Fill::Black).legend(WHITE, dial_legend(d, false), 15.5).arch(1.9);
    }
    key(&mut keys, "vdn", 438.0, 670.0, 62.0, 20.0, 3.0, Fill::Black).legend(WHITE, Legend::Icon("minus"), 14.0).bare();
    key(&mut keys, "vup", 500.0, 670.0, 62.0, 20.0, 3.0, Fill::Black).legend(WHITE, Legend::Icon("plus"), 14.0).bare();
    key(&mut keys, "select", 623.0, 446.0, 46.0, 20.0, 3.0, Fill::Black).legend(WHITE, Legend::Icon("check"), 13.0).bare();
    key(&mut keys, "cancel", 669.0, 446.0, 46.0, 20.0, 3.0, Fill::Black).legend(WHITE, Legend::Icon("x"), 12.0).bare();
    for (id, cx, cy) in [("up", 648.0, 490.0), ("left", 624.0, 514.0), ("right", 672.0, 514.0), ("down", 648.0, 538.0)] {
        key(&mut keys, id, cx, cy, 26.0, 26.0, 13.0, Fill::Black).legend(WHITE, Legend::Icon(icon_name(id)), 14.0).bare();
    }
    for (id, cx, cy) in [("msgs", 610.0, 590.0), ("xfer", 688.0, 590.0), ("redial", 610.0, 637.0), ("hold", 688.0, 637.0)] {
        key(&mut keys, id, cx, cy, 56.0, 25.0, 12.5, Fill::Black).legend(WHITE, Legend::Icon(icon_name(id)), 17.0);
    }
    Panel {
        parts: Parts::New(NewParts {
            face: NEW_FACE,
            split: 411.0,
            mwi: [678.0, 112.0, 28.0, 6.0],
            nav_disc: [648.0, 514.0, 37.0],
        }),
        unit_mm: NEW_UNIT_MM,
        center: (410.0, 410.0),
        keys,
        display: [312.0, 155.0, 303.0, 303.0 * 272.0 / 480.0],
        side: None,
        wing: [62.0, 170.0, 276.0, 700.0],
        cups: vec![([92.0, 172.0, 142.0, 92.0], [4.0, 4.0, 38.0, 38.0]), ([98.0, 568.0, 132.0, 132.0], [66.0, 66.0, 4.0, 4.0])],
        handset: [50.0, 63.0, 174.0, 669.0],
        bars: vec![[407.0, 660.0, 124.0, 20.0, 10.0], [600.0, 436.0, 92.0, 20.0, 10.0]],
        bar_splits: vec![[469.0, 662.0, 678.0], [646.0, 438.0, 454.0]],
        lines: soft_x.iter().map(|&cx| [cx, 334.0, cx, 366.0]).collect(),
    }
}

/// The icon of a key whose icon has the key's own name.
fn icon_name(id: &str) -> &'static str {
    match id {
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        "msgs" => "msgs",
        "xfer" => "xfer",
        "redial" => "redial",
        "hold" => "hold",
        "conf" => "conf",
        "info" => "info",
        "apps" => "apps",
        "dir" => "dir",
        "menu" => "menu",
        "status" => "status",
        _ => unreachable!("{id}"),
    }
}

// ------------------------------------------------------------ old family

struct Body {
    xl: f64,
    top: f64,
    xr: f64,
    yb: f64,
    bow: f64,
    wing_rise: f64,
    wing_drop: f64,
}

struct Housing {
    hr: f64,
    lift: f64,
    arch: f64,
    y1: f64,
    yc: f64,
}

struct Nav {
    x: f64,
    y: f64,
    step: P2,
    cancel: P2,
    size: f64,
    ring: f64,
}

struct WingLayout {
    dx: f64,
    y: f64,
    w: f64,
    key_x: f64,
    key: P2,
    page_y: f64,
    page: P2,
    page_x: P2,
}

/// Where the parts of each body are, from the front photographs (as in
/// phones.ts, which says what each field is).
struct Layout {
    handset: [f64; 4],
    cradle: P2,
    body: Body,
    housing: Housing,
    display: [f64; 4],
    bezel: [f64; 4],
    soft: (f64, f64, f64),
    line_x: P2,
    line_y: Option<P2>,
    side: (f64, f64, f64, f64, f64),
    dial: (f64, f64, f64, f64, f64, f64),
    volume: (f64, f64, f64),
    fx: Vec<f64>,
    top: Option<(f64, f64, f64)>,
    nav: Nav,
    beside: Option<P2>,
    feature: (f64, f64, f64, f64),
    wing: Option<WingLayout>,
}

const D40_DISPLAY: [f64; 4] = [340.0, 200.0, 196.0, 98.0];

fn layout(model: ModelId) -> Layout {
    match model {
        ModelId::D40 => Layout {
            handset: [106.0, 96.0, 152.0, 512.0],
            cradle: (232.0, 598.0),
            body: Body { xl: 284.0, top: 212.0, xr: 680.0, yb: 598.0, bow: 16.0, wing_rise: 14.0, wing_drop: 0.0 },
            housing: Housing { hr: 664.0, lift: 0.0, arch: 6.0, y1: 348.0, yc: 378.0 },
            display: D40_DISPLAY,
            bezel: [326.0, 188.0, 224.0, 122.0],
            soft: (338.0, 36.0, 15.0),
            line_x: (577.0, 610.0),
            line_y: None,
            side: (334.0, 402.0, 50.0, 37.0, 28.0),
            dial: (394.0, 402.0, 47.0, 40.0, 37.0, 29.0),
            volume: (442.0, 566.0, 112.0),
            fx: vec![568.0, 636.0],
            top: None,
            nav: Nav { x: 590.0, y: 430.0, step: (32.0, 30.0), cancel: (34.0, -32.0), size: 23.0, ring: 40.0 },
            beside: None,
            feature: (503.0, 40.0, 48.0, 24.0),
            wing: None,
        },
        ModelId::D50 => Layout {
            handset: [106.0, 96.0, 152.0, 512.0],
            cradle: (232.0, 598.0),
            body: Body { xl: 279.0, top: 212.0, xr: 853.0, yb: 598.0, bow: 16.0, wing_rise: 14.0, wing_drop: 0.0 },
            housing: Housing { hr: 664.0, lift: 0.0, arch: 6.0, y1: 348.0, yc: 378.0 },
            display: D40_DISPLAY,
            bezel: [326.0, 188.0, 224.0, 122.0],
            soft: (338.0, 36.0, 15.0),
            line_x: (577.0, 610.0),
            line_y: None,
            side: (313.0, 402.0, 50.0, 37.0, 28.0),
            dial: (373.0, 402.0, 47.0, 40.0, 37.0, 29.0),
            volume: (421.0, 566.0, 112.0),
            fx: vec![528.0, 589.0, 650.0],
            top: None,
            nav: Nav { x: 589.0, y: 430.0, step: (32.0, 30.0), cancel: (34.0, -32.0), size: 23.0, ring: 40.0 },
            beside: None,
            feature: (503.0, 40.0, 44.0, 20.0),
            wing: None,
        },
        // From a front photograph of a D70 (one photograph pixel is 1.083 units).
        ModelId::D70 => Layout {
            handset: [124.5, 28.0, 145.0, 576.0],
            cradle: (127.0, 526.5),
            body: Body { xl: 284.0, top: 107.0, xr: 995.4, yb: 584.0, bow: 46.0, wing_rise: 0.0, wing_drop: 17.0 },
            housing: Housing { hr: 799.4, lift: 86.3, arch: 18.4, y1: 326.0, yc: 387.0 },
            display: [359.5, 139.0, 282.0, 141.0],
            bezel: [337.0, 115.0, 321.0, 179.0],
            soft: (312.6, 38.0, 17.0),
            line_x: (685.6, 720.3),
            line_y: Some((144.0, 20.4)),
            side: (351.4, 404.0, 56.0, 36.0, 26.0),
            dial: (425.0, 402.5, 47.5, 45.0, 35.0, 33.0),
            volume: (470.5, 586.0, 103.0),
            fx: vec![594.0, 658.0, 723.0],
            top: Some((384.6, 42.0, 22.0)),
            nav: Nav { x: 626.0, y: 460.5, step: (35.0, 35.0), cancel: (36.8, -35.7), size: 20.0, ring: 44.7 },
            beside: Some((723.0, 491.3)),
            feature: (531.0, 40.0, 42.0, 23.0),
            wing: Some(WingLayout {
                dx: 16.2,
                y: 210.7,
                w: 103.0,
                key_x: 150.6,
                key: (25.0, 13.0),
                page_y: 28.5,
                page: (36.0, 16.0),
                page_x: (0.24, 0.74),
            }),
        },
        _ => unreachable!(),
    }
}

/// The D50's label card: rows alternately white and solid light gray.
const PAPER_WHITE: &str = "#fbfbfc";
const PAPER_GRAY: &str = "#c4c8cd";

#[derive(Clone, Copy, PartialEq)]
enum Wing {
    Paper,
    Display,
}

fn old_family(
    model: ModelId,
    line_keys: usize,
    features: &[&[&'static str]],
    words: bool,
    top: Option<&[&'static str]>,
    beside: Option<&'static str>,
    wing: Option<Wing>,
) -> Panel {
    let l = layout(model);
    let mut keys = Vec::new();
    let mut lines = Vec::new();
    let (hr, lift) = (l.housing.hr, l.housing.lift);
    let [dx, _dy, dw, _dh] = l.display;
    let [bx, by, bw, bh] = l.bezel;
    let [hsx, hsy, hsw, hsh] = l.handset;

    // Line keys on the housing, right of the display, staggered; a printed
    // line runs from each to the display's bezel.
    let (y0, step) = l.line_y.unwrap_or((
        if line_keys > 2 { 204.0 } else { 214.0 },
        if line_keys > 4 {
            15.0
        } else if line_keys > 2 {
            19.0
        } else {
            17.0
        },
    ));
    for i in 0..line_keys {
        let cx = if i % 2 == 0 { l.line_x.0 } else { l.line_x.1 };
        let cy = y0 + i as f64 * step;
        key(&mut keys, &format!("line{}", i + 1), cx, cy, 27.0, 14.0, 7.0, Fill::Housing);
        lines.push([bx + bw, cy, cx - 13.5, cy]);
    }
    let (soft_y, soft_w, soft_h) = l.soft;
    for i in 0..4 {
        let cx = dx + dw * (i as f64 + 0.5) / 4.0;
        key(&mut keys, &format!("sk{}", i + 1), cx, soft_y, soft_w, soft_h, 5.0, Fill::Teal);
        lines.push([cx, by + bh - 2.0, cx, soft_y - soft_h / 2.0]);
    }
    let (sx, sy0, sstep, sw, sh) = l.side;
    for (i, (id, icon)) in [("hdst", "headset-turned"), ("spkr", "speaker-turned"), ("mute", "mute-turned")].into_iter().enumerate() {
        key(&mut keys, id, sx, sy0 + i as f64 * sstep, sw, sh, 3.5, Fill::OldSilver)
            .legend("#555b63", Legend::Icon(icon), 20.0)
            .arch(1.3);
    }
    let (dx0, dy0, ddx, ddy, kw, kh) = l.dial;
    for (i, d) in DIGITS.iter().enumerate() {
        let (cx, cy) = (dx0 + (i % 3) as f64 * ddx, dy0 + (i / 3) as f64 * ddy);
        key(&mut keys, d, cx, cy, kw, kh, 3.0, Fill::Black).legend(WHITE, dial_legend(d, true), 12.9).arch(1.4);
    }
    let (vx, vy, vw) = l.volume;
    key(&mut keys, "vdn", vx - vw / 4.0, vy, vw / 2.0, 16.0, 3.0, Fill::Black).legend(WHITE, Legend::Icon("minus"), 11.0).bare();
    key(&mut keys, "vup", vx + vw / 4.0, vy, vw / 2.0, 16.0, 3.0, Fill::Black).legend(WHITE, Legend::Icon("plus"), 11.0).bare();
    let (fxs, n) = (&l.fx, &l.nav);
    if let (Some(top), Some((ty, tw, th))) = (top, l.top) {
        for (i, id) in top.iter().enumerate() {
            key(&mut keys, id, fxs[i], ty, tw, th, 4.0, Fill::Black).legend(WHITE, Legend::Icon(icon_name(id)), 16.0);
        }
    }
    for (id, icon, cx, cy) in [
        ("up", "up", n.x, n.y - n.step.1),
        ("cancel", "x", n.x + n.cancel.0, n.y + n.cancel.1),
        ("left", "left", n.x - n.step.0, n.y),
        ("select", "check", n.x, n.y),
        ("right", "right", n.x + n.step.0, n.y),
        ("down", "down", n.x, n.y + n.step.1),
    ] {
        key(&mut keys, id, cx, cy, n.size, n.size, n.size / 2.0, Fill::Black).legend(WHITE, Legend::Icon(icon), 11.0 * n.size / 23.0);
    }
    if let (Some(beside), Some((bx_, by_)), Some((_, tw, th))) = (beside, l.beside, l.top) {
        key(&mut keys, beside, bx_, by_, tw, th, 4.0, Fill::Black).legend(WHITE, Legend::Icon(icon_name(beside)), 16.0);
    }
    let (fy0, fstep, fw, fh) = l.feature;
    for (r, row) in features.iter().enumerate() {
        for (i, id) in row.iter().enumerate() {
            let k = key(&mut keys, id, fxs[i], fy0 + r as f64 * fstep, fw, fh, 4.0, Fill::Black);
            if words {
                k.legend(WHITE, Legend::Text(word(id)), 9.5);
            } else {
                k.legend(WHITE, Legend::Icon(icon_name(id)), 16.0);
            }
        }
    }

    let hl = 300.0;
    let b = &l.body;
    let (xl, bt, xr, yb, bow) = (b.xl, b.top, b.xr, b.yb, b.bow);
    let (rise, drop) = (b.wing_rise, b.wing_drop);
    let cl = xl + 20.0;
    let cr = if wing.is_some() { hr } else { xr - 20.0 };
    let bottom_to = |xa: f64| -> [Seg; 2] {
        let t = (xa - cl) / (cr - cl);
        [Seg::Q(((cl + xa) / 2.0, yb + 2.0 * bow * t), (cl, yb)), Seg::Q((xl, yb), (xl, yb - 20.0))]
    };
    let body = if wing.is_some() {
        let mut segs = vec![
            Seg::L((hr, bt)),
            Seg::L((xr - 24.0, bt + drop)),
            Seg::Q((xr, bt + drop), (xr, bt + drop + 24.0)),
            Seg::L((xr, yb - rise - 24.0)),
            Seg::Q((xr, yb - rise - 2.0), (xr - 22.0, yb - rise)),
            Seg::L((hr, yb)),
        ];
        segs.extend(bottom_to(hr));
        segs.extend([Seg::L((xl, bt + 24.0)), Seg::Q((xl, bt), (xl + 24.0, bt))]);
        path((xl + 24.0, bt), &segs)
    } else {
        let mut segs = vec![Seg::L((xr - 24.0, bt)), Seg::Q((xr, bt), (xr, bt + 24.0)), Seg::L((xr, yb - 20.0)), Seg::Q((xr, yb), (cr, yb))];
        segs.extend(bottom_to(cr));
        segs.extend([Seg::L((xl, bt + 24.0)), Seg::Q((xl, bt), (xl + 24.0, bt))]);
        path((xl + 24.0, bt), &segs)
    };
    let mid = (hl + hr) / 2.0;
    // The housing: arched top edge, rounded corners, and a bottom edge that
    // bows downward between rounded corners.
    let h = &l.housing;
    let (rb, y1, yc, arch) = (20.0, h.y1, h.yc, h.arch);
    let yk = y1 - rb * (yc - y1) / (hr - rb - mid);
    let t = |v: f64| v - lift;
    let housing = path(
        (hl, t(182.0)),
        &[
            Seg::Q((hl, t(162.0)), (hl + 22.0, t(160.0))),
            Seg::Q((mid, t(160.0 - 2.0 * arch)), (hr - 22.0, t(160.0))),
            Seg::Q((hr, t(162.0)), (hr, t(182.0))),
            Seg::L((hr, yk - rb)),
            Seg::Q((hr, yk), (hr - rb, y1)),
            Seg::Q((mid, yc), (hl + rb, y1)),
            Seg::Q((hl, yk), (hl, yk - rb)),
        ],
    );
    // The message lamp is the housing's top right corner: a smoked lens
    // along the top edge and down the right edge.
    let mwi = path(
        (hr - 38.0, t(159.0)),
        &[Seg::L((hr - 22.0, t(160.0))), Seg::Q((hr, t(162.0)), (hr, t(182.0))), Seg::L((hr, t(188.0))), Seg::L((hr - 38.0, t(188.0)))],
    );
    let mut side = None;
    let mut side_frame = None;
    let mut paper = Vec::new();
    match wing {
        Some(Wing::Paper) => {
            // D50: ten label rows behind a narrow window, alternately white
            // and solid light gray, a small key beside each row.
            let (wx, row_h, wy) = (hr, 24.3, 268.0);
            side_frame = Some([wx + 20.0, wy - 3.0, 90.0, 10.0 * row_h + 6.0, 2.0]);
            for i in 0..10 {
                let i_ = i as f64;
                paper.push(([wx + 23.0, wy + i_ * row_h, 84.0, row_h], if i % 2 == 1 { PAPER_GRAY } else { PAPER_WHITE }));
                key(&mut keys, &format!("blf{}", i + 1), wx + 139.0, wy + (i_ + 0.5) * row_h, 27.0, 14.0, 5.5, Fill::Housing);
            }
        }
        Some(Wing::Display) => {
            // D70: the second display (ten entries and a page row), a key
            // beside each entry, and two teal page keys under the display.
            let g = l.wing.as_ref().unwrap();
            let wx = hr;
            let sw = g.w;
            let sh = sw * (357.0 / 113.0);
            let sy = g.y;
            side = Some([wx + g.dx, sy, sw, sh]);
            side_frame = Some([wx + g.dx - 4.0, sy - 4.0, sw + 8.0, sh + 8.0, 2.0]);
            for i in 0..10 {
                let cy = sy + (i as f64 + 0.5) * (sh / 11.0);
                key(&mut keys, &format!("blf{}", i + 1), wx + g.key_x, cy, g.key.0, g.key.1, g.key.1 / 2.0, Fill::Housing);
            }
            for (id, f) in [("blfleft", g.page_x.0), ("blfright", g.page_x.1)] {
                let cx = wx + g.dx + sw * f;
                key(&mut keys, id, cx, sy + sh + g.page_y, g.page.0, g.page.1, 5.0, Fill::Teal);
                lines.push([cx, sy + sh + 5.0, cx, sy + sh + g.page_y - g.page.1 / 2.0]);
            }
        }
        None => {}
    }
    let (ct, cb) = l.cradle;
    let cx0 = hsx - 8.0;
    let (lo, hi) = x_range(&body);
    let housing_top = housing.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    Panel {
        parts: Parts::Old(OldParts {
            body_top: bt,
            body_bottom: yb + bow,
            housing_box: [hl, housing_top - 6.0, hr, y1 + 15.0],
            body,
            housing,
            bezel: [bx, by, bw, bh, 10.0],
            side_frame,
            paper,
            hook: [cx0 + 76.0, ct + 64.0, 20.0, 9.0, 2.0],
            ring: [n.x, n.y, n.ring],
            mwi,
        }),
        unit_mm: 0.36,
        center: ((lo + hi) / 2.0, 410.0),
        keys,
        display: l.display,
        side,
        wing: [cx0, ct, cx0 + 200.0, cb],
        cups: vec![([cx0 + 18.0, ct + 12.0, 136.0, 72.0], [24.0; 4]), ([cx0 + 18.0, cb - 100.0, 136.0, 88.0], [28.0; 4])],
        handset: [hsx, hsy, hsw, hsh],
        bars: vec![[vx - vw / 2.0, vy - 8.0, vw, 16.0, 7.0]],
        bar_splits: vec![[vx, vy - 6.0, vy + 6.0]],
        lines,
    }
}

// ------------------------------------------------------------------- D80

/// The D80: a glass slab around an upright 7-inch 800x1280 display, and a
/// cradle at the slab's left that bulges out past the handset.
fn d80() -> Panel {
    const UNIT: f64 = 0.229;
    let diag = 7.0 * 25.4 / UNIT;
    let (dw, dh) = (diag * 800.0 / 800f64.hypot(1280.0), diag * 1280.0 / 800f64.hypot(1280.0));
    let cradle = path(
        (380.0, 200.0),
        &[
            Seg::L((100.0, 200.0)),
            Seg::Q((70.0, 200.0), (63.0, 290.0)),
            Seg::Q((20.0, 380.0), (8.0, 580.0)),
            Seg::Q((4.0, 690.0), (50.0, 800.0)),
            Seg::Q((78.0, 845.0), (130.0, 870.0)),
            Seg::Q((170.0, 905.0), (230.0, 910.0)),
            Seg::L((380.0, 910.0)),
        ],
    );
    let (lo, hi) = x_range(&cradle);
    Panel {
        parts: Parts::Touch(TouchParts {
            face: [354.0, 61.0, 990.0, 947.0, 29.0],
            glass_inset: 3.0,
            cradle,
            grille: [(52.0, 470.0, 50.0), (62.0, 450.0, 90.0), (72.0, 440.0, 110.0)].map(|(x, y, h)| [x, y, 5.0, h]).to_vec(),
            mwi: [862.0, 61.0, 38.0, 6.0],
        }),
        unit_mm: UNIT,
        center: (500.0, 500.0),
        keys: Vec::new(),
        display: [672.0 - dw / 2.0, 501.0 - dh / 2.0, dw, dh],
        side: None,
        wing: [lo, 200.0, hi, 910.0],
        cups: Vec::new(),
        handset: [82.0, 48.0, 223.0, 893.0],
        bars: Vec::new(),
        bar_splits: Vec::new(),
        lines: Vec::new(),
    }
}

const TWO: &[&[&str]] = &[&["msgs", "xfer"], &["redial", "hold"]];
const THREE: &[&[&str]] = &[&["msgs", "xfer", "conf"], &["redial", "hold", "info"]];

pub fn panel(model: ModelId) -> Panel {
    match model {
        ModelId::D40 => old_family(model, 2, TWO, true, None, None, None),
        ModelId::D50 => old_family(model, 4, THREE, true, None, None, Some(Wing::Paper)),
        ModelId::D60 => new_family(2, false),
        ModelId::D65 => new_family(6, true),
        ModelId::D70 => old_family(model, 6, THREE, false, Some(&["apps", "dir", "menu"]), Some("status"), Some(Wing::Display)),
        ModelId::D80 => d80(),
    }
}
