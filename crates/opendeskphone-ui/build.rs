//! Build-time asset pipeline: rasterizes the TrueType fonts in
//! `assets/fonts` into glyph atlases (color skin), converts the bitmap
//! fonts in `assets/fonts/bitmap` into one-bit glyph tables (gray skins),
//! and rasterizes the SVG artwork in `assets/icons` and `assets/art` into
//! per-skin bitmaps, so the phone renders with integer blits only.
//! (Floating point is used here, on the build host.)
//!
//! Output: `$OUT_DIR/assets_gen.rs` plus one blob per table.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

const SKINS: [&str; 4] = ["SMALL_GRAY", "LARGE_GRAY", "COLOR", "TOUCH"];
/// The skins' names in asset file names (`splash.small-gray.svg`).
const SKIN_FILE_TAGS: [&str; 4] = ["small-gray", "large-gray", "color", "touch"];

/// (font file, pixel size, digits only) per skin, in role order: Normal,
/// Bold, Small, Title, Large, then the two idle-screen roles: Clock (the
/// big time; digits and the colon only), Sub (meridiem and date), Number
/// (the number line of a call card), and Meridiem (AM/PM beside the clock).
///
/// Color sizes follow the stock screenshots: 14 px body text (cap height
/// 10), a 16 px title, and a clock whose digits are 64 px tall.
///
/// The gray skins (the first two rows) take bitmap fonts, drawn pixel for
/// pixel in solid levels: on a 240x120 four-level panel, scaled outline
/// text is blurry.  A file name ending in `.bdf` is a bitmap font in
/// `assets/fonts/bitmap` (the pixel size here is then only a note: a
/// bitmap font has one size).  The faces are the X11 Adobe Helvetica
/// bitmaps: proportional, with digits of one width; body text has a cap
/// height of 8 pixels on the small panels (stock's 10 px text: 7) and 9
/// on the D70.  The clock is the largest size there is (digits 25 pixels
/// tall).
const FONTS: [[(&str, f32, bool); 9]; 4] = [
    [
        ("helvR10-75dpi.bdf", 10.0, false),
        ("helvB10-75dpi.bdf", 10.0, false),
        ("helvR08-75dpi.bdf", 8.0, false),
        ("helvB10-75dpi.bdf", 10.0, false),
        ("helvB18-75dpi.bdf", 18.0, false),
        ("helvB24-100dpi.bdf", 34.0, true),
        ("helvR10-75dpi.bdf", 10.0, false),
        ("helvR10-75dpi.bdf", 10.0, false),
        ("helvR10-75dpi.bdf", 10.0, false),
    ],
    [
        ("helvR12-75dpi.bdf", 12.0, false),
        ("helvB12-75dpi.bdf", 12.0, false),
        ("helvR10-75dpi.bdf", 10.0, false),
        ("helvB12-75dpi.bdf", 12.0, false),
        ("helvB24-75dpi.bdf", 24.0, false),
        ("helvB24-100dpi.bdf", 34.0, true),
        ("helvR12-75dpi.bdf", 12.0, false),
        ("helvR12-75dpi.bdf", 12.0, false),
        ("helvR12-75dpi.bdf", 12.0, false),
    ],
    [
        ("Roboto-Regular.ttf", 14.0, false),
        ("Roboto-Medium.ttf", 15.0, false),
        ("Roboto-Regular.ttf", 12.0, false),
        ("Roboto-Bold.ttf", 16.0, false),
        ("Roboto-Regular.ttf", 30.0, false),
        ("Roboto-Thin.ttf", 89.0, true),
        ("Roboto-Light.ttf", 22.0, false),
        ("Roboto-Regular.ttf", 17.0, false),
        ("Roboto-Regular.ttf", 22.0, false),
    ],
    // The touch skin (D80, about 216 pixels to the inch): about twice the
    // color skin's sizes.  The clock is as large as a glyph table holds
    // (`GlyphRec` and `FontData` keep a font's ascent in eight bits).
    [
        ("Roboto-Regular.ttf", 28.0, false),
        ("Roboto-Medium.ttf", 30.0, false),
        ("Roboto-Regular.ttf", 24.0, false),
        ("Roboto-Bold.ttf", 32.0, false),
        ("Roboto-Regular.ttf", 60.0, false),
        ("Roboto-Thin.ttf", 130.0, true),
        ("Roboto-Light.ttf", 44.0, false),
        ("Roboto-Regular.ttf", 34.0, false),
        ("Roboto-Regular.ttf", 44.0, false),
    ],
];

