//! The theme the configuration asks for, as the UI takes it: a
//! built-in one by name, or the provisioning server's picture
//! (`/nvdata/user_wallpaper.png`) with the palette file that came with it
//! (`/nvdata/opendeskphone/wallpaper_palette.txtpb`).
//!
//! A picture that cannot be shown (not downloaded yet, or not a PNG file)
//! is passed over for the built-in wallpaper.  A palette file that cannot
//! be read is passed over for the firmware's default palette, with a log
//! entry: a bad file never keeps the UI from drawing.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use opendeskphone_config::palette::{PALETTE_FILE, WALLPAPER_FILE};
use opendeskphone_config::{Config, Nvdata, PaletteFile, ThemeChoice};
use opendeskphone_ui::skin::Palette;
use opendeskphone_ui::wallpaper::Picture;
use opendeskphone_ui::UiConfig;
use log::{info, warn};

/// The palette of a palette file, and what the log should say about the
/// file (fields that were skipped, pairings with little contrast).
/// `Err` when the file is not a palette file at all.
pub fn palette(text: &str) -> Result<(Palette, Vec<String>), String> {
    let file = PaletteFile::parse(text)?;
    let (palette, unknown) = opendeskphone_ui::wallpaper::palette_from(file.light, file.primary, file.surface, &file.roles);
    let mut notes = file.skipped;
    // (The file's roles are the UI's roles; a test holds them together.)
    notes.extend(unknown.into_iter().map(|role| format!("unknown role {role:?}")));
    notes.extend(palette.low_contrast());
    Ok((palette, notes))
}

/// The picture in nvdata with its palette, decoded once per content.
fn picture(nv: &Nvdata, with_palette: bool) -> Option<Arc<Picture>> {
    static LAST: Mutex<Option<(u64, Option<Arc<Picture>>)>> = Mutex::new(None);
    let png = nv.read(WALLPAPER_FILE).ok().flatten()?;
    let palette_text = if with_palette { nv.read(PALETTE_FILE).ok().flatten() } else { None };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&png, &palette_text).hash(&mut hasher);
    let key = hasher.finish();
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((k, picture)) = last.as_ref()
        && *k == key
    {
        return picture.clone();
    }
    let made = match opendeskphone_ui::image::decode_png(&png) {
        Ok(image) => {
            let palette = palette_text.and_then(|bytes| match String::from_utf8(bytes).map_err(|e| e.to_string()).and_then(|t| palette(&t)) {
                Ok((palette, notes)) => {
                    for note in notes {
                        warn!("wallpaper: palette file: {note}");
                    }
                    Some(palette)
                }
                Err(e) => {
                    warn!("wallpaper: the palette file is not used (the default palette is): {e}");
                    None
                }
            });
            info!(
                "wallpaper: the provisioned picture ({} x {}) with {}",
                image.width,
                image.height,
                if palette.is_some() { "its palette file" } else { "the default palette" }
            );
            Some(Arc::new(Picture::new(&image, palette)))
        }
        Err(e) => {
            warn!("wallpaper: the provisioned picture is not used: {e}");
            None
        }
    };
    *last = Some((key, made.clone()));
    made
}

