//! A phone's 3D model, built from the panel's data.
//!
//! The geometry is original: every part is generated here from the panel
//! drawing (the positions of the display and the keys) and a few
//! dimensions chosen by eye.  No maker's name or mark is modeled.
//!
//! Lengths are millimeters.  The model's frame is the face's: x across the
//! face, y up the face, z out of the face.  The face leans back by `TILT`
//! from the desk; `to_world` turns the model's frame into the world's (x
//! across, y toward the back, z up).

use crate::geom::{self, Mesh, P2, V3, v3};
use crate::panel::{Fill, Key, NEW_FACE, NEW_UNIT_MM, Panel, Parts};
use std::f64::consts::PI;

pub const TILT_DEG: f64 = 50.0;
/// The old family's base plate's thickness.
const TRAY_T: f64 = 7.0;
const BODY_T: f64 = 24.0;
const KEY_H: f64 = 1.3;
/// The cradle's surface, below the face.
const WING_Z: f64 = -5.0;
/// The old family's display housing: the housing's surface above the
/// body's face.
const HOUSING_Z: f64 = 2.6;

/// A material, as glTF's metallic-roughness model has one; colors are
/// linear.
#[derive(Clone)]
pub struct Material {
    pub name: String,
    pub color: [f64; 3],
    pub alpha: f64,
    pub rough: f64,
    pub metal: f64,
    pub coat: f64,
    pub emissive: [f64; 3],
    pub emissive_strength: f64,
    pub base_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
    /// Shadows: false for the legends, which float on the keys.
    pub casts_shadow: bool,
}

fn mat(name: &str, color: [f64; 3], rough: f64) -> Material {
    Material {
        name: name.into(),
        color,
        alpha: 1.0,
        rough,
        metal: 0.0,
        coat: 0.0,
        emissive: [0.0; 3],
        emissive_strength: 1.0,
        base_texture: None,
        emissive_texture: None,
        casts_shadow: true,
    }
}

impl Material {
    fn metal(mut self, m: f64) -> Material {
        self.metal = m;
        self
    }

    fn coat(mut self, c: f64) -> Material {
        self.coat = c;
        self
    }

    fn emit(mut self, color: [f64; 3], strength: f64) -> Material {
        self.emissive = color;
        self.emissive_strength = strength;
        self
    }
}

/// A color written as #rrggbb, in linear values.
pub fn srgb(h: &str) -> [f64; 3] {
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
    [c(1), c(3), c(5)].map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
}

