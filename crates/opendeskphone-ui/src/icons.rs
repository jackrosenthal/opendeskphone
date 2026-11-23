//! Built-in icons and artwork, rasterized per skin from `assets/` at build
//! time.

/// Encodings of [`IconRec`] data.
pub(crate) const FMT_MASK4: u8 = 0;
pub(crate) const FMT_GRAY2: u8 = 1;

/// One bitmap in a skin's icon table.
#[derive(Clone, Copy, Debug)]
pub struct IconRec {
    pub name: &'static str,
    pub w: u16,
    pub h: u16,
    /// 0 ([`FMT_MASK4`]): 4-bit alpha mask (tinted when drawn); 1: four gray levels plus
    /// transparency; 2: run-length RGBA.
    pub fmt: u8,
    pub off: u32,
    pub len: u32,
}

/// A decodable icon: its record and the skin's data blob.
#[derive(Clone, Copy)]
pub struct Icon {
    pub rec: &'static IconRec,
    pub data: &'static [u8],
}

impl Icon {
    pub fn bytes(&self) -> &'static [u8] {
        &self.data[self.rec.off as usize..(self.rec.off + self.rec.len) as usize]
    }
    pub fn width(&self) -> i32 {
        self.rec.w as i32
    }
    pub fn height(&self) -> i32 {
        self.rec.h as i32
    }
}

/// Names of the built-in icons (the same set in every skin).  Icons from
/// `assets/icons/status` are monochrome and take the text color; the app
/// icons from `assets/icons/app` are named `app-<file>` (`app-contacts`)
/// and are full color on the color skin; `splash` is the boot-screen
/// artwork (the only branding; no other screen uses it).
pub fn names() -> Vec<&'static str> {
    crate::atlas::ICONS_COLOR.iter().map(|r| r.name).collect()
}

const TWINS: [(&str, &str); 20] = [
    ("app-alarms", "alarm"),
    ("app-app", "apps"),
    ("app-boxpusher", "boxpusher"),
    ("app-calendar", "calendar"),
    ("app-calllog", "calllog"),
    ("app-contacts", "contacts"),
    ("app-desksparrow", "desksparrow"),
    ("app-diagnostics", "diagnostics"),
    ("app-forward", "forward"),
    ("app-hotdesking", "desk"),
    ("app-info", "info"),
    ("app-lightup", "lightup"),
    ("app-network", "network"),
    ("app-paging", "page"),
    ("app-parking", "park"),
    ("app-queues", "conference"),
    ("app-settings", "settings"),
    ("app-status", "presence"),
    ("app-update", "update"),
    ("app-voicemail", "voicemail"),
];

/// Returns the one-color twin of an app icon, which is the status icon of
/// the glyph on the app icon's tile, e.g., `alarm` for `app-alarms`.  Any
/// other name has no twin.
pub fn twin(app_icon: &str) -> Option<&'static str> {
    TWINS.iter().find(|(app, _)| *app == app_icon).map(|(_, twin)| *twin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas;

    const SKINS: &[&[IconRec]] = &[
        atlas::ICONS_SMALL_GRAY,
        atlas::ICONS_LARGE_GRAY,
        atlas::ICONS_COLOR,
        #[cfg(feature = "touch")]
        atlas::ICONS_TOUCH,
    ];

    #[test]
    fn every_app_icon_has_a_one_color_twin_in_every_skin() {
        for icons in SKINS {
            let apps: Vec<&str> = icons.iter().map(|r| r.name).filter(|n| n.starts_with("app-")).collect();
            assert_eq!(apps.len(), TWINS.len());
            for app in apps {
                let one = icons.iter().find(|r| Some(r.name) == twin(app));
                assert!(one.is_some_and(|r| r.fmt == FMT_MASK4), "{app}");
            }
        }
    }
}
