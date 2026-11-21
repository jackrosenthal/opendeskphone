// The wallpapers themselves (part of `wallpaper.rs`).

const fn vertical(stops: Stops) -> Base {
    Base::Grad(Grad::Linear { ax: 0, ay: 1, stops })
}

const fn solid(id: &'static str, name: &'static str, color: Rgb, scheme: Scheme) -> Wallpaper {
    Wallpaper { id, name, family: Family::Solids, scheme, tweaks: &[], spec: Spec { base: Base::Grad(Grad::Solid(color)), layers: &[], dither: false, moves: None } }
}

/// The facets of the first wallpaper, leaning right and leaning left.
const FACETS_RIGHT: &[Band] = &[(60, 170, 120, 0), (330, 150, 88, 0), (560, 190, 128, 0)];
const FACETS_LEFT: &[Band] = &[(-120, 160, 96, 0), (130, 140, 72, 0), (300, 170, 112, 0)];

/// The first wallpaper (`harbor`) as a description, for the screens it is
/// not kept for pixel for pixel ([`Base::Original`]): its gradient, and its
/// facets lit by its brighter blue, as the other facets have theirs.
static ORIGINAL: Spec = Spec {
    base: vertical(&[(0, 0x151E2C), (21, 0x102444), (56, 0x0D203E), (100, 0x0A1426)]),
    layers: &[
        Layer::Bands { num: 3, den: 5, add: true, color: 0x061638, bands: FACETS_RIGHT },
        Layer::Bands { num: -3, den: 5, add: true, color: 0x061638, bands: FACETS_LEFT },
    ],
    dither: true,
    moves: None,
};

/// The first wallpaper in another hue: its gradient from the stops, its
/// facets lit by `light`.
macro_rules! facets {
    ($id:expr, $name:expr, $stops:expr, $light:expr, $colors:expr $(,)?) => {
        Wallpaper {
            id: $id,
            name: $name,
            family: Family::Facets,
            scheme: $colors,
            tweaks: &[],
            spec: Spec {
                base: vertical($stops),
                layers: &[
                    Layer::Bands { num: 3, den: 5, add: true, color: $light, bands: FACETS_RIGHT },
                    Layer::Bands { num: -3, den: 5, add: true, color: $light, bands: FACETS_LEFT },
                ],
                dither: true,
                moves: None,
            },
        }
    };
}

/// Pure red and pure yellow.
const KETCHUP_MUSTARD: (Rgb, Rgb) = (0xFF0000, 0xFFFF00);

/// Regatta's colors: vivid yellows, a navy, and red between them (the
/// flame is the red that dark text can still be read on).
const YELLOW: Rgb = 0xFFD21E;
const AMBER: Rgb = 0xFFB800;
const PALE: Rgb = 0xFFEC6A;
const FLAME: Rgb = 0xFF6A2A;
const RED: Rgb = 0xE5281E;
const NAVY: Rgb = 0x14235A;
const NAVY_DEEP: Rgb = 0x0B1640;
/// Its palette: navy menus with light text under a yellow title bar; the
/// selected row is yellow too, with the title's dark text.  The picture
/// is mostly yellow, so the text on it is dark as well.
const REGATTA: Scheme = Scheme::Dark { primary: 0x2A46A0, surface: 0x101C4A };
const REGATTA_ROLES: &[(&str, Rgb)] = &[
    ("title_bg", YELLOW),
    ("title_fg", 0x1B1E24),
    ("sel_bg", YELLOW),
    ("sel_fg", 0x1B1E24),
    ("accent", YELLOW),
    ("wallpaper_fg", 0x1B1E24),
];