/// A linear color as sRGB bytes.
pub fn srgb_bytes(c: [f64; 3]) -> [u8; 3] {
    c.map(|v| {
        let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

/// A picture printed over part of the face: a PNG file, and the box the
/// picture covers (the panel's units: x0, y0, x1, y1).
pub struct Print {
    pub png: Vec<u8>,
    pub at: [f64; 4],
}

/// The pictures the model's materials use, as PNG files.
pub struct Textures {
    pub legends: Option<Print>,
    pub rocker: Option<Vec<u8>>,
    pub screen: Vec<u8>,
    pub side_screen: Option<Vec<u8>>,
}

pub struct Model {
    pub materials: Vec<Material>,
    pub images: Vec<Vec<u8>>,
    /// Parts by material, in the model's frame.
    pub parts: Vec<(usize, Mesh)>,
    /// The desk's height in the world.
    pub desk_z: f64,
}

struct Builder<'a> {
    p: &'a Panel,
    k: f64,
    cx: f64,
    cy: f64,
    materials: Vec<Material>,
    images: Vec<Vec<u8>>,
    parts: Vec<(usize, Mesh)>,
}

pub fn to_world(v: V3) -> V3 {
    let (s, c) = TILT_DEG.to_radians().sin_cos();
    v3(v.x, v.y * c - v.z * s, v.y * s + v.z * c)
}

/// World millimeters, or a world direction, in the model's frame.
fn to_model(w: V3) -> V3 {
    let (y, z) = to_local(w.y, w.z);
    v3(w.x, y, z)
}

fn to_local(wy: f64, wz: f64) -> P2 {
    let (s, c) = TILT_DEG.to_radians().sin_cos();
    (wy * c + wz * s, -wy * s + wz * c)
}

fn smoothstep(a: f64, b: f64, t: f64) -> f64 {
    let t = ((t - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Builder<'_> {
    fn fx(&self, x: f64) -> f64 {
        (x - self.cx) * self.k
    }

    fn fy(&self, y: f64) -> f64 {
        -(y - self.cy) * self.k
    }

    /// Panel-unit points as face millimeters.
    fn face(&self, pts: &[P2]) -> Vec<P2> {
        pts.iter().map(|&(x, y)| (self.fx(x), self.fy(y))).collect()
    }

    fn rrect(&self, x: f64, y: f64, w: f64, h: f64, r: [f64; 4]) -> Vec<P2> {
        self.face(&geom::rrect(x, y, w, h, r, 12))
    }

    fn material(&mut self, m: Material) -> usize {
        if let Some(i) = self.materials.iter().position(|o| o.name == m.name) {
            return i;
        }
        self.materials.push(m);
        self.materials.len() - 1
    }

    fn add(&mut self, m: usize, mesh: Mesh) {
        self.parts.push((m, mesh));
    }

    fn prism(&mut self, pts: Vec<P2>, z0: f64, z1: f64, m: usize, bevel: f64, segs: usize) {
        self.add(m, geom::prism(&pts, z0, z1, bevel, segs));
    }

    /// A rectangle (panel units) at height z, with texture coordinates over
    /// the whole.
    fn plane(&mut self, [x, y, w, h]: [f64; 4], z: f64, m: usize) {
        let mut mesh = Mesh::default();
        let up = v3(0.0, 0.0, 1.0);
        let a = mesh.vertex_uv(v3(self.fx(x), self.fy(y + h), z), up, (0.0, 1.0));
        let b = mesh.vertex_uv(v3(self.fx(x + w), self.fy(y + h), z), up, (1.0, 1.0));
        let c = mesh.vertex_uv(v3(self.fx(x + w), self.fy(y), z), up, (1.0, 0.0));
        let d = mesh.vertex_uv(v3(self.fx(x), self.fy(y), z), up, (0.0, 0.0));
        mesh.quad(a, b, c, d);
        self.add(m, mesh);
    }

    /// A key from below the surface `base` to KEY_H above: a rounded
    /// rectangle, or (`arch`) straight sides with bowed top and bottom
    /// edges.
    fn key(&mut self, k: &Key, base: f64, m: usize) {
        let (x, y) = (k.cx - k.w / 2.0, k.cy - k.h / 2.0);
        // Keys are small: fewer segments than the body's outlines.
        let pts = if k.arch > 0.0 {
            self.face(&geom::arched(x, y, k.w, k.h, k.r, k.arch, 12))
        } else {
            self.face(&geom::rrect(x, y, k.w, k.h, [k.r.min(k.h / 2.0 - 0.01); 4], 5))
        };
        self.prism(pts, base - 0.5, base + KEY_H, m, 0.45, 2);
    }

    /// A rocker key over the box of key `k`: one cap, lowest across the
    /// middle, where the key pivots, and rising in a shallow V to each end.
    /// The cap's texture coordinates cover the key's box.
    fn rocker(&mut self, k: &Key, z0: f64, z_mid: f64, z_end: f64, m: usize, m_side: usize) {
        let (x0, x1) = (k.cx - k.w / 2.0, k.cx + k.w / 2.0);
        let (hh, rad) = (k.h / 2.0, k.r);
        let (nx, ny) = (96usize, 6usize);
        let mut top = Mesh::default();
        let mut cols = Vec::with_capacity(nx + 1);
        for i in 0..=nx {
            let x = x0 + (x1 - x0) * i as f64 / nx as f64;
            let u = (2.0 * i as f64 / nx as f64 - 1.0).abs();
            let e = (x - x0).min(x1 - x);
            let half = if e >= rad { hh } else { hh - rad + (rad * rad - (rad - e).powi(2)).max(0.0).sqrt() };
            let z = z_mid + (z_end - z_mid) * u;
            cols.push(
                (0..=ny)
                    .map(|j| {
                        let py = k.cy + half * (2.0 * j as f64 / ny as f64 - 1.0);
                        let uv = ((x - x0) / (x1 - x0), (py - (k.cy - hh)) / (2.0 * hh));
                        top.vertex_uv(v3(self.fx(x), self.fy(py), z), v3(0.0, 0.0, 1.0), uv)
                    })
                    .collect::<Vec<_>>(),
            );
        }
        for i in 0..nx {
            for j in 0..ny {
                // The panel's y runs down the face: this order faces out.
                top.quad(cols[i][j], cols[i][j + 1], cols[i + 1][j + 1], cols[i + 1][j]);
            }
        }
        top.smooth_normals();
        // The walls down to z0, all around the cap's rim.
        let mut rim: Vec<u32> = cols.iter().map(|c| c[0]).collect();
        rim.extend(cols[nx][1..ny].iter());
        rim.extend(cols.iter().rev().map(|c| c[ny]));
        rim.extend(cols[0][1..ny].iter().rev());
        let ring: Vec<V3> = rim.iter().map(|&i| top.pos[i as usize]).collect();
        let outline: Vec<P2> = ring.iter().map(|p| (p.x, p.y)).collect();
        let mut walls = geom::prism(&outline, z0, z_mid, 0.0, 0);
        // The prism's top is flat at z_mid; lift each wall's top edge to the
        // cap's rim, and drop the caps (the bottom is under the panel).
        let cap_from = walls.idx.len() - 2 * 3 * geom::triangulate(&geom::ccw(&outline)).len();
        walls.idx.truncate(cap_from);
        for p in walls.pos.iter_mut() {
            if (p.z - z_mid).abs() < 1e-9
                && let Some(r) = ring.iter().find(|r| (r.x - p.x).abs() < 1e-9 && (r.y - p.y).abs() < 1e-9)
            {
                p.z = r.z;
            }
        }
        self.add(m, top);
        self.add(m_side, walls);
    }

    /// A lofted handset, even along the handset's length: one rounded
    /// cross-section from end to end, bowed gently so that the middle
    /// stands clear of the cradle, with rounded ends.  Returns the opening
    /// of the jack's recess in the foot.
    fn handset(&mut self, m_handset: usize, m_cup: usize) -> V3 {
        const HANDSET_T: f64 = 21.0;
        const HANDSET_BOW: f64 = 4.0;
        let [hx, hy, hw, hh] = self.p.handset;
        let cx = self.fx(hx + hw / 2.0);
        let (y_top, y_bot) = (self.fy(hy), self.fy(hy + hh));
        let (rings, around) = (96usize, 48usize);
        let mut mesh = Mesh::default();
        let mut ends = Vec::new();
        for i in 0..=rings {
            let t = i as f64 / rings as f64;
            let e = t.min(1.0 - t);
            let cap = (1.0 - (1.0 - (e / 0.07).min(1.0)).powi(2)).max(0.0).sqrt().max(0.02);
            let half = hw * self.k / 2.0 * (0.3 + 0.7 * cap);
            let bot = WING_Z + 0.3 + HANDSET_BOW * (PI * t).sin();
            let (zc, b) = (bot + HANDSET_T / 2.0, HANDSET_T / 2.0 * (0.25 + 0.75 * cap));
            let y = y_top + (y_bot - y_top) * t;
            let mut ring = Vec::with_capacity(around);
            for j in 0..around {
                let a = 2.0 * PI * j as f64 / around as f64;
                let (s, c) = a.sin_cos();
                // A squarer underside than back.
                let n = if s < 0.0 { 3.2 } else { 2.4 };
                let x = half * c.signum() * c.abs().powf(2.0 / n);
                let z = zc + b * s.signum() * s.abs().powf(2.0 / n);
                ring.push(v3(cx + x, y, z));
            }
            if i == 0 || i == rings {
                ends.push(ring.clone());
            }
            for p in ring {
                mesh.vertex(p, v3(0.0, 0.0, 0.0));
            }
        }
        for i in 0..rings {
            for j in 0..around {
                let k = (j + 1) % around;
                let (a, b, c, d) = (i * around + j, i * around + k, (i + 1) * around + k, (i + 1) * around + j);
                mesh.quad(a as u32, b as u32, c as u32, d as u32);
            }
        }
        mesh.smooth_normals();
        // Face the surface outward: the first ring's x-most point.
        if mesh.nrm[0].x < 0.0 {
            for t in mesh.idx.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
            for n in mesh.nrm.iter_mut() {
                *n = -*n;
            }
        }
        // Flat ends.
        for (ring, dir) in ends.iter().zip([1.0, -1.0]) {
            let normal = v3(0.0, dir, 0.0);
            let centre = ring.iter().fold(V3::default(), |a, &p| a + p) * (1.0 / ring.len() as f64);
            let c = mesh.vertex(centre, normal);
            let base = mesh.pos.len() as u32;
            for &p in ring {
                mesh.vertex(p, normal);
            }
            for j in 0..around as u32 {
                let k = (j + 1) % around as u32;
                let (a, b) = (base + j, base + k);
                // Wind each fan to face along `normal`.
                let (pa, pb) = (mesh.pos[a as usize], mesh.pos[b as usize]);
                if (pa - centre).cross(pb - centre).dot(normal) > 0.0 { mesh.tri(c, a, b) } else { mesh.tri(c, b, a) }
            }
        }
        self.add(m_handset, mesh);
        // The jack's recess opens in the middle of the foot's end: the cord
        // goes in, and the plug is out of sight.
        let zc = WING_Z + 0.3 + HANDSET_T / 2.0;
        let mut hole = Mesh::default();
        let n = v3(0.0, -1.0, 0.0);
        let y = y_bot - 0.02;
        let (w, h) = (11.0 / 2.0, 5.0 / 2.0);
        let a = hole.vertex(v3(cx - w, y, zc - h), n);
        let b = hole.vertex(v3(cx + w, y, zc - h), n);
        let c = hole.vertex(v3(cx + w, y, zc + h), n);
        let d = hole.vertex(v3(cx - w, y, zc + h), n);
        hole.quad(a, b, c, d);
        self.add(m_cup, hole);
        v3(cx, y_bot + 2.0, zc)
    }

    /// A bar of `width` (along `across`) by `thick`, from `from` to `to`,
    /// with rounded edges (the model's frame).
    #[allow(clippy::too_many_arguments)]
    fn bar(&mut self, m: usize, from: V3, to: V3, across: V3, width: f64, thick: f64, bevel: f64) {
        let d = (to - from).normalized();
        let u = (across - d * across.dot(d)).normalized();
        let v = d.cross(u);
        let (w, t) = (width / 2.0, thick / 2.0);
        let rect = [(-w, -t), (w, -t), (w, t), (-w, t)];
        let mesh = geom::prism(&rect, 0.0, (to - from).length(), bevel, 3).map(|p| from + u * p.x + v * p.y + d * p.z);
        self.add(m, mesh);
    }

    /// A slab of the outline `pts` (face millimeters) from z0 to z1.
    fn slab(&mut self, m: usize, [xa, ya, xb, yb]: [f64; 4], r: f64, z0: f64, z1: f64) {
        self.add(m, geom::prism(&face_box(xa, ya, xb, yb, r), z0, z1, 0.8, 2));
    }

    /// A foot on the desk, centered at `at` (world millimeters): `long`
    /// along the world's y, `wide` across, `high` up.
    #[allow(clippy::too_many_arguments)]
    fn pad(&mut self, m: usize, at: V3, desk_z: f64, long: f64, wide: f64, high: f64) {
        let along = to_model(v3(0.0, 1.0, 0.0));
        let c = to_model(v3(at.x, at.y, desk_z + high / 2.0));
        self.bar(m, c - along * (long / 2.0), c + along * (long / 2.0), v3(1.0, 0.0, 0.0), wide, high, 0.8);
    }

    /// The stand of the D60, D65, and D80: one bent sheet in the shape of a
    /// tent, two plates the same width meeting at a rounded ridge a little
    /// above the middle of the body's back.  The front plate lies against
    /// the body's back, with two slots; the back plate drops to the desk
    /// behind.  Each plate's lower part is open, a window between two side
    /// bars, and each plate ends in a rail along the desk that joins the
    /// side bars, with a rubber foot at each end.  `body`: the face's box,
    /// x0, y0 (bottom), x1, y1 (top), in face millimeters; the stand is
    /// centered under the box's bottom edge.
    fn tent(&mut self, [x0, y0, x1, _]: [f64; 4], desk_z: f64, m: usize, m_pad: usize) {
        const T: f64 = 3.0; // the sheet's thickness
        const BAR: f64 = 13.0; // the side bars' and the rails' width
        // One stand for every model: sized for the D60's and D65's face.
        let [fx0, fy0, fx1, fy1] = NEW_FACE;
        let (w, h) = ((fx1 - fx0) * NEW_UNIT_MM, (fy1 - fy0) * NEW_UNIT_MM);
        let cx = (x0 + x1) / 2.0;
        // Narrower than the D60's and D65's body, so that from the front the
        // body hides the stand.
        let half = 0.30 * w;
        let (outer, inner) = (-BODY_T - T, -BODY_T + 0.3);
        let ridge = y0 + 0.56 * h;
        // The front plate, in the face's frame: the rail, the side bars
        // around the window, and the solid upper part.
        let wy1 = y0 + 0.30 * h;
        self.slab(m, [cx - half, y0 + 1.0, cx + half, y0 + 1.0 + BAR], 2.0, outer, inner);
        self.slab(m, [cx - half, y0 + BAR, cx - half + BAR, wy1 + 1.0], 0.5, outer, inner);
        self.slab(m, [cx + half - BAR, y0 + BAR, cx + half, wy1 + 1.0], 0.5, outer, inner);
        self.slab(m, [cx - half, wy1, cx + half, ridge], 2.0, outer, inner);
        // The slots: dark lines through the plate.
        for sx in [-1.0, 1.0] {
            let x = cx + sx * 0.4 * half;
            self.add(m_pad, geom::prism(&face_box(x - 1.2, wy1 + 0.03 * h, x + 1.2, ridge - 0.05 * h, 0.5), outer - 0.1, outer + 1.0, 0.0, 0));
        }
        // The ridge: the sheet folds over in a round.
        let (ry, rz) = (ridge - 1.0, -BODY_T - T * 0.5 - 0.4);
        let fold = geom::prism(&geom::circle(0.0, 0.0, T * 0.8, 20), cx - half, cx + half, 0.6, 2).map(|p| v3(p.z, ry + p.x, rz + p.y));
        self.add(m, fold);
        // The back plate: from the ridge down to the desk behind, at 40
        // degrees from the desk.  Drawn in the plate's own frame: x across,
        // y from the desk's rail up to the ridge, z through the sheet.
        let top = to_world(v3(cx, ry, rz));
        let bottom = v3(top.x, top.y + (top.z - desk_z - T / 2.0) / 40f64.to_radians().tan(), desk_z + T / 2.0);
        let (top_m, bottom_m) = (to_model(top), to_model(bottom));
        let length = (top_m - bottom_m).length();
        let (u, v) = (v3(1.0, 0.0, 0.0), (top_m - bottom_m).normalized());
        let n = u.cross(v);
        let place = |p: V3| bottom_m + u * (p.x - cx) + v * p.y + n * (p.z - T / 2.0);
        let open_top = 0.55 * length;
        for [xa, ya, xb, yb] in [
            [cx - half, 0.0, cx + half, BAR],
            [cx - half, BAR - 1.0, cx - half + BAR, open_top + 1.0],
            [cx + half - BAR, BAR - 1.0, cx + half, open_top + 1.0],
            [cx - half, open_top, cx + half, length],
        ] {
            let piece = geom::prism(&face_box(xa, ya, xb, yb, 1.0), 0.0, T, 0.8, 2).map(place);
            self.add(m, piece);
        }
        // The rubber feet, at both ends of each rail.
        for sx in [-1.0, 1.0] {
            let x = cx + sx * (half - BAR / 2.0);
            let front = to_world(v3(x, y0 + 2.0, -BODY_T - T * 0.5));
            self.pad(m_pad, front + v3(0.0, 6.0, 0.0), desk_z, 12.0, BAR + 1.0, 2.5);
            self.pad(m_pad, v3(x, bottom.y - 4.0, desk_z), desk_z, 12.0, BAR + 1.0, 2.5);
        }
    }

    /// A rectangular frame of bars: the corners `a` (bottom left), `a +
    /// u`, `a + v`, and `a + u + v` are the bars' outer corners, the bars
    /// `bar` wide and `thick` thick (the model's frame).
    #[allow(clippy::too_many_arguments)]
    fn frame(&mut self, m: usize, a: V3, u: V3, v: V3, bar: f64, thick: f64) {
        let (un, vn) = (u.normalized(), v.normalized());
        let (hu, hv) = (un * (bar / 2.0), vn * (bar / 2.0));
        // Across the bottom and the top, and up each side.
        self.bar(m, a + hv, a + u + hv, vn, bar, thick, 1.0);
        self.bar(m, a + v - hv, a + u + v - hv, vn, bar, thick, 1.0);
        self.bar(m, a + hu, a + v + hu, un, bar, thick, 1.0);
        self.bar(m, a + u - hu, a + u + v - hu, un, bar, thick, 1.0);
    }

    /// The old family's stand (D40, D50, D70): a thick base plate with
    /// rounded corners, behind the body's lower edge, and on the base an
    /// A-frame: two inclined rectangular frames, open in the middle and at
    /// the sides, meeting at a ridge.  Narrower than the body, so that from
    /// the front the body hides the stand.  The body's back clips onto
    /// the front frame; the back frame drops steeply to the base.
    fn aframe(&mut self, [x0, y0, x1, y1]: [f64; 4], desk_z: f64, m: usize) {
        const BAR: f64 = 10.0;
        const THICK: f64 = 6.0;
        let (w, h) = (x1 - x0, y1 - y0);
        let cx = (x0 + x1) / 2.0;
        let top = desk_z + TRAY_T;
        // The front frame, against the body's back.
        let fw = 0.52 * w;
        let (yb, yt) = (y0 + 0.06 * h, y0 + 0.60 * h);
        let zf = -BODY_T - THICK / 2.0;
        let a = v3(cx - fw / 2.0, yb, zf);
        self.frame(m, a, v3(fw, 0.0, 0.0), v3(0.0, yt - yb, 0.0), BAR, THICK);
        // The back frame: from the ridge down to the base, at 65 degrees
        // from the base.
        let ridge = to_world(v3(cx - fw / 2.0, yt - BAR / 2.0, zf));
        let foot = v3(ridge.x, ridge.y + (ridge.z - top) / 65f64.to_radians().tan(), top + THICK / 2.0);
        let (rm, fm) = (to_model(ridge), to_model(foot));
        self.frame(m, fm, v3(fw, 0.0, 0.0), rm - fm, BAR, THICK);
        // The ridge: a round along the top, where the frames meet.
        let fold = geom::prism(&geom::circle(0.0, 0.0, THICK * 0.6, 16), cx - fw / 2.0, cx + fw / 2.0, 0.6, 2)
            .map(|p| v3(p.z, rm.y + p.x, rm.z + p.y));
        self.add(m, fold);
        // The base, drawn in the world's (x, y) and extruded up from the
        // desk: from behind the body's lower edge to behind the back
        // frame's foot.  The body's lower edge bows up toward the corners;
        // the base starts far enough back that the body still hides the
        // base under the raised corners.
        let edge = to_world(v3(cx, y0, -BODY_T));
        let half = 0.32 * w;
        let (front, back) = (edge.y + 18.0, foot.y + 26.0);
        let outline = geom::rrect(cx - half, front, 2.0 * half, back - front, [18.0; 4], 8);
        self.add(m, geom::prism(&outline, desk_z, top, 2.0, 3).map(to_model));
    }

    /// The coiled cord: from the recessed jack in the handset's foot down
    /// to the desk, in a loop around the front left, back along the
    /// handset, and under the cradle up to the recessed jack in the
    /// cradle's underside.  The path is one cubic B-spline through control
    /// points given in world millimeters; the wire winds around the path at
    /// the coil's radius, carried along the path without twist, one turn per
    /// CORD_PITCH of path in the coil and not at all in the straight leads
    /// at the ends.  The coil rests on the desk.
    fn cord(&mut self, mouth: V3, desk_z: f64, m: usize) {
        const WIRE: f64 = 1.0;
        const COIL: f64 = 3.4;
        const PITCH: f64 = 2.75;
        const LEAD_HANDSET: f64 = 28.2;
        const LEAD_JACK: f64 = 21.0;
        const TURN_IN: f64 = 3.0;
        // Where the D65's handset jack is, in world millimeters (y): the
        // path below is drawn for the D65, and moves with the handset's foot.
        const D65_MOUTH_Y: f64 = -70.06;
        let [hx, _, hw, _] = self.p.handset;
        let x0 = self.fx(hx + hw / 2.0);
        let dy = to_world(mouth).y - D65_MOUTH_Y;
        let local = |x: f64, y: f64, z: f64| {
            let (ly, lz) = to_local(y + dy, z);
            v3(x, ly, lz)
        };
        let up = local(0.0, 0.0, 1.0) - local(0.0, 0.0, 0.0);
        let lo = desk_z + WIRE + 0.12;
        let axis = v3(0.0, -1.0, 0.0);
        let down = v3(0.0, 0.0, -1.0);
        let jack = v3(x0 + 14.0, self.fy(self.p.wing[3]) + 51.0, -BODY_T + 1.5);
        let mut ctrl = vec![mouth - axis * 6.0, mouth + axis * 1.0, mouth + axis * 8.0];
        for (x, y, dz) in [
            (x0 - 1.0, -80.5, 5.6),
            (x0 - 4.5, -88.5, 0.9),
            (x0 - 12.0, -95.5, 0.0),
            (x0 - 22.5, -99.5, 0.0),
            (x0 - 34.0, -97.0, 0.0),
            (x0 - 42.0, -86.0, 0.0),
            (x0 - 45.0, -68.0, 0.0),
            (x0 - 43.5, -48.0, 0.0),
            (x0 - 41.0, -28.0, 0.0),
            (x0 - 40.5, -8.0, 0.0),
            (x0 - 36.0, 9.0, 0.0),
            (x0 - 26.0, 19.0, 1.0),
            (x0 - 10.0, 22.0, 8.0),
        ] {
            ctrl.push(local(x, y, lo + dz));
        }
        ctrl.extend([jack + down * 30.0, jack + down * 14.0, jack + down * 1.0, jack - down * 6.0]);
        let step = PITCH / 32.0;
        let base = by_length(&bspline(&ctrl, 200), step);
        let n = base.len();
        let total = (n - 1) as f64 * step;
        let offset = |i: usize| {
            let s = i as f64 * step;
            COIL * smoothstep(3.0, LEAD_HANDSET - 5.0, s) * (1.0 - smoothstep(total - LEAD_JACK + 4.0, total - 2.0, s))
        };
        let winding = |i: usize| {
            let s = i as f64 * step;
            smoothstep(LEAD_HANDSET, LEAD_HANDSET + TURN_IN, s) * (1.0 - smoothstep(total - LEAD_JACK - TURN_IN, total - LEAD_JACK, s))
        };
        // The coil rests on the desk: lift the path by the wire's distance
        // from the path, and by less, gradually, where the path is already
        // above the desk.
        let mut path: Vec<V3> = base
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let h = (to_world(p).z - lo).max(0.0);
                p + up * (offset(i) * (1.0 - smoothstep(0.0, 3.0 * COIL, h)))
            })
            .collect();
        for _ in 0..60 {
            let prev = path.clone();
            for i in 1..n - 1 {
                path[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
            }
        }
        // A frame carried along the path without twist.
        let tans: Vec<V3> = (0..n).map(|i| (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalized()).collect();
        let mut sides = vec![tans[0].cross(up).normalized()];
        for i in 1..n {
            let s = sides[i - 1];
            sides.push((s - tans[i] * s.dot(tans[i])).normalized());
        }
        // The angle at which the handset's lead lies: under the path, where
        // the lead reaches the first turn.
        let i0 = (LEAD_HANDSET / step).round() as usize;
        let under = -(up - tans[i0] * up.dot(tans[i0])).normalized();
        let mut angle = under.dot(tans[i0].cross(sides[i0])).atan2(under.dot(sides[i0]));
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            if i > 0 {
                angle += 2.0 * PI * (path[i] - path[i - 1]).length() / PITCH * winding(i);
            }
            let around = sides[i] * angle.cos() + tans[i].cross(sides[i]) * angle.sin();
            pts.push(path[i] + around * offset(i));
        }
        // Every other point: 16 per turn are enough for the coil's curve.
        let last = pts[n - 1];
        let mut pts: Vec<V3> = pts.iter().step_by(2).copied().collect();
        if !(n - 1).is_multiple_of(2) {
            pts.push(last);
        }
        self.add(m, geom::tube(&pts, WIRE, 6));
    }
}

/// Points along the uniform cubic B-spline of `ctrl`, with each end point
/// counted three times so that the curve starts and ends on the end points.
fn bspline(ctrl: &[V3], per: usize) -> Vec<V3> {
    let mut c = vec![ctrl[0]; 2];
    c.extend_from_slice(ctrl);
    c.extend([ctrl[ctrl.len() - 1]; 2]);
    let mut out = Vec::new();
    for i in 0..c.len() - 3 {
        for s in 0..per {
            let u = s as f64 / per as f64;
            let b0 = (1.0 - u).powi(3) / 6.0;
            let b1 = (3.0 * u.powi(3) - 6.0 * u * u + 4.0) / 6.0;
            let b2 = (-3.0 * u.powi(3) + 3.0 * u * u + 3.0 * u + 1.0) / 6.0;
            let b3 = u.powi(3) / 6.0;
            out.push(c[i] * b0 + c[i + 1] * b1 + c[i + 2] * b2 + c[i + 3] * b3);
        }
    }
    out.push(ctrl[ctrl.len() - 1]);
    out
}

/// `pts` sampled again at every `step` of length along the polyline.
fn by_length(pts: &[V3], step: f64) -> Vec<V3> {
    let mut out = vec![pts[0]];
    let (mut at, mut want) = (0.0, step);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = (b - a).length();
        while d > 0.0 && at + d >= want {
            out.push(a.lerp(b, (want - at) / d));
            want += step;
        }
        at += d;
    }
    out
}

pub fn build(p: &Panel, tex: Textures) -> Model {
    let new = p.is_new();
    let mut b = Builder { p, k: p.unit_mm, cx: p.center.0, cy: p.center.1, materials: Vec::new(), images: Vec::new(), parts: Vec::new() };
    let panel_z = if new { 0.6 } else { 0.0 };

    let (body, handset) = if new {
        (mat("body", [0.022, 0.023, 0.025], 0.52), mat("handset", [0.026, 0.027, 0.030], 0.48))
    } else {
        (mat("body", srgb("#34373c"), 0.55), mat("handset", srgb("#303338"), 0.5))
    };
    let m_body = b.material(body);
    let m_handset = b.material(handset);
    let m_matte = b.material(mat("lower panel", [0.030, 0.031, 0.034], 0.58));
    let m_gloss = b.material(mat("upper panel", [0.006, 0.006, 0.007], 0.07).coat(1.0));
    let m_key = b.material(mat("black key", [0.012, 0.012, 0.014], 0.38));
    let m_cup = b.material(mat("cradle cup", [0.010, 0.010, 0.011], 0.6));
    let m_tick = b.material(mat("print", [0.16, 0.17, 0.18], 0.6));
    let m_cord = b.material(mat("cord", if new { [0.02, 0.02, 0.022] } else { srgb("#2c2f33") }, 0.45));
    let fill = |b: &mut Builder, f: Fill| -> usize {
        b.material(match f {
            Fill::Silver => mat("silver key", [0.66, 0.68, 0.70], 0.30).metal(0.35),
            Fill::Black => mat("black key", [0.012, 0.012, 0.014], 0.38),
            // The old family's soft keys: clear-ish slate plastic, the color
            // measured from photographs (the web drawing's teal is bluer).
            Fill::Teal => mat("soft key", srgb("#7e939a"), 0.3).coat(0.4),
            Fill::Housing => mat("housing key", srgb("#eceef0"), 0.32).metal(0.15),
            Fill::OldSilver => mat("silver key (old)", srgb("#d5d8db"), 0.30).metal(0.35),
        })
    };
    let screen = |b: &mut Builder, name: &str, png: Vec<u8>, strength: f64| -> usize {
        b.images.push(png);
        let t = b.images.len() - 1;
        let mut m = mat(name, [0.0; 3], 0.12).emit([1.0; 3], strength);
        m.emissive_texture = Some(t);
        b.material(m)
    };

    let (top, bottom, left, right);
    match &p.parts {
        Parts::New(n) => {
            let [x0, y0, x1, y1] = n.face;
            let pts = b.rrect(x0, y0, x1 - x0, y1 - y0, [22.0; 4]);
            b.prism(pts, -BODY_T, 0.0, m_body, 2.2, 4);
            let [wx0, wy0, wx1, wy1] = p.wing;
            let pts = b.rrect(wx0, wy0, wx1 - wx0 + 20.0, wy1 - wy0, [26.0, 0.0, 0.0, 26.0]);
            b.prism(pts, -BODY_T + 1.5, WING_Z, m_body, 2.0, 4);
            for &([x, y, w, h], r) in &p.cups {
                let pts = b.rrect(x, y, w, h, r);
                b.prism(pts, WING_Z - 0.2, WING_Z + 0.15, m_cup, 0.0, 0);
            }
            let split = n.split;
            let pts = b.rrect(x0, y0, x1 - x0, split - y0 - 0.8, [22.0, 22.0, 0.0, 0.0]);
            b.prism(pts, -1.0, panel_z, m_gloss, 0.5, 3);
            let pts = b.rrect(x0, split + 0.8, x1 - x0, y1 - split - 0.8, [0.0, 0.0, 22.0, 22.0]);
            b.prism(pts, -1.0, panel_z, m_matte, 0.5, 3);
            let m_screen = screen(&mut b, "screen", tex.screen.clone(), 1.0);
            b.plane(p.display, panel_z + 0.03, m_screen);
            let [mx, my, mw, mh] = n.mwi;
            let m_mwi = b.material(mat("message lamp", [0.5, 0.02, 0.02], 0.25).emit([1.0, 0.07, 0.04], 2.2));
            let pts = b.rrect(mx, my - 2.2, mw, mh + 3.5, [1.5; 4]);
            b.prism(pts, -3.5, panel_z + 0.25, m_mwi, 0.3, 3);
            for k in p.keys.iter().filter(|k| !k.bare) {
                let m = fill(&mut b, k.fill);
                b.key(k, panel_z, m);
            }
            if let Some(page) = p.keys.iter().find(|k| k.id == "page") {
                let png = tex.rocker.clone().expect("the rocker's print");
                b.images.push(png);
                let mut m = mat("page rocker", [1.0; 3], 0.30).metal(0.35);
                m.base_texture = Some(b.images.len() - 1);
                let m = b.material(m);
                let m_side = fill(&mut b, Fill::Silver);
                b.rocker(page, panel_z - 0.5, panel_z + KEY_H - 0.45, panel_z + KEY_H + 0.15, m, m_side);
            }
            bars(&mut b, panel_z, m_key, m_tick);
            let [ncx, ncy, nr] = n.nav_disc;
            let pts = b.face(&geom::circle(ncx, ncy, nr, 64));
            b.prism(pts, panel_z - 0.5, panel_z + KEY_H, m_key, 0.6, 4);
            let pts = b.face(&geom::circle(ncx, ncy, nr + 3.5, 96));
            b.prism(pts, panel_z - 0.5, panel_z + 0.25, m_cup, 0.2, 3);
            for &[lx0, ly0, _, ly1] in &p.lines {
                let pts = b.rrect(lx0 - 0.6, ly0, 1.2, ly1 - ly0, [0.0; 4]);
                b.prism(pts, panel_z - 0.2, panel_z + 0.02, m_tick, 0.0, 0);
            }
            (top, bottom, left, right) = (y0, y1, x0, x1);
        }
        Parts::Touch(t) => {
            let [x0, y0, x1, y1, r] = t.face;
            let pts = b.rrect(x0, y0, x1 - x0, y1 - y0, [r; 4]);
            b.prism(pts, -BODY_T, 0.0, m_body, 2.2, 4);
            let pts = b.face(&t.cradle);
            b.prism(pts, -BODY_T + 1.5, WING_Z, m_body, 2.0, 4);
            for &[x, y, w, h] in &t.grille {
                let pts = b.rrect(x, y, w, h, [w / 2.0 - 0.01; 4]);
                b.prism(pts, WING_Z - 0.2, WING_Z + 0.15, m_cup, 0.0, 0);
            }
            let i = t.glass_inset;
            let pts = b.rrect(x0 + i, y0 + i, x1 - x0 - 2.0 * i, y1 - y0 - 2.0 * i, [r - i; 4]);
            b.prism(pts, -1.0, panel_z, m_gloss, 0.5, 3);
            let m_screen = screen(&mut b, "screen", tex.screen.clone(), 1.0);
            b.plane(p.display, panel_z + 0.03, m_screen);
            let [mx, my, mw, mh] = t.mwi;
            let m_mwi = b.material(mat("message lamp", [0.5, 0.02, 0.02], 0.25).emit([1.0, 0.07, 0.04], 2.2));
            let pts = b.rrect(mx, my - 2.2, mw, mh + 3.5, [1.5; 4]);
            b.prism(pts, -3.5, panel_z + 0.25, m_mwi, 0.3, 3);
            (top, bottom, left, right) = (y0, y1, x0, x1);
        }
        Parts::Old(o) => {
            let pts = b.face(&o.body);
            b.prism(pts, -BODY_T, 0.0, m_body, 2.2, 4);
            let [wx0, wy0, wx1, wy1] = p.wing;
            let pts = b.rrect(wx0, wy0, wx1 - wx0, wy1 - wy0, [24.0, 0.0, 0.0, 24.0]);
            b.prism(pts, -BODY_T + 1.5, WING_Z, m_body, 2.0, 4);
            for &([x, y, w, h], r) in &p.cups {
                let pts = b.rrect(x, y, w, h, r);
                b.prism(pts, WING_Z - 0.2, WING_Z + 0.15, m_cup, 0.0, 0);
            }
            let [hx, hy, hw, hh, hr] = o.hook;
            let pts = b.rrect(hx, hy, hw, hh, [hr; 4]);
            b.prism(pts, WING_Z - 0.2, WING_Z + 1.2, m_cup, 0.3, 3);
            // The housing: a block whose back reaches into the body, and
            // whose top stands above the body's top edge.
            let m_housing = b.material(mat("display housing", srgb("#c9ccd0"), 0.26).metal(0.55).coat(0.6));
            let pts = b.face(&o.housing);
            b.prism(pts, -12.0, HOUSING_Z, m_housing, 1.6, 4);
            let m_lens = b.material(mat("message lens", [0.06, 0.01, 0.01], 0.15).coat(1.0).emit([0.9, 0.05, 0.03], 0.35));
            let pts = b.face(&o.mwi);
            b.prism(pts, HOUSING_Z - 1.2, HOUSING_Z + 0.08, m_lens, 0.3, 3);
            let m_bezel = b.material(mat("display bezel", srgb("#2b2d2f"), 0.35).coat(0.4));
            let [bx, by, bw, bh, br] = o.bezel;
            let pts = b.rrect(bx, by, bw, bh, [br; 4]);
            b.prism(pts, HOUSING_Z - 0.5, HOUSING_Z + 0.2, m_bezel, 0.3, 3);
            // A gray display is a backlit LCD: less bright than the color
            // panel.
            let m_screen = screen(&mut b, "screen", tex.screen.clone(), 0.85);
            b.plane(p.display, HOUSING_Z + 0.23, m_screen);
            for k in &p.keys {
                let base = if k.id.starts_with("line") || k.id.starts_with("sk") { HOUSING_Z } else { panel_z };
                let m = fill(&mut b, k.fill);
                b.key(k, base, m);
            }
            bars(&mut b, panel_z, m_key, m_tick);
            let m_ring = b.material(mat("pad ring", srgb("#26282c"), 0.5));
            let [rcx, rcy, rr] = o.ring;
            let pts = b.face(&geom::circle(rcx, rcy, rr, 96));
            b.prism(pts, panel_z - 0.5, panel_z + 0.2, m_ring, 0.3, 3);
            let hb = o.housing_box;
            let m_line = b.material(mat("line print", srgb("#8b9097"), 0.5));
            for &[lx0, ly0, lx1, ly1] in &p.lines {
                let z = if hb[1] <= ly0 && ly0 <= hb[3] && lx0 <= hb[2] { HOUSING_Z } else { panel_z };
                let pts = if ly0 == ly1 {
                    b.rrect(lx0.min(lx1), ly0 - 0.6, (lx1 - lx0).abs(), 1.2, [0.0; 4])
                } else {
                    b.rrect(lx0 - 0.6, ly0.min(ly1), 1.2, (ly1 - ly0).abs(), [0.0; 4])
                };
                b.prism(pts, z - 0.2, z + 0.03, if z > 0.0 { m_line } else { m_tick }, 0.0, 0);
            }
            if let Some([fx0, fy0, fw, fh, fr]) = o.side_frame {
                let m_frame = b.material(mat("window frame", [0.008, 0.008, 0.009], 0.3).coat(0.5));
                let pts = b.rrect(fx0, fy0, fw, fh, [fr; 4]);
                b.prism(pts, panel_z - 0.4, panel_z + 0.25, m_frame, 0.2, 3);
                if !o.paper.is_empty() {
                    // Paper label strips under a clear window: the strips
                    // touch, so that the card reads as rows of solid white
                    // and light gray.
                    for &([px, py, pw, ph], color) in &o.paper {
                        let m = b.material(mat(&format!("label {color}"), srgb(color), 0.85));
                        let pts = b.rrect(px, py, pw, ph, [0.0; 4]);
                        b.prism(pts, panel_z + 0.2, panel_z + 0.3, m, 0.0, 0);
                    }
                    let mut window = mat("label window", [0.95, 0.96, 0.97], 0.2);
                    window.alpha = 0.05;
                    let m = b.material(window);
                    let pts = b.rrect(fx0 + 2.0, fy0 + 2.0, fw - 4.0, fh - 4.0, [1.0; 4]);
                    b.prism(pts, panel_z + 0.35, panel_z + 0.75, m, 0.15, 3);
                }
            }
            if let Some(side) = p.side {
                let m = screen(&mut b, "side screen", tex.side_screen.clone().expect("the side display's picture"), 0.85);
                b.plane(side, panel_z + 0.28, m);
            }
            let (lo, hi) = crate::panel::x_range(&o.body);
            (top, bottom, left, right) = (o.body_top, o.body_bottom, lo, hi);
        }
    }

    // The legends, as one print floating on the key tops.
    if let Some(Print { png, at: [ex0, ey0, ex1, ey1] }) = tex.legends {
        b.images.push(png);
        let mut legends = mat("legends", [1.0; 3], 0.6);
        legends.base_texture = Some(b.images.len() - 1);
        legends.casts_shadow = false;
        let m_legends = b.material(legends);
        b.plane([ex0, ey0, ex1 - ex0, ey1 - ey0], panel_z + KEY_H + 0.04, m_legends);
    }

    let mouth = b.handset(m_handset, m_cup);

    // The stand, behind the body, down to the desk.  The body's lower back
    // edge rests on the desk, and the stand is narrower than the body and
    // wholly behind the body's lower edge: from the front, the body hides
    // the stand.
    let (s, c) = TILT_DEG.to_radians().sin_cos();
    let desk_z = b.fy(bottom) * s + (-BODY_T) * c;
    let body = [b.fx(left), b.fy(bottom), b.fx(right), b.fy(top)];
    if new {
        b.tent(body, desk_z, m_body, m_cup);
    } else {
        b.aframe(body, desk_z, m_body);
    }

    b.cord(mouth, desk_z, m_cord);
    Model { materials: b.materials, images: b.images, parts: b.parts, desk_z }
}

/// The joined bars (the volume keys, and on the new family the select and
/// cancel keys) and the lines across them.
fn bars(b: &mut Builder, panel_z: f64, m_key: usize, m_tick: usize) {
    for &[bx, by, bw, bh, br] in &b.p.bars.clone() {
        let pts = b.rrect(bx, by, bw, bh, [br - 0.01; 4]);
        b.prism(pts, panel_z - 0.5, panel_z + KEY_H, m_key, 0.45, 3);
    }
    for &[sx, sy0, sy1] in &b.p.bar_splits.clone() {
        let pts = b.rrect(sx - 0.5, sy0, 1.0, sy1 - sy0, [0.0; 4]);
        b.prism(pts, panel_z + KEY_H - 0.1, panel_z + KEY_H + 0.03, m_tick, 0.0, 0);
    }
}

/// A rectangle in face millimeters, with rounded corners.
fn face_box(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Vec<P2> {
    geom::rrect(x0, y0, x1 - x0, y1 - y0, [r; 4], 6)
}
