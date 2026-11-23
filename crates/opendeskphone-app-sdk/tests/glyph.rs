//! These tests check that `glyph!` makes the masks that the renderer
//! makes, and that the manifest's section carries them.

use opendeskphone_app_sdk::manifest::{Glyph, Manifest};

const SUN: Glyph = opendeskphone_app_sdk::glyph!("glyph/sun.svg");

#[test]
fn the_glyph_is_what_the_renderer_makes_of_the_file() {
    let rendered = opendeskphone_svg::glyph(include_bytes!("glyph/sun.svg")).unwrap();
    let masks: Vec<_> = SUN.masks.iter().map(|m| (m.width, m.height, m.alpha.to_vec())).collect();
    let expected: Vec<_> = rendered.into_iter().map(|m| (m.width, m.height, m.alpha)).collect();
    assert_eq!(masks, expected);
    let heights: Vec<u32> = SUN.masks.iter().map(|m| m.height).collect();
    assert_eq!(heights, opendeskphone_svg::GLYPH_HEIGHTS);
}

#[test]
fn the_manifest_carries_the_glyph() {
    const WITH: Manifest = Manifest {
        name: "sun",
        display_name: "Sun",
        glyph: SUN,
        ..Manifest::EMPTY
    }
    .checked();
    const WITHOUT: Manifest = Manifest {
        glyph: Glyph::NONE,
        ..WITH
    };
    let coverage: usize = SUN.masks.iter().map(|m| m.alpha.len()).sum();
    assert!(WITH.encoded_len() > WITHOUT.encoded_len() + coverage);
    let section = WITH.encode::<{ WITH.encoded_len() }>();
    let first = SUN.masks[0].alpha;
    assert!(section.windows(first.len()).any(|w| w == first));
}
