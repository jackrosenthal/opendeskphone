//! The recovery program's pictures, drawn here so that the program needs
//! neither a font nor an SVG renderer: the logo (the start-up artwork of
//! the D80's skin, `assets/art/splash.touch.svg`), and each line of text
//! it can show, as Android's recovery has its texts in pictures.
//!
//! Nothing is drawn without the feature `program`: the library has no
//! pictures.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "program")]
    art::draw();
}

#[cfg(feature = "program")]
mod art {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    /// The lines of text: the name of each in the program, its color, and
    /// the text.
    const TEXTS: &[(&str, &str, &str)] = &[
        ("INSTALLING", "#B4B8BE", "Installing system update\u{2026}"),
        ("NO_COMMAND", "#B4B8BE", "No update to install"),
        ("ERROR", "#EF5350", "The update was not installed"),
        ("RESTARTING", "#B4B8BE", "Restarting\u{2026}"),
    ];
    /// The width of the D80's screen, and the height of a line of text.
    const WIDTH: u32 = 800;
    const LINE: u32 = 56;

    /// The picture of an SVG document, with the transparent rows and
    /// columns around it cut off: its size, and its pixels as tiny-skia
    /// has them (red, green, blue, and alpha, the colors multiplied by
    /// the alpha).
    fn render(svg: &[u8], opt: &resvg::usvg::Options) -> (u32, u32, Vec<u8>) {
        let tree = resvg::usvg::Tree::from_data(svg, opt).expect("an SVG document");
        let size = tree.size().to_int_size();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        let (w, h) = (size.width() as usize, size.height() as usize);
        let data = pixmap.data();
        let alpha = |x: usize, y: usize| data[4 * (y * w + x) + 3];
        let rows: Vec<usize> = (0..h).filter(|y| (0..w).any(|x| alpha(x, *y) != 0)).collect();
        let columns: Vec<usize> = (0..w).filter(|x| (0..h).any(|y| alpha(*x, y) != 0)).collect();
        let (Some(top), Some(bottom), Some(left), Some(right)) = (rows.first(), rows.last(), columns.first(), columns.last()) else {
            panic!("a picture with nothing in it");
        };
        let mut out = Vec::new();
        for y in *top..=*bottom {
            out.extend_from_slice(&data[4 * (y * w + left)..4 * (y * w + right + 1)]);
        }
        ((right - left + 1) as u32, (bottom - top + 1) as u32, out)
    }

    pub fn draw() {
        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut opt = resvg::usvg::Options::default();
        let fonts = assets.join("fonts");
        println!("cargo:rerun-if-changed={}", fonts.display());
        for file in std::fs::read_dir(&fonts).unwrap().flatten() {
            if file.path().extension().is_some_and(|e| e == "ttf") {
                opt.fontdb_mut().load_font_data(std::fs::read(file.path()).unwrap());
            }
        }

        let mut code = String::new();
        let mut picture = |name: &str, svg: &[u8]| {
            let (w, h, pixels) = render(svg, &opt);
            let file = out_dir.join(format!("{}.rgba", name.to_lowercase()));
            std::fs::write(&file, pixels).unwrap();
            writeln!(code, "pub static {name}: Picture = Picture {{ width: {w}, height: {h}, pixels: include_bytes!({file:?}) }};").unwrap();
        };
        let logo = assets.join("art/splash.touch.svg");
        println!("cargo:rerun-if-changed={}", logo.display());
        picture("LOGO", &std::fs::read(&logo).unwrap());
        for (name, color, text) in TEXTS {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} {LINE}\" width=\"{WIDTH}\" height=\"{LINE}\">\
                 <text x=\"{}\" y=\"40\" text-anchor=\"middle\" font-family=\"Roboto\" font-weight=\"400\" font-size=\"34\" fill=\"{color}\">{text}</text></svg>",
                WIDTH / 2
            );
            picture(name, svg.as_bytes());
        }
        std::fs::write(out_dir.join("art.rs"), code).unwrap();
    }
}
