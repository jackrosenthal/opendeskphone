//! This crate renders SVG artwork to pixels.  The build script of
//! `opendeskphone-ui` renders the phone's icons and artwork with it, and
//! the macro `glyph!` of `opendeskphone-app-sdk` renders an app's glyph
//! with it, so that both come from one renderer and one quantization.

pub use resvg;
use resvg::{tiny_skia, usvg};

/// A glyph is rendered at the height in pixels of the status icons of each
/// skin, which are the small gray, large gray, color, and touch skins in
/// that order.
pub const GLYPH_HEIGHTS: [u32; 4] = [13, 15, 22, 44];

/// Renders `tree` `height` pixels tall, or at its own size.  The pixels
/// are RGBA, not premultiplied, in rows from the top.
pub fn render(tree: &usvg::Tree, height: Option<u32>) -> (u32, u32, Vec<u8>) {
    let size = tree.size();
    let height = height.unwrap_or(size.height().round().max(1.0) as u32);
    let scale = height as f32 / size.height();
    let width = (size.width() * scale).round().max(1.0) as u32;
    let mut pm = tiny_skia::Pixmap::new(width, height).unwrap();
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
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

/// Converts the alpha of RGBA pixels to four bits of coverage a pixel.
/// The phone's one-color icons and the apps' glyphs share this format,
/// which `GlyphMask` of the SDK's `proto/manifest.proto` describes.
pub fn mask4(rgba: &[u8]) -> Vec<u8> {
    let px = rgba.as_chunks::<4>().0;
    let mut out = vec![0; px.len().div_ceil(2)];
    for (i, p) in px.iter().enumerate() {
        let v = ((p[3] as u32 * 15 + 127) / 255) as u8;
        out[i / 2] |= if i % 2 == 0 { v << 4 } else { v };
    }
    out
}

/// The renderer makes one mask for each height, and its `alpha` comes from
/// [`mask4`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

/// Renders the content of an SVG file at each of [`GLYPH_HEIGHTS`], or
/// says why the file cannot be a glyph.
pub fn glyph(svg: &[u8]) -> Result<Vec<Mask>, String> {
    let text = std::str::from_utf8(svg).map_err(|_| "is not an SVG file: it is not UTF-8 text".to_string())?;
    let xml = usvg::roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = usvg::roxmltree::Document::parse_with_options(text, xml).map_err(|e| format!("is not an SVG file: {e}"))?;
    for node in doc.descendants().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "text" => return Err("has text, and a glyph has none: convert the text to paths".into()),
            "image" => return Err("has an image, and a glyph has none: draw it with paths".into()),
            _ => {}
        }
    }
    let tree = usvg::Tree::from_xmltree(&doc, &usvg::Options::default()).map_err(|e| format!("is not an SVG file: {e}"))?;
    let mut colors = Vec::new();
    paints(tree.root(), &mut colors)?;
    if let [a, b, ..] = colors[..] {
        let hex = |c: usvg::Color| format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue);
        return Err(format!(
            "paints in more than one color ({} and {}), and a glyph is one color: leave a hole where the other color would be",
            hex(a),
            hex(b)
        ));
    }
    let mut masks = Vec::new();
    for h in GLYPH_HEIGHTS {
        let (width, height, rgba) = render(&tree, Some(h));
        if width > 2 * height {
            return Err(format!("is {width} by {height} pixels, and a glyph is at most twice as wide as it is tall"));
        }
        let alpha = mask4(&rgba);
        if alpha.iter().all(|a| *a == 0) {
            return Err(format!("renders empty at {height} pixels"));
        }
        masks.push(Mask { width, height, alpha });
    }
    Ok(masks)
}

