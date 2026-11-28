//! Makes the two faces' tables of the digits, `:`, and `.`: Roboto Thin,
//! rasterized once, large, for the app to shrink to the size it draws it
//! at on the color displays; and the Helvetica bitmap font that the gray
//! displays' clock is in, pixel for pixel.

use std::fmt::Write;
use std::path::Path;

/// The size Roboto is rasterized at: a cell is then as tall as a canvas's
/// bitmap may be (256 pixels).
const PX: f32 = 280.0;
const CHARS: &str = "0123456789:.";

/// One character: its advance, and its coverage (0 to 255, row by row)
/// placed by its left edge and its bottom edge above the baseline.
struct Raster {
    advance: i32,
    xmin: i32,
    ymin: i32,
    w: usize,
    h: usize,
    coverage: Vec<u8>,
}

/// The table of a face named `name`, with its coverage in `<file>.bin`
/// of `out`.  Each cell has a tenth of the characters' height above and
/// below them.
fn table(name: &str, file: &str, scales: bool, rasters: &[Raster], out: &Path) -> String {
    let top = rasters.iter().map(|r| r.ymin + r.h as i32).max().unwrap();
    let bottom = rasters.iter().map(|r| r.ymin).min().unwrap();
    let pad = (top - bottom) / 10;
    let height = top - bottom + 2 * pad;
    assert!(height <= 256, "{name}: a cell {height} pixels tall");
    let mut rs = format!(
        "pub static {name}: Face = Face {{\n    scales: {scales},\n    height: {height},\n    \
         coverage: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}.bin\")),\n    glyphs: &[\n"
    );
    let mut coverage = Vec::new();
    for (c, r) in CHARS.chars().zip(rasters) {
        let (x, y) = (r.xmin, top + pad - (r.ymin + r.h as i32));
        assert!(
            x >= 0 && x + r.w as i32 <= r.advance,
            "{name}: {c:?} lies outside its cell"
        );
        writeln!(
            rs,
            "        Glyph {{ ch: {c:?}, advance: {}, x: {x}, y: {y}, w: {}, h: {}, at: {} }},",
            r.advance,
            r.w,
            r.h,
            coverage.len()
        )
        .unwrap();
        coverage.extend_from_slice(&r.coverage);
    }
    rs.push_str("    ],\n};\n");
    std::fs::write(out.join(format!("{file}.bin")), coverage).unwrap();
    rs
}

fn read(fonts: &Path, file: &str) -> Vec<u8> {
    let path = fonts.join(file);
    println!("cargo::rerun-if-changed={}", path.display());
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The characters of [`CHARS`] in a BDF bitmap font, which is a
/// line-oriented text format; the few keywords read here are all a
/// glyph needs.
fn bitmap_font(text: &str) -> Vec<Raster> {
    let num = |w: Option<&str>| -> i32 { w.and_then(|w| w.parse().ok()).expect("a number") };
    let mut found: Vec<(char, Raster)> = Vec::new();
    let (mut ch, mut advance, mut bbx) = (-1, 0, [0i32; 4]);
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let mut words = line.split_ascii_whitespace();
        match words.next() {
            Some("ENCODING") => ch = num(words.next()),
            Some("DWIDTH") => advance = num(words.next()),
            Some("BBX") => bbx = [(); 4].map(|_| num(words.next())),
            Some("BITMAP") => {
                let [w, h] = [bbx[0], bbx[1]].map(|n| n as usize);
                let mut coverage = Vec::with_capacity(w * h);
                for _ in 0..h {
                    let row = lines.next().expect("a bitmap row").trim();
                    let bits = u128::from_str_radix(row, 16).expect("a row in hex");
                    let len = 4 * row.len();
                    coverage.extend((0..w).map(|x| {
                        if bits >> (len - 1 - x) & 1 == 1 {
                            255
                        } else {
                            0
                        }
                    }));
                }
                let c = u32::try_from(ch).ok().and_then(char::from_u32);
                if let Some(c) = c.filter(|c| CHARS.contains(*c)) {
                    found.push((
                        c,
                        Raster {
                            advance,
                            xmin: bbx[2],
                            ymin: bbx[3],
                            w,
                            h,
                            coverage,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    CHARS
        .chars()
        .map(|c| {
            let i = found
                .iter()
                .position(|(f, _)| *f == c)
                .unwrap_or_else(|| panic!("no {c:?}"));
            found.swap_remove(i).1
        })
        .collect()
}

fn main() {
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/fonts");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).to_path_buf();

    let roboto = fontdue::Font::from_bytes(
        read(&fonts, "Roboto-Thin.ttf"),
        fontdue::FontSettings::default(),
    )
    .unwrap();
    let roboto: Vec<Raster> = CHARS
        .chars()
        .map(|c| {
            let (m, coverage) = roboto.rasterize(c, PX);
            Raster {
                advance: m.advance_width.round() as i32,
                xmin: m.xmin,
                ymin: m.ymin,
                w: m.width,
                h: m.height,
                coverage,
            }
        })
        .collect();

    let helvetica = read(&fonts, "bitmap/helvB24-100dpi.bdf");
    let helvetica = bitmap_font(&String::from_utf8(helvetica).unwrap());

    let rs = table("ROBOTO", "roboto", true, &roboto, &out)
        + &table("HELVETICA", "helvetica", false, &helvetica, &out);
    std::fs::write(out.join("faces.rs"), rs).unwrap();
}