/// Sets the theme fields of the UI's configuration.
pub fn apply(cfg: &Config, nv: &Nvdata, out: &mut UiConfig) {
    let theme = cfg.theme();
    out.theme_locked = theme.locked;
    out.wallpaper_picture = None;
    match theme.choice {
        ThemeChoice::Builtin(name) => out.theme = name,
        ThemeChoice::Picture => {
            out.theme = theme.builtin;
            if cfg.model().has_themes() {
                out.wallpaper_picture = picture(nv, cfg.palette_file_named());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_config::{ConfigStore, Model};
    use opendeskphone_ui::skin::Theme;

    fn skin() -> Palette {
        Theme::for_model(opendeskphone_state::Model::D65).palette()
    }

    /// A PNG file: 8 x 4 pixels of #204060.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x04, 0x08, 0x02, 0x00, 0x00, 0x00, 0x3c, 0xaf, 0xe9,
        0xa7, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x50, 0x70, 0x48, 0xc0,
        0x8a, 0x18, 0xa8, 0x27, 0x01, 0x00, 0xa8, 0xa0, 0x18, 0x01, 0x30, 0x17, 0x20, 0x15, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn the_roles_of_a_palette_file_are_the_uis_roles() {
        let names: Vec<&str> = skin().roles().iter().map(|r| r.0).collect();
        assert_eq!(names, opendeskphone_config::palette::ROLES);
    }

    #[test]
    fn palette_files() {
        // The example of `palette.proto`: derived, then two roles by name.
        let (p, notes) = palette(
            "# Palette for lobby.png\nprimary: \"#1F6E4E\"\nsurface: \"#0F1E18\"\ntitle_bg: \"#14503A\"\naccent: \"#8FD6B4\"\n",
        )
        .unwrap();
        assert_eq!((p.sel_bg, p.bg, p.title_bg, p.accent), (0x1F6E4E, 0x0F1E18, 0x14503A, 0x8FD6B4));
        assert_eq!(p.fg, skin().fg);
        assert!(notes.is_empty(), "{notes:?}");
        // An empty file is the default palette.
        assert_eq!(palette("").unwrap(), (skin(), vec![]));
        // Partial: one role; the rest is the default palette.
        let (p, notes) = palette("sel_bg: \"#204060\"\n").unwrap();
        assert_eq!(p, Palette { sel_bg: 0x204060, ..skin() });
        assert!(notes.is_empty());
        // Unknown roles and bad colors are skipped and said; little
        // contrast is said and used.
        let (p, notes) = palette("sparkle: \"#FFFFFF\"\nbg: \"#EEEEEE\"\ntab_bg: \"nope\"\n").unwrap();
        assert_eq!(p, Palette { bg: 0xEEEEEE, ..skin() });
        assert_eq!(notes[0], "unknown field \"sparkle\"");
        assert_eq!(notes[1], "tab_bg: \"nope\" is not a color (\"#RRGGBB\")");
        assert!(notes[2..].iter().any(|n| n.starts_with("text (fg) on the background (bg): contrast 1.0")), "{notes:?}");
        // Not a palette file.
        assert!(palette("<config/>").is_err());
        assert!(palette("bg: 7").is_err());
    }

    #[test]
    fn the_configured_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nvdata");
        std::fs::create_dir_all(dir.join("opendeskphone")).unwrap();
        let nv = Nvdata::new(&dir);
        let mut store = ConfigStore::open(Nvdata::new(&dir), Model::D65, tmp.path().join("tmp")).unwrap();
        let ui = |store: &ConfigStore| {
            let mut out = UiConfig::default();
            apply(&store.config(), &nv, &mut out);
            out
        };
        // Nothing set: the default theme.
        let c = ui(&store);
        assert_eq!((c.theme.as_str(), c.theme_locked, c.wallpaper_picture.is_none()), ("", false, true));
        // A name chosen on the phone.
        store.update_opendeskphone_settings(|s| s.theme = Some("dusk".into())).unwrap();
        assert_eq!(ui(&store).theme, "dusk");

        // A provisioned picture that is not there yet: the built-in one
        // goes on showing, and the choice is the server's (locked).
        let picture = r#"<setting id="wallpaper_file" value="user" url="file://wall.png" path="/user_wallpaper.png" conflict="freeze"/>"#;
        store.apply_fetched(&format!("<config>{picture}</config>")).unwrap();
        let c = ui(&store);
        assert_eq!((c.theme.as_str(), c.theme_locked, c.wallpaper_picture.is_none()), ("dusk", true, true));
        // Downloaded: the picture, in the default palette.
        nv.write(WALLPAPER_FILE, PNG).unwrap();
        let c = ui(&store);
        let shown = c.wallpaper_picture.expect("the picture");
        assert_eq!(shown.palette, None);
        assert!(shown.pixels().iter().all(|p| *p == 0x204060));
        // A palette file on disk that the configuration does not name is
        // not used.
        nv.write(PALETTE_FILE, b"sel_bg: \"#204060\"\n").unwrap();
        assert_eq!(ui(&store).wallpaper_picture.unwrap().palette, None);
        // Named: the picture with its palette.
        let both = format!(r#"<config>{picture}<setting id="opendeskphone_palette_file" value="user" url="file://wall.txtpb" conflict="freeze"/></config>"#);
        store.apply_fetched(&both).unwrap();
        assert_eq!(ui(&store).wallpaper_picture.unwrap().palette, Some(Palette { sel_bg: 0x204060, ..skin() }));
        // The same files again are the same picture, not decoded anew.
        assert!(Arc::ptr_eq(&ui(&store).wallpaper_picture.unwrap(), &ui(&store).wallpaper_picture.unwrap()));
        // A new palette file takes effect; a bad one is the default
        // palette, and the picture stays.
        nv.write(PALETTE_FILE, b"light: true\nsurface: \"#ECE9E4\"\n").unwrap();
        assert!(ui(&store).wallpaper_picture.unwrap().palette.unwrap().is_light());
        nv.write(PALETTE_FILE, b"\x89PNG not a palette").unwrap();
        let c = ui(&store);
        assert_eq!(c.wallpaper_picture.as_ref().unwrap().palette, None);
        assert!(c.wallpaper_picture.unwrap().pixels().iter().all(|p| *p == 0x204060));
        // A file that is not a picture: the built-in wallpaper.
        nv.write(WALLPAPER_FILE, b"not a png").unwrap();
        let c = ui(&store);
        assert_eq!((c.theme.as_str(), c.wallpaper_picture.is_none()), ("dusk", true));
        // No longer named: back to the phone's own choice, unlocked.
        store.apply_fetched("<config></config>").unwrap();
        let c = ui(&store);
        assert_eq!((c.theme.as_str(), c.theme_locked, c.wallpaper_picture.is_none()), ("dusk", false, true));
        // A gray model has no wallpaper picture.
        let mut gray = ConfigStore::open(Nvdata::new(&dir), Model::D50, tmp.path().join("tmp")).unwrap();
        nv.write(WALLPAPER_FILE, PNG).unwrap();
        gray.apply_fetched(&both).unwrap();
        let mut out = UiConfig::default();
        apply(&gray.config(), &nv, &mut out);
        assert!(out.wallpaper_picture.is_none());
    }
}