/// The fallback of each bitmap face: a fixed-width X11 "misc" font of
/// about the same height with far more characters (Greek, Cyrillic,
/// Hebrew, Armenian, Georgian, symbols, ...), used for one character at a
/// time when the face lacks it.  A character neither has is drawn as the
/// fallback's replacement character (U+FFFD).  None of these fonts has
/// Chinese, Japanese, or Korean.
const BITMAP_FALLBACKS: [(&str, &str); 8] = [
    ("helvR08-75dpi.bdf", "6x10.bdf"),
    ("helvR10-75dpi.bdf", "6x10.bdf"),
    ("helvB10-75dpi.bdf", "6x10.bdf"),
    ("helvR12-75dpi.bdf", "6x13.bdf"),
    ("helvB12-75dpi.bdf", "6x13.bdf"),
    ("helvB18-75dpi.bdf", "6x13.bdf"),
    ("helvB24-75dpi.bdf", "6x13.bdf"),
    ("helvB24-100dpi.bdf", "6x13.bdf"),
];

/// Asset classes: directory, pixel height per skin, and whether the color
/// and touch skins keep color (else an alpha mask tinted at draw time).
/// The last flag inverts luminance on the gray skins (artwork drawn for a
/// dark background, shown on the light gray screens).
///
/// A file named `<name>.<skin>.svg` (`small-gray`, `large-gray`, `color`,
/// or `touch`) replaces `<name>.svg` on that skin.  It is drawn for that
/// skin's pixels: it is rendered at its own size, and never inverted.
const CLASSES: [(&str, [u32; 4], bool, bool); 3] = [
    ("icons/status", [13, 15, 22, 44], false, false),
    ("icons/app", [24, 28, 48, 96], true, false),
    ("art", [44, 60, 104, 150], true, true),
];

const FMT_MASK4: u8 = 0;
const FMT_GRAY2: u8 = 1;
const FMT_RGBA_RLE: u8 = 2;

fn charset() -> Vec<char> {
    let mut v: Vec<char> = (32u32..127).chain(160..256).filter_map(char::from_u32).collect();
    v.extend(['\u{2013}', '\u{2014}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2026}']);
    v.sort();
    v
}

struct Nibbles(Vec<u8>, bool);
impl Nibbles {
    fn new() -> Self {
        Nibbles(Vec::new(), false)
    }
    fn push(&mut self, n: u8) {
        if self.1 {
            *self.0.last_mut().unwrap() |= n & 15;
        } else {
            self.0.push((n & 15) << 4);
        }
        self.1 = !self.1;
    }
    /// Byte-aligns and returns the bytes.
    fn finish(self) -> Vec<u8> {
        self.0
    }
}

