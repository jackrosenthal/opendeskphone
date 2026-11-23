//! The manifest `opendeskphone-app-sdk` writes at compile time is the one this
//! crate reads: the two are written by hand against the same
//! `manifest.proto`, and this holds them to each other.

use opendeskphone_apps::manifest::{AppType, Manifest};
use opendeskphone_app_sdk::manifest::{AppKind, Glyph, GlyphMask, Manifest as Written};
use opendeskphone_app_sdk::{Icon, PhoneModel};

/// A module that holds nothing but the section.
fn module(section: &[u8]) -> Vec<u8> {
    let name = opendeskphone_app_sdk::manifest::SECTION.as_bytes();
    assert_eq!(name, opendeskphone_apps::manifest::SECTION.as_bytes());
    let mut body = vec![name.len() as u8];
    body.extend_from_slice(name);
    body.extend_from_slice(section);
    assert!(body.len() < 128 * 128);
    let mut m = b"\0asm\x01\0\0\0\0".to_vec();
    if body.len() < 128 {
        m.push(body.len() as u8);
    } else {
        m.extend([(body.len() % 128) as u8 | 0x80, (body.len() / 128) as u8]);
    }
    m.extend(body);
    m
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nthe rest of a picture";
const COLOR: &[u8] = b"\x89PNG\r\n\x1a\na picture in color";
const GLYPH: Glyph = Glyph {
    masks: &[
        GlyphMask { width: 3, height: 2, alpha: &[0x0f, 0x80, 0x7f] },
        GlyphMask { width: 1, height: 1, alpha: &[0xf0] },
    ],
};

#[test]
fn every_field_arrives() {
    const W: Written = Written {
        name: "probe",
        id: "org.example.probe",
        display_name: "Probe",
        description: "Looks around",
        kind: AppKind::Background,
        has_idle: true,
        hidden: true,
        icon: PNG,
        icon_for: &[(PhoneModel::D60, COLOR), (PhoneModel::D65, COLOR)],
        phone_icon: Some(Icon::AppStatus),
        glyph: GLYPH,
        memory_kib: 8192,
    }
    .checked();
    const LEN: usize = W.encoded_len();
    const SECTION: [u8; LEN] = W.encode::<LEN>();

    let m = Manifest::from_wasm(&module(&SECTION)).unwrap();
    assert_eq!(m.name, "probe");
    assert_eq!(m.id, "org.example.probe");
    assert_eq!(m.display_name, "Probe");
    assert_eq!(m.description, "Looks around");
    assert_eq!(m.kind, AppType::Background);
    assert!(m.has_idle && m.hidden);
    assert_eq!(m.icon, "app-status");
    assert_eq!(m.icon_for("D60"), COLOR);
    assert_eq!(m.icon_for("D65"), COLOR);
    assert_eq!(m.icon_for("D40"), PNG);
    let masks: Vec<_> = m.glyph.iter().map(|g| (g.width, g.height, g.alpha.to_vec())).collect();
    assert_eq!(masks, [(3, 2, vec![0x0f, 0x80, 0x7f]), (1, 1, vec![0xf0])]);
    assert_eq!(m.memory_kib, 8192);
    assert_eq!(m.wasm, "probe.wasm");
    assert!(m.capabilities.is_empty());
    // What this crate writes for the same manifest is the same bytes.
    assert_eq!(m.section(), SECTION);
}

#[test]
fn the_least_manifest() {
    const W: Written = Written {
        name: "hello",
        display_name: "Hello",
        ..Written::EMPTY
    }
    .checked();
    const LEN: usize = W.encoded_len();
    let m = Manifest::from_wasm(&module(&W.encode::<LEN>())).unwrap();
    assert_eq!(
        (m.name.as_str(), m.display_name.as_str()),
        ("hello", "Hello")
    );
    assert_eq!(m.kind, AppType::Foreground);
    assert!(!m.has_idle && !m.hidden);
    assert!(m.icon.is_empty() && m.icon_for("D60").is_empty());
    assert!(m.glyph.is_empty());
}