/// Collects the colors that the paths under `group` paint with, and
/// refuses a gradient or a pattern.
fn paints(group: &usvg::Group, colors: &mut Vec<usvg::Color>) -> Result<(), String> {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => paints(g, colors)?,
            usvg::Node::Path(p) => {
                for paint in p.fill().map(|f| f.paint()).into_iter().chain(p.stroke().map(|s| s.paint())) {
                    match paint {
                        usvg::Paint::Color(c) if !colors.contains(c) => colors.push(*c),
                        usvg::Paint::Color(_) => {}
                        _ => return Err("paints with a gradient or a pattern, and a glyph is one color".into()),
                    }
                }
            }
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(body: &str) -> String {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">{body}</svg>"#)
    }

    /// Returns the coverage of the pixel at (`x`, `y`) of a mask.
    fn at(m: &Mask, x: u32, y: u32) -> u8 {
        let n = (y * m.width + x) as usize;
        let byte = m.alpha[n / 2];
        if n.is_multiple_of(2) { byte >> 4 } else { byte & 15 }
    }

    #[test]
    fn coverage_is_four_bits_a_pixel_the_first_in_the_high_nibble() {
        let rgba = [0, 0, 0, 255, 9, 9, 9, 0, 1, 2, 3, 128];
        assert_eq!(mask4(&rgba), [0xf0, 0x80]);
    }

    #[test]
    fn a_glyph_has_a_mask_at_every_height() {
        let ring = svg(r##"<circle cx="12" cy="12" r="8" fill="none" stroke="#000" stroke-width="4"/>"##);
        let masks = glyph(ring.as_bytes()).unwrap();
        assert_eq!(masks.iter().map(|m| m.height).collect::<Vec<_>>(), GLYPH_HEIGHTS);
        for m in &masks {
            assert_eq!(m.width, m.height);
            assert_eq!(m.alpha.len(), (m.width * m.height).div_ceil(2) as usize);
            // The middle is a hole, and the ring is covered.
            assert_eq!(at(m, m.width / 2, m.height / 2), 0);
            assert_eq!(at(m, m.width / 2, m.height / 6), 15);
        }
    }

    #[test]
    fn a_hole_is_what_is_not_painted() {
        // A mask cuts a hole in the disk, and the mask's black and white do
        // not count as paint.
        let disk = svg(
            r##"<mask id="m"><rect width="24" height="24" fill="#fff"/><circle cx="12" cy="12" r="4"/></mask><circle cx="12" cy="12" r="10" fill="#123456" mask="url(#m)"/>"##,
        );
        let masks = glyph(disk.as_bytes()).unwrap();
        let m = &masks[3];
        assert_eq!(at(m, 22, 22), 0);
        assert_eq!(at(m, 22, 6), 15);
    }

    #[test]
    fn what_is_not_a_glyph_is_refused_with_why() {
        let refused = |body: &str| glyph(svg(body).as_bytes()).unwrap_err();
        assert!(refused(r#"<text x="2" y="20">A</text>"#).starts_with("has text"));
        assert!(refused(r#"<image href="data:image/png;base64," width="24" height="24"/>"#).starts_with("has an image"));
        assert_eq!(
            refused(r##"<rect width="10" height="10" fill="#000"/><rect x="12" width="10" height="10" fill="#ff0000"/>"##),
            "paints in more than one color (#000000 and #ff0000), and a glyph is one color: leave a hole where the other color would be"
        );
        assert!(refused(r##"<path d="M2 2h20v20z" fill="#000" stroke="#00f"/>"##).starts_with("paints in more than one color"));
        assert!(
            refused(r##"<linearGradient id="g"><stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#fff"/></linearGradient><rect width="24" height="24" fill="url(#g)"/>"##)
                .starts_with("paints with a gradient")
        );
        assert_eq!(refused(r#"<rect width="24" height="24" fill="none"/>"#), "renders empty at 13 pixels");
        assert!(glyph(b"<svg").unwrap_err().starts_with("is not an SVG file"));
        assert!(glyph(b"\xff\xfe").unwrap_err().starts_with("is not an SVG file"));
        let wide = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 50 24"><rect width="50" height="24" fill="#000"/></svg>"##;
        assert_eq!(glyph(wide.as_bytes()).unwrap_err(), "is 27 by 13 pixels, and a glyph is at most twice as wide as it is tall");
    }
}