fn gen_font(out: &mut String, blobs: &Path, assets: &Path, name: &str, file: &str, px: f32, digits: bool) {
    let data = fs::read(assets.join("fonts").join(file)).expect("font file");
    let font = fontdue::Font::from_bytes(data, fontdue::FontSettings { scale: px, ..Default::default() })
        .expect("font parse");
    let lm = font.horizontal_line_metrics(px).expect("line metrics");
    let ascent = lm.ascent.round() as i32;
    let descent = (-lm.descent).round() as i32;
    let mut blob: Vec<u8> = Vec::new();
    let mut recs = String::new();
    for ch in charset() {
        if digits && !(ch.is_ascii_digit() || matches!(ch, ':' | ' ' | '-')) {
            continue;
        }
        if font.lookup_glyph_index(ch) == 0 && ch != ' ' {
            continue;
        }
        let (m, cov) = font.rasterize(ch, px);
        let off = blob.len();
        let mut nib = Nibbles::new();
        for &c in &cov {
            nib.push(((c as u32 * 15 + 127) / 255) as u8);
        }
        blob.extend(nib.finish());
        let top = m.ymin + m.height as i32; // above baseline
        let adv = m.advance_width.round() as i32;
        writeln!(
            recs,
            "    GlyphRec {{ ch: {}, adv: {}, xoff: {}, top: {}, w: {}, h: {}, off: {} }},",
            ch as u32, adv, m.xmin, top, m.width, m.height, off
        )
        .unwrap();
    }
    let blob_path = blobs.join(format!("{name}.bin"));
    fs::write(&blob_path, &blob).unwrap();
    writeln!(
        out,
        "pub static {name}: FontData = FontData {{ px: {}, ascent: {ascent}, descent: {descent}, bits: 4, fallback: None, glyphs: &[\n{recs}], data: include_bytes!({:?}) }};",
        px as u32,
        blob_path
    )
    .unwrap();
}

/// Renders an SVG `height` pixels tall, or at its own size.
/// Converts a BDF bitmap font into a one-bit glyph table: each glyph's
/// rows as in the file, most significant bit first, each row padded to a
/// byte.  (BDF is a line-oriented text format; the few keywords read here
/// are all a font needs.)
fn gen_bitmap_font(out: &mut String, blobs: &Path, assets: &Path, name: &str, file: &str, digits: bool, fallback: Option<&str>) {
    let path = assets.join("fonts/bitmap").join(file);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let num = |w: Option<&str>| -> i32 { w.and_then(|w| w.parse().ok()).unwrap_or_else(|| panic!("{}: bad number", path.display())) };
    let (mut px, mut ascent, mut descent) = (0, 0, 0);
    let mut blob: Vec<u8> = Vec::new();
    // (code point, record line)
    let mut recs: Vec<(u32, String)> = Vec::new();
    let (mut ch, mut adv, mut bbx) = (-1, 0, [0i32; 4]);
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let mut words = line.split_ascii_whitespace();
        match words.next() {
            Some("PIXEL_SIZE") => px = num(words.next()),
            Some("FONT_ASCENT") => ascent = num(words.next()),
            Some("FONT_DESCENT") => descent = num(words.next()),
            Some("ENCODING") => ch = num(words.next()),
            Some("DWIDTH") => adv = num(words.next()),
            Some("BBX") => bbx = [num(words.next()), num(words.next()), num(words.next()), num(words.next())],
            Some("BITMAP") => {
                let [w, h, xoff, yoff] = bbx;
                let row_bytes = (w as usize).div_ceil(8);
                let off = blob.len();
                for _ in 0..h {
                    let row = lines.next().unwrap_or_else(|| panic!("{}: short bitmap", path.display())).trim();
                    for i in 0..row_bytes {
                        let byte = row.get(i * 2..i * 2 + 2).and_then(|b| u8::from_str_radix(b, 16).ok());
                        blob.push(byte.unwrap_or_else(|| panic!("{}: bad bitmap row {row:?}", path.display())));
                    }
                }
                // (The glyph tables hold code points of the Basic
                // Multilingual Plane, which is all these fonts have.)
                let keep = match u16::try_from(ch).ok().and_then(|c| char::from_u32(c as u32)) {
                    // (Code point 0 is some fonts' own "missing" box.)
                    Some(c) if digits => c.is_ascii_digit() || matches!(c, ':' | ' ' | '-'),
                    Some(c) => c >= ' ',
                    None => false,
                };
                if keep {
                    recs.push((
                        ch as u32,
                        format!("    GlyphRec {{ ch: {ch}, adv: {adv}, xoff: {xoff}, top: {}, w: {w}, h: {h}, off: {off} }},\n", yoff + h),
                    ));
                } else {
                    blob.truncate(off);
                }
            }
            _ => {}
        }
    }
    assert!(px > 0 && !recs.is_empty(), "{}: not a BDF font", path.display());
    recs.sort();
    let recs: String = recs.into_iter().map(|(_, r)| r).collect();
    let blob_path = blobs.join(format!("{name}.bin"));
    fs::write(&blob_path, &blob).unwrap();
    let fallback = fallback.map_or("None".to_string(), |f| format!("Some(&{f})"));
    writeln!(
        out,
        "pub static {name}: FontData = FontData {{ px: {px}, ascent: {ascent}, descent: {descent}, bits: 1, fallback: {fallback}, glyphs: &[\n{recs}], data: include_bytes!({blob_path:?}) }};",
    )
    .unwrap();
}