static ALL: [Wallpaper; 42] = [
    // ------------------------------------------------------------- Facets
    Wallpaper {
        id: "harbor",
        name: "Harbor",
        family: Family::Facets,
        scheme: Scheme::Skin,
        tweaks: &[],
        spec: Spec { base: Base::Original, layers: &[], dither: false, moves: None },
    },
    facets!(
        "evergreen",
        "Evergreen",
        &[(0, 0x14221C), (21, 0x103426), (55, 0x0D2E22), (100, 0x0A1C16)],
        0x08301C,
        Scheme::Dark { primary: 0x1F6E4E, surface: 0x0F1E18 },
    ),
    facets!(
        "merlot",
        "Merlot",
        &[(0, 0x26151B), (21, 0x3C1020), (55, 0x360D1C), (100, 0x200A12)],
        0x380C18,
        Scheme::Dark { primary: 0x8E2440, surface: 0x211015 },
    ),
    facets!(
        "graphite",
        "Graphite",
        &[(0, 0x1E1F21), (21, 0x26282B), (55, 0x222427), (100, 0x151618)],
        0x1E2024,
        Scheme::Dark { primary: 0x4A5562, surface: 0x1E1F21 },
    ),
    facets!(
        "amethyst",
        "Amethyst",
        &[(0, 0x1E182C), (21, 0x261444), (55, 0x22123E), (100, 0x140C26)],
        0x241238,
        Scheme::Dark { primary: 0x5B3A9E, surface: 0x171123 },
    ),
    facets!(
        "ember",
        "Ember",
        &[(0, 0x2A1A14), (21, 0x402010), (55, 0x381C0E), (100, 0x22100A)],
        0x381806,
        Scheme::Dark { primary: 0x9A4A1C, surface: 0x21140E },
    ),
    facets!(
        "lagoon",
        "Lagoon",
        &[(0, 0x122226), (21, 0x0C343C), (55, 0x0A2E36), (100, 0x081C22)],
        0x063034,
        Scheme::Dark { primary: 0x1A6C78, surface: 0x0D1D20 },
    ),
    // ---------------------------------------------------------- Gradients
    Wallpaper {
        id: "dusk",
        name: "Dusk",
        family: Family::Gradients,
        scheme: Scheme::Dark { primary: 0x5A3A8A, surface: 0x161226 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x141A3A), (38, 0x2B2452), (68, 0x5A2C56), (88, 0x8A3E48), (100, 0x9A5540)]),
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "aurora",
        name: "Aurora",
        family: Family::Gradients,
        scheme: Scheme::Dark { primary: 0x187260, surface: 0x081923 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x07131F), (100, 0x0B1A2A)]),
            layers: &[
                Layer::Glow { cx: 90, cy: 250, r: 300, color: 0x0F8A6C, alpha: 150 },
                Layer::Glow { cx: 330, cy: 300, r: 280, color: 0x2A7FA8, alpha: 130 },
                Layer::Glow { cx: 470, cy: 60, r: 260, color: 0x5A3AA0, alpha: 120 },
                Layer::Glow { cx: 200, cy: -40, r: 220, color: 0x16506E, alpha: 110 },
            ],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "deep-sea",
        name: "Deep Sea",
        family: Family::Gradients,
        scheme: Scheme::Dark { primary: 0x14687A, surface: 0x061921 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Radial {
                cx: 120,
                cy: 300,
                r: 520,
                stops: &[(0, 0x14707C), (35, 0x0E4A62), (70, 0x0A2740), (100, 0x060F1E)],
            }),
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "slate",
        name: "Slate",
        family: Family::Gradients,
        scheme: Scheme::Dark { primary: 0x4A5A6E, surface: 0x181D24 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 3, ay: 4, stops: &[(0, 0x3A4654), (55, 0x28313C), (100, 0x181D25)] }),
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "nebula",
        name: "Nebula",
        family: Family::Gradients,
        scheme: Scheme::Dark { primary: 0x7A2E86, surface: 0x1D1027 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x100A1E), (100, 0x160C22)]),
            layers: &[
                Layer::Glow { cx: 60, cy: 40, r: 280, color: 0x8A2A7A, alpha: 135 },
                Layer::Glow { cx: 420, cy: 250, r: 300, color: 0x3A3AA8, alpha: 140 },
                Layer::Glow { cx: 250, cy: 150, r: 180, color: 0x5A2C90, alpha: 90 },
                Layer::Glow { cx: 470, cy: 0, r: 200, color: 0xA83A58, alpha: 80 },
            ],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "sunrise",
        name: "Sunrise",
        family: Family::Gradients,
        scheme: Scheme::Light { primary: 0xB0503A, surface: 0xF6EAE2 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0xBFD3E6), (40, 0xE9D3D6), (72, 0xF6CDB4), (100, 0xF3B58E)]),
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "mist",
        name: "Mist",
        family: Family::Gradients,
        scheme: Scheme::Light { primary: 0x45617E, surface: 0xECEFF3 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 2, ay: 5, stops: &[(0, 0xE9EDF1), (60, 0xD3DAE2), (100, 0xBCC6D1)] }),
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "regatta",
        name: "Regatta",
        family: Family::Gradients,
        scheme: REGATTA,
        tweaks: REGATTA_ROLES,
        spec: Spec {
            // Yellow from the left, bent into slow curves, with two thin
            // ribbons of flame in it; a red edge and the navy at the
            // right, clear of the clock, the date, and the status icons.
            base: Base::Flow {
                flow: Flow::Ribbons { ax: 4, ay: 1, waves: &[(230, 0, 210, 40), (120, 330, 150, 130), (60, 140, 0, 10)] },
                stops: &[
                    (0, AMBER), (16, YELLOW), (24, PALE), (27, FLAME), (30, YELLOW), (45, YELLOW), (49, FLAME), (52, AMBER),
                    (68, YELLOW), (75, RED), (79, NAVY), (100, NAVY_DEEP),
                ],
            },
            layers: &[],
            dither: true,
            // The bends travel along the ribbons, each at its own pace, so
            // the flame and the red edge fold into the yellow and the navy.
            moves: Some(Moves { every_ms: 100, motion: Motion::Run(&[1, 2, -1]) }),
        },
    },
    // ------------------------------------------------------------ Bubbles
    Wallpaper {
        id: "bokeh",
        name: "Bokeh",
        family: Family::Bubbles,
        scheme: Scheme::Dark { primary: 0x3E56A8, surface: 0x111827 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 2, ay: 3, stops: &[(0, 0x161C30), (100, 0x0A0E1A)] }),
            layers: &[
                Layer::Bubbles { seed: 7, count: 9, r: (40, 78), alpha: (18, 34), soft: 60, rim: 0, colors: &[0x5A7CC8, 0x8A6CC0, 0x4A9AB8] },
                Layer::Bubbles { seed: 21, count: 14, r: (14, 34), alpha: (26, 50), soft: 30, rim: 0, colors: &[0x7C9CE0, 0xC89A6A, 0x9C84D8, 0x5AB0C8] },
            ],
            dither: true,
            moves: Some(Moves { every_ms: 100, motion: Motion::Wander { reach: (7, 5), turns: (2, 4) } }),
        },
    },
    Wallpaper {
        id: "fizz",
        name: "Fizz",
        family: Family::Bubbles,
        scheme: Scheme::Dark { primary: 0x1C7480, surface: 0x081C1E },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x0A262C), (100, 0x103E46)]),
            layers: &[
                Layer::Bubbles { seed: 3, count: 20, r: (5, 17), alpha: (10, 22), soft: 0, rim: 34, colors: &[0x7CC4C0, 0x9AD4CA] },
                Layer::Bubbles { seed: 11, count: 6, r: (20, 34), alpha: (8, 16), soft: 0, rim: 28, colors: &[0x7CC4C0] },
            ],
            dither: true,
            // Up and down more than across, as bubbles in a glass.
            moves: Some(Moves { every_ms: 50, motion: Motion::Wander { reach: (3, 7), turns: (3, 6) } }),
        },
    },
    Wallpaper {
        id: "lanterns",
        name: "Lanterns",
        family: Family::Bubbles,
        scheme: Scheme::Dark { primary: 0xA04A22, surface: 0x231311 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x1E1018), (100, 0x2A1412)]),
            layers: &[
                Layer::Bubbles { seed: 5, count: 8, r: (36, 70), alpha: (22, 40), soft: 70, rim: 0, colors: &[0xD8782E, 0xC84A3A, 0xE0A040] },
                Layer::Bubbles { seed: 17, count: 12, r: (12, 28), alpha: (30, 56), soft: 40, rim: 0, colors: &[0xE89A40, 0xD86444, 0xF0C060] },
            ],
            dither: true,
            moves: Some(Moves { every_ms: 100, motion: Motion::Wander { reach: (5, 7), turns: (2, 4) } }),
        },
    },
    Wallpaper {
        id: "orbit",
        name: "Orbit",
        family: Family::Bubbles,
        scheme: Scheme::Dark { primary: 0x4A4AB8, surface: 0x13122B },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 4, ay: 3, stops: &[(0, 0x1A1846), (100, 0x10102A)] }),
            layers: &[Layer::Circles {
                soft: 0,
                rim: 0,
                circles: &[
                    (60, 250, 150, 0x4A5AD0, 46),
                    (190, 300, 130, 0x7A4AC8, 44),
                    (330, 40, 120, 0x3A7AD0, 40),
                    (440, 150, 110, 0x6A5AD8, 42),
                    (250, 120, 60, 0x5A8AE0, 30),
                ],
            }],
            dither: true,
            moves: Some(Moves { every_ms: 50, motion: Motion::Wander { reach: (8, 8), turns: (3, 5) } }),
        },
    },
    Wallpaper {
        id: "pebbles",
        name: "Pebbles",
        family: Family::Bubbles,
        scheme: Scheme::Light { primary: 0x7A6450, surface: 0xEFEAE2 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0xE8E2D8), (100, 0xD8D0C4)]),
            layers: &[
                Layer::Bubbles { seed: 9, count: 12, r: (26, 64), alpha: (36, 70), soft: 0, rim: 0, colors: &[0xC8B8A0, 0xB4C0B8, 0xD8C0B0, 0xF4F0EA] },
            ],
            dither: true,
            moves: None,
        },
    },
    // ------------------------------------------------------------- Modern
    Wallpaper {
        id: "slant",
        name: "Slant",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x1E6474, surface: 0x0E1D26 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 1, ay: 0, stops: &[(0, 0x10202C), (100, 0x16303E)] }),
            layers: &[
                // The clock and the date are where x - y is between -170
                // and 205: the bands keep clear of them.
                Layer::Bands { num: -1, den: 1, add: false, color: 0x1E5468, bands: &[(230, 96, 170, 170), (-330, 130, 110, 110)] },
                Layer::Bands { num: -1, den: 1, add: false, color: 0x2A7C82, bands: &[(334, 44, 170, 170), (420, 160, 70, 70)] },
                Layer::Bands { num: -1, den: 1, add: false, color: 0xC89A48, bands: &[(326, 8, 220, 220)] },
            ],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "prism",
        name: "Prism",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x4A3EA8, surface: 0x14122B },
        tweaks: &[],
        spec: Spec {
            base: Base::Poly {
                grad: Grad::Linear { ax: 3, ay: 2, stops: &[(0, 0x1C2A6A), (50, 0x3A2A78), (100, 0x1A1440)] },
                cell: 56,
                jitter: 14,
                seed: 4,
                amp: 9,
            },
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "glacier",
        name: "Glacier",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x1A6E84, surface: 0x0A1D23 },
        tweaks: &[],
        spec: Spec {
            base: Base::Poly {
                grad: Grad::Radial { cx: 480, cy: 0, r: 560, stops: &[(0, 0x1E7488), (50, 0x124A5E), (100, 0x0A2030)] },
                cell: 44,
                jitter: 11,
                seed: 12,
                amp: 8,
            },
            layers: &[],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "tides",
        name: "Tides",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x1A5C86, surface: 0x091B2D },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x0C1830), (100, 0x102444)]),
            layers: &[Layer::Waves(&[
                Wave { y: 120, amp: 16, period: 520, phase: 30, amp2: 5, period2: 170, color: 0x14345C, alpha: 256, fade: 0 },
                Wave { y: 165, amp: 14, period: 400, phase: 150, amp2: 4, period2: 140, color: 0x164A70, alpha: 256, fade: 0 },
                Wave { y: 205, amp: 12, period: 460, phase: 80, amp2: 5, period2: 120, color: 0x175A74, alpha: 256, fade: 0 },
                Wave { y: 240, amp: 10, period: 340, phase: 210, amp2: 3, period2: 110, color: 0x1A6C7C, alpha: 256, fade: 0 },
            ])],
            dither: true,
            // Each wave rises and falls a few pixels, the nearer ones a
            // little more, no two in step.
            moves: Some(Moves { every_ms: 50, motion: Motion::Bob(&[(48, 6, 0), (56, 8, 90), (64, 10, 170), (72, 8, 220)]) }),
        },
    },
    Wallpaper {
        id: "dunes",
        name: "Dunes",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x9A4E30, surface: 0x261711 },
        tweaks: &[],
        spec: Spec {
            base: vertical(&[(0, 0x2A1820), (60, 0x4A2428), (100, 0x5A2C28)]),
            layers: &[Layer::Waves(&[
                Wave { y: 110, amp: 18, period: 620, phase: 200, amp2: 0, period2: 0, color: 0x9A5236, alpha: 110, fade: 90 },
                Wave { y: 160, amp: 16, period: 480, phase: 60, amp2: 0, period2: 0, color: 0xA45E38, alpha: 110, fade: 80 },
                Wave { y: 208, amp: 14, period: 560, phase: 130, amp2: 0, period2: 0, color: 0xAE6C40, alpha: 110, fade: 70 },
            ])],
            dither: true,
            moves: Some(Moves { every_ms: 50, motion: Motion::Bob(&[(56, 4, 40), (64, 6, 150), (72, 4, 230)]) }),
        },
    },
    Wallpaper {
        id: "summit",
        name: "Summit",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x2E6E52, surface: 0x131E19 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 2, ay: 3, stops: &[(0, 0x1C2E28), (100, 0x10201C)] }),
            layers: &[Layer::Contours { seed: 8, scale: 150, levels: 14, color: 0x7CB094, alpha: 60, major: 4, major_alpha: 120 }],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "pinpoint",
        name: "Pinpoint",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x4E5866, surface: 0x1D1F23 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Radial { cx: 150, cy: 136, r: 420, stops: &[(0, 0x2A2D33), (100, 0x16181C)] }),
            layers: &[Layer::Dots { px: 16, py: 16, r: 22, stagger: false, color: 0x8C96A6, alpha: 90 }],
            dither: true,
            moves: None,
        },
    },
    Wallpaper {
        id: "drift",
        name: "Drift",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x2E5A9A, surface: 0x121D2D },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 1, ay: 1, stops: &[(0, 0x16223A), (100, 0x121C30)] }),
            layers: &[
                Layer::Circles {
                    soft: 40,
                    rim: 0,
                    circles: &[(60, 40, 130, 0x2A5490, 130), (410, 240, 160, 0x1E6E80, 130), (310, 10, 100, 0x5A4698, 110), (160, 300, 120, 0x3A4AA0, 110)],
                },
            ],
            dither: true,
            moves: Some(Moves { every_ms: 100, motion: Motion::Wander { reach: (16, 12), turns: (1, 2) } }),
        },
    },
    Wallpaper {
        id: "eclipse",
        name: "Eclipse",
        family: Family::Modern,
        scheme: Scheme::Dark { primary: 0x8A4440, surface: 0x1D171B },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Linear { ax: 1, ay: 1, stops: &[(0, 0x221C24), (100, 0x15121A)] }),
            layers: &[
                Layer::Circles { soft: 0, rim: 0, circles: &[(390, 290, 210, 0x3A2C3C, 256), (390, 290, 150, 0x5A3440, 256), (390, 290, 92, 0x8A4440, 256)] },
                Layer::Circles { soft: 0, rim: 0, circles: &[(60, -30, 120, 0x2E2632, 256)] },
            ],
            dither: true,
            moves: None,
        },
    },
    // ------------------------------------------------------------ Classic
    Wallpaper {
        id: "houndstooth",
        name: "Houndstooth",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x6E5E4E, surface: 0x1E1B19 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x47423C)),
            layers: &[Layer::Weave { kind: Weave::Houndstooth { unit: 4 }, color: 0x322F2B, alpha: 256, color2: 0, alpha2: 0 }],
            dither: false,
            moves: None,
        },
    },
    Wallpaper {
        id: "herringbone",
        name: "Herringbone",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x44586E, surface: 0x151B23 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x34404E)),
            layers: &[Layer::Weave { kind: Weave::Herringbone { col: 30, period: 15 }, color: 0x2C3744, alpha: 256, color2: 0, alpha2: 0 }],
            dither: false,
            moves: None,
        },
    },
    Wallpaper {
        id: "chevron",
        name: "Chevron",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x2E6A60, surface: 0x121F1D },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x2C4541)),
            layers: &[Layer::Weave { kind: Weave::Chevron { px: 56, py: 32 }, color: 0x35524D, alpha: 256, color2: 0, alpha2: 0 }],
            dither: false,
            moves: None,
        },
    },
    Wallpaper {
        id: "tartan",
        name: "Tartan",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x1F5E4A, surface: 0x0D1915 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x1E3A32)),
            layers: &[
                Layer::Stripes {
                    vertical: true,
                    repeat: 96,
                    stripes: &[(0, 34, 0x14243C, 128), (48, 2, 0xB8A46C, 60), (64, 12, 0x14243C, 128), (86, 2, 0x8E4440, 90)],
                },
                Layer::Stripes {
                    vertical: false,
                    repeat: 96,
                    stripes: &[(0, 34, 0x14243C, 128), (48, 2, 0xB8A46C, 60), (64, 12, 0x14243C, 128), (86, 2, 0x8E4440, 90)],
                },
            ],
            dither: false,
            moves: None,
        },
    },
    Wallpaper {
        id: "argyle",
        name: "Argyle",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x6A4478, surface: 0x1D1523 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x34293C)),
            layers: &[Layer::Weave { kind: Weave::Argyle { w: 60, h: 90, line: 16 }, color: 0x4A3450, alpha: 256, color2: 0xA8946C, alpha2: 90 }],
            dither: false,
            moves: None,
        },
    },
    Wallpaper {
        id: "polka",
        name: "Polka",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x34508A, surface: 0x111929 },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x1C2A44)),
            layers: &[Layer::Dots { px: 36, py: 18, r: 80, stagger: true, color: 0xC8B8A0, alpha: 48 }],
            dither: false,
            moves: None,
        },
    },
    Wallpaper {
        id: "pinstripe",
        name: "Pinstripe",
        family: Family::Classic,
        scheme: Scheme::Dark { primary: 0x3A4668, surface: 0x13161F },
        tweaks: &[],
        spec: Spec {
            base: Base::Grad(Grad::Solid(0x1A1E2A)),
            layers: &[Layer::Stripes { vertical: true, repeat: 12, stripes: &[(5, 1, 0xA8B0C4, 70)] }],
            dither: false,
            moves: None,
        },
    },
    // The one joke: ketchup and mustard, as loud as they come, on every
    // screen.  Nothing here is toned down.  It can be read all the same:
    // the text is black, which pure red and pure yellow both carry (5.3
    // and 19.6 to 1), where white on the red would be 4.0.
    Wallpaper {
        id: "hot-dog-stand",
        name: "Hot Dog Stand",
        family: Family::Classic,
        scheme: Scheme::Hand(Palette {
            bg: KETCHUP_MUSTARD.1,
            fg: 0x000000,
            title_bg: KETCHUP_MUSTARD.0,
            title_fg: 0x000000,
            sel_bg: KETCHUP_MUSTARD.0,
            sel_fg: 0x000000,
            secondary: 0x5A0000,
            border: KETCHUP_MUSTARD.0,
            modal_bg: KETCHUP_MUSTARD.1,
            sk_bg: KETCHUP_MUSTARD.1,
            sk_edge: 0x000000,
            sk_fg: 0x000000,
            sk_go: 0x00FF00,
            sk_stop: KETCHUP_MUSTARD.0,
            good: 0x1B5E20,
            warn: 0x8A4B00,
            error: 0xC00000,
            accent: KETCHUP_MUSTARD.0,
            card: KETCHUP_MUSTARD.1,
            card_ringing: 0x00FF00,
            card_held: 0xFF8000,
            card_dark: KETCHUP_MUSTARD.0,
            tab_bg: KETCHUP_MUSTARD.1,
            tab_fg: 0x000000,
            load_track: KETCHUP_MUSTARD.0,
            load_fill: KETCHUP_MUSTARD.1,
            wallpaper_fg: 0x000000,
        }),
        tweaks: &[],
        spec: Spec {
            // The stand's awning.
            base: Base::Grad(Grad::Solid(KETCHUP_MUSTARD.0)),
            layers: &[Layer::Stripes { vertical: true, repeat: 80, stripes: &[(40, 40, KETCHUP_MUSTARD.1, 256)] }],
            dither: false,
            moves: None,
        },
    },
    // ------------------------------------------------------------- Solids
    solid("black", "Black", 0x000000, Scheme::Dark { primary: 0x444444, surface: 0x0A0A0A }),
    solid("charcoal", "Charcoal", 0x24262A, Scheme::Dark { primary: 0x4A5058, surface: 0x1C1D20 }),
    solid("navy", "Navy", 0x12213C, Scheme::Dark { primary: 0x2A4E8E, surface: 0x0F1828 }),
    solid("forest", "Forest", 0x14302A, Scheme::Dark { primary: 0x226A54, surface: 0x0F1E1A }),
    solid("stone", "Stone", 0xD9D5CE, Scheme::Light { primary: 0x5E6470, surface: 0xECE9E4 }),
];