fn render_svg(path: &Path, height: Option<u32>, opt: &resvg::usvg::Options) -> (u32, u32, Vec<u8>) {
    let data = fs::read(path).unwrap();
    let tree = resvg::usvg::Tree::from_data(&data, opt)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let size = tree.size();
    let height = height.unwrap_or(size.height().round().max(1.0) as u32);
    let scale = height as f32 / size.height();
    let width = (size.width() * scale).round().max(1.0) as u32;
    let mut pm = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    // Un-premultiply.
    let mut px = pm.take();
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    (width, height, px)
}

/// `on_white` (gray artwork): partly covered pixels are blended with the
/// white background the artwork is shown on before the four levels are
/// chosen, so edges are anti-aliased through the two middle grays; only
/// untouched pixels stay transparent.  Without it (app icons, which also
/// sit on selected, black rows) a pixel is either opaque or transparent.
fn encode(rgba: &[u8], fmt: u8, invert: bool, on_white: bool) -> Vec<u8> {
    match fmt {
        FMT_MASK4 => {
            let mut n = Nibbles::new();
            for p in rgba.as_chunks::<4>().0 {
                n.push(((p[3] as u32 * 15 + 127) / 255) as u8);
            }
            n.finish()
        }
        FMT_GRAY2 => {
            let mut n = Nibbles::new();
            for p in rgba.as_chunks::<4>().0 {
                let a = p[3] as u32;
                if (on_white && a == 0) || (!on_white && a < 128) {
                    n.push(15);
                } else {
                    let lum = (p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8;
                    let lum = if invert { 255 - lum } else { lum };
                    let lum = if on_white { (lum * a + 255 * (255 - a) + 127) / 255 } else { lum };
                    n.push(((lum * 3 + 127) / 255) as u8);
                }
            }
            n.finish()
        }
        _ => {
            let mut out = Vec::new();
            let mut it = rgba.as_chunks::<4>().0.iter().peekable();
            while let Some(p) = it.next() {
                let mut run = 1u8;
                while run < 255 && it.peek().is_some_and(|q| *q == p || (p[3] == 0 && q[3] == 0)) {
                    it.next();
                    run += 1;
                }
                out.push(run);
                out.extend_from_slice(if p[3] == 0 { &[0, 0, 0, 0] } else { p });
            }
            out
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest.join("../../assets");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", assets.display());

    let mut out = String::from("// Generated by build.rs; do not edit.\nuse crate::font::{FontData, GlyphRec};\nuse crate::icons::IconRec;\n");

    // The bitmap fonts' fallbacks, once each.
    let mut fallbacks: Vec<(&str, String)> = Vec::new();
    for (_, file) in BITMAP_FALLBACKS {
        if !fallbacks.iter().any(|f| f.0 == file) {
            let name = format!("FONT_FALLBACK_{}", fallbacks.len());
            gen_bitmap_font(&mut out, &out_dir, &assets, &name, file, false, None);
            fallbacks.push((file, name));
        }
    }

    // Fonts (deduplicated by file and size; the outline fonts' tables are
    // named after their skin).
    let mut done: Vec<(&str, u32, bool, String)> = Vec::new();
    for (si, skin) in SKINS.iter().enumerate() {
        let mut names: Vec<String> = Vec::new();
        for (file, px, digits) in FONTS[si] {
            let key = (file, px as u32, digits);
            let name = match done.iter().find(|d| (d.0, d.1, d.2) == key) {
                Some(d) => d.3.clone(),
                None => {
                    let name = format!("FONT_{skin}_{}", done.len());
                    if file.ends_with(".bdf") {
                        let fallback = BITMAP_FALLBACKS.iter().find(|f| f.0 == file).map(|f| f.1);
                        let fallback = fallback.and_then(|f| fallbacks.iter().find(|n| n.0 == f)).map(|n| n.1.as_str());
                        gen_bitmap_font(&mut out, &out_dir, &assets, &name, file, digits, fallback);
                    } else {
                        gen_font(&mut out, &out_dir, &assets, &name, file, px, digits);
                    }
                    done.push((file, px as u32, digits, name.clone()));
                    name
                }
            };
            names.push(format!("&{name}"));
        }
        writeln!(out, "pub static FONTS_{skin}: [&FontData; 9] = [{}];", names.join(", ")).unwrap();
    }

    // Icons and art.
    let mut opt = resvg::usvg::Options::default();
    {
        let db = opt.fontdb_mut();
        for f in fs::read_dir(assets.join("fonts")).unwrap().flatten() {
            if f.path().extension().is_some_and(|e| e == "ttf") {
                db.load_font_data(fs::read(f.path()).unwrap());
            }
        }
    }
    for (si, skin) in SKINS.iter().enumerate() {
        let mut blob: Vec<u8> = Vec::new();
        let mut recs = String::new();
        // (name, file, pixel height or `None` for the file's own, format,
        // invert, blend with white).
        let mut entries: Vec<(String, PathBuf, Option<u32>, u8, bool, bool)> = Vec::new();
        for (dir, heights, colored, invert) in CLASSES {
            let d = assets.join(dir);
            let Ok(rd) = fs::read_dir(&d) else { continue };
            let files: Vec<PathBuf> = rd.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "svg")).collect();
            for p in &files {
                let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
                // A file for one skin: used on that skin only, where it
                // replaces the file for every skin.
                let (stem, own) = match stem.split_once('.') {
                    Some((base, tag)) if tag == SKIN_FILE_TAGS[si] => (base.to_string(), true),
                    Some(_) => continue,
                    None if files.iter().any(|q| q.file_name().is_some_and(|n| *n == *format!("{stem}.{}.svg", SKIN_FILE_TAGS[si]))) => continue,
                    None => (stem, false),
                };
                // App icons get their own namespace: `app-contacts` beside
                // the monochrome `contacts`.
                let name = if dir == "icons/app" { format!("app-{stem}") } else { stem };
                let fmt = match (colored, si) {
                    (false, _) => FMT_MASK4,
                    (true, 2 | 3) => FMT_RGBA_RLE,
                    (true, _) => FMT_GRAY2,
                };
                let height = if own { None } else { Some(heights[si]) };
                entries.push((name, p.clone(), height, fmt, invert && !own, dir == "art"));
            }
        }
        entries.sort();
        for (name, path, h, fmt, invert, on_white) in entries {
            let (w, h, rgba) = render_svg(&path, h, &opt);
            let enc = encode(&rgba, fmt, invert, on_white);
            writeln!(
                recs,
                "    IconRec {{ name: {name:?}, w: {w}, h: {h}, fmt: {fmt}, off: {}, len: {} }},",
                blob.len(),
                enc.len()
            )
            .unwrap();
            blob.extend(enc);
        }
        let blob_path = out_dir.join(format!("icons_{skin}.bin"));
        fs::write(&blob_path, &blob).unwrap();
        writeln!(
            out,
            "pub static ICONS_{skin}: &[IconRec] = &[\n{recs}];\npub static ICON_DATA_{skin}: &[u8] = include_bytes!({blob_path:?});"
        )
        .unwrap();
    }
    fs::write(out_dir.join("assets_gen.rs"), out).unwrap();
}
