//! The app's manifest: what the phone has to know about an app before it
//! runs it.  It is written in Rust, in [`export_app!`](crate::export_app),
//! and travels inside the app's WebAssembly file, in the custom section
//! `opendeskphone-app`, as the protobuf message `opendeskphone.app.v1.Manifest`
//! (`proto/manifest.proto` in this crate).
//!
//! The section is made when the app is compiled, so everything here is
//! `const`: a mistake in the manifest is a compile error.

use crate::{Icon, PhoneModel};

/// The custom section that holds the manifest.
pub const SECTION: &str = "opendeskphone-app";

/// The version of the manifest format this crate writes.
pub const FORMAT: u32 = 1;

/// When an app runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    /// While it is on screen (the default).
    Foreground,
    /// From the phone's start, without being on screen.
    Background,
}

/// The fields of [`export_app!`](crate::export_app), after `app`.  Only
/// `name` and `display_name` must be given.
#[derive(Clone, Copy, Debug)]
pub struct Manifest {
    /// The app's name on the phone: 1 to 64 characters, the first a letter
    /// or digit, the rest letters, digits, `_`, `.`, or `,`.  It cannot be
    /// the name of a built-in app.
    pub name: &'static str,
    /// An identifier of the app, e.g., in reverse domain form.
    pub id: &'static str,
    /// What the phone calls the app on screen.
    pub display_name: &'static str,
    /// A line about the app.
    pub description: &'static str,
    pub kind: AppKind,
    /// The app has a window on the idle screen, and starts with the phone
    /// to draw there.
    pub has_idle: bool,
    /// The app is marked as hidden.
    pub hidden: bool,
    /// The app's icon: the content of a PNG file, e.g.,
    /// `include_bytes!("hello.png")`.  At most 256 KiB.
    pub icon: &'static [u8],
    /// Icons for particular models of phone, each a PNG file's content:
    /// (model, icon), e.g.,
    /// `&[(PhoneModel::D60, include_bytes!("color.png"))]`.  A phone shows
    /// its model's icon if there is one, else `icon`.
    pub icon_for: &'static [(PhoneModel, &'static [u8])],
    /// One of the phone's icons, for an app without an icon, e.g.,
    /// `Some(Icon::AppCalendar)`.
    pub phone_icon: Option<Icon>,
    /// The macro `glyph!` makes this from an SVG file, and the default is
    /// [`Glyph::NONE`].
    pub glyph: Glyph,
    /// The most memory the app may have, in KiB.  The default (0) is
    /// 4 MiB, and the phone gives no app more than 16 MiB.
    pub memory_kib: u32,
}

/// The section holds a glyph as the message `Glyph` of
/// `proto/manifest.proto`, which sets its limits and its format.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub masks: &'static [GlyphMask],
}

impl Glyph {
    pub const NONE: Glyph = Glyph { masks: &[] };
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphMask {
    pub width: u32,
    pub height: u32,
    pub alpha: &'static [u8],
}

/// `proto/manifest.proto` sets this limit.
pub const MAX_GLYPH_MASKS: usize = 8;

/// The largest icon.
pub const MAX_ICON_BYTES: usize = 256 << 10;

/// A model's number in the manifest (`PhoneModel` of `manifest.proto`):
/// its place in the app interface's enum, from 1.
const fn model_number(model: PhoneModel) -> u64 {
    model as u64 + 1
}

/// An icon's number in the manifest (`Icon` of `manifest.proto`): its
/// place in the app interface's enum, from 1.
const fn icon_number(icon: Option<Icon>) -> u64 {
    match icon {
        Some(icon) => icon as u64 + 1,
        None => 0,
    }
}

const fn png_ok(icon: &[u8]) -> bool {
    icon.len() > 8 && icon[0] == 0x89 && icon[1] == b'P' && icon[2] == b'N' && icon[3] == b'G'
}

const fn model_icon_len(i: &(PhoneModel, &[u8])) -> usize {
    number_len(model_number(i.0)) + delimited_len(i.1.len())
}

const fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// A length-delimited field of `len` bytes, with its one-byte tag.
const fn delimited_len(len: usize) -> usize {
    1 + varint_len(len as u64) + len
}

/// A number field, with its one-byte tag; nothing for 0.
const fn number_len(v: u64) -> usize {
    if v == 0 { 0 } else { 1 + varint_len(v) }
}

const fn string_len(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        delimited_len(s.len())
    }
}

/// A `Text` message: the text is the first field of it.
const fn text_len(text: &str) -> usize {
    string_len(text)
}

const fn mask_len(m: &GlyphMask) -> usize {
    let alpha = if m.alpha.is_empty() { 0 } else { delimited_len(m.alpha.len()) };
    number_len(m.width as u64) + number_len(m.height as u64) + alpha
}

const fn glyph_len(g: &Glyph) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < g.masks.len() {
        n += delimited_len(mask_len(&g.masks[i]));
        i += 1;
    }
    n
}

/// A cursor over the section being written.
struct Out<const N: usize> {
    bytes: [u8; N],
    at: usize,
}

impl<const N: usize> Out<N> {
    const fn varint(mut self, mut v: u64) -> Self {
        while v >= 0x80 {
            self.bytes[self.at] = (v as u8 & 0x7f) | 0x80;
            self.at += 1;
            v >>= 7;
        }
        self.bytes[self.at] = v as u8;
        self.at += 1;
        self
    }

    /// The tag of a length-delimited field and its length.
    const fn open(self, field: u32, len: usize) -> Self {
        self.varint((field as u64) << 3 | 2).varint(len as u64)
    }

    const fn bytes(mut self, field: u32, data: &[u8]) -> Self {
        self = self.open(field, data.len());
        let (_, rest) = self.bytes.split_at_mut(self.at);
        let (dest, _) = rest.split_at_mut(data.len());
        dest.copy_from_slice(data);
        self.at += data.len();
        self
    }

    const fn string(self, field: u32, s: &str) -> Self {
        if s.is_empty() {
            self
        } else {
            self.bytes(field, s.as_bytes())
        }
    }

    const fn number(self, field: u32, v: u64) -> Self {
        if v == 0 {
            self
        } else {
            self.varint((field as u64) << 3).varint(v)
        }
    }

    const fn text(self, field: u32, text: &str) -> Self {
        let len = text_len(text);
        if len == 0 {
            return self;
        }
        self.open(field, len).string(1, text)
    }
}

const fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    if b.is_empty() || b.len() > 64 || !b[0].is_ascii_alphanumeric() {
        return false;
    }
    let mut i = 1;
    while i < b.len() {
        if !(b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'.' | b',')) {
            return false;
        }
        i += 1;
    }
    true
}

impl Manifest {
    /// Every field at its default; `name` and `display_name` are missing.
    pub const EMPTY: Manifest = Manifest {
        name: "",
        id: "",
        display_name: "",
        description: "",
        kind: AppKind::Foreground,
        has_idle: false,
        hidden: false,
        icon: &[],
        icon_for: &[],
        phone_icon: None,
        glyph: Glyph::NONE,
        memory_kib: 0,
    };

    /// The manifest, if the phone will take it; otherwise the compilation
    /// fails with what is wrong.
    pub const fn checked(self) -> Manifest {
        assert!(!self.name.is_empty(), "export_app!: `name` is missing");
        assert!(
            name_ok(self.name),
            "export_app!: `name` must be 1 to 64 characters, the first a letter or digit, the rest letters, digits, '_', '.', or ','"
        );
        assert!(
            !self.display_name.is_empty(),
            "export_app!: `display_name` is missing"
        );
        assert!(
            self.memory_kib <= 16 << 10,
            "export_app!: `memory_kib` is more than the phone gives any app (16384)"
        );
        assert!(
            self.icon.len() <= MAX_ICON_BYTES,
            "export_app!: `icon` is larger than 256 KiB"
        );
        assert!(
            self.icon.is_empty() || png_ok(self.icon),
            "export_app!: `icon` is not a PNG file's content (for one of the phone's icons, give it as `phone_icon`)"
        );
        let mut i = 0;
        while i < self.icon_for.len() {
            let (model, icon) = self.icon_for[i];
            assert!(
                icon.len() <= MAX_ICON_BYTES && png_ok(icon),
                "export_app!: an icon in `icon_for` is not a PNG file's content of at most 256 KiB"
            );
            let mut j = 0;
            while j < i {
                assert!(
                    model_number(self.icon_for[j].0) != model_number(model),
                    "export_app!: `icon_for` names a model twice"
                );
                j += 1;
            }
            i += 1;
        }
        let masks = self.glyph.masks;
        assert!(
            masks.len() <= MAX_GLYPH_MASKS,
            "export_app!: `glyph` has more than 8 masks"
        );
        let mut i = 0;
        while i < masks.len() {
            let m = masks[i];
            assert!(
                m.height >= 1 && m.height <= 128,
                "export_app!: a mask of `glyph` is not 1 to 128 pixels tall"
            );
            assert!(
                m.width <= 2 * m.height,
                "export_app!: a mask of `glyph` is more than twice as wide as it is tall"
            );
            assert!(
                m.alpha.len() as u64 == (m.width as u64 * m.height as u64).div_ceil(2),
                "export_app!: a mask of `glyph` does not have (width * height).div_ceil(2) bytes of coverage"
            );
            let mut j = 0;
            while j < i {
                assert!(
                    masks[j].height != m.height,
                    "export_app!: `glyph` has two masks of one height"
                );
                j += 1;
            }
            i += 1;
        }
        self
    }

    /// The size of the manifest's section.
    pub const fn encoded_len(&self) -> usize {
        let mut n = 1 + varint_len(FORMAT as u64);
        n += string_len(self.name) + string_len(self.id);
        let t = text_len(self.display_name);
        if t > 0 {
            n += delimited_len(t);
        }
        let t = text_len(self.description);
        if t > 0 {
            n += delimited_len(t);
        }
        // `kind` is always written; the flags only when set.
        n += 2;
        n += self.has_idle as usize * 2 + self.hidden as usize * 2;
        n += number_len(icon_number(self.phone_icon));
        if self.memory_kib > 0 {
            n += 1 + varint_len(self.memory_kib as u64);
        }
        if !self.icon.is_empty() {
            n += delimited_len(self.icon.len());
        }
        let mut i = 0;
        while i < self.icon_for.len() {
            n += delimited_len(model_icon_len(&self.icon_for[i]));
            i += 1;
        }
        if !self.glyph.masks.is_empty() {
            n += delimited_len(glyph_len(&self.glyph));
        }
        n
    }

    /// The manifest's section; `N` is [`Manifest::encoded_len`].
    pub const fn encode<const N: usize>(&self) -> [u8; N] {
        let kind = match self.kind {
            AppKind::Foreground => 1,
            AppKind::Background => 2,
        };
        let mut out = Out {
            bytes: [0; N],
            at: 0,
        }
        .number(1, FORMAT as u64)
        .string(2, self.name)
        .string(3, self.id)
        .text(4, self.display_name)
        .text(5, self.description)
        .number(6, kind)
        .number(7, self.has_idle as u64)
        .number(8, self.hidden as u64)
        .number(10, self.memory_kib as u64)
        .number(13, icon_number(self.phone_icon));
        if !self.icon.is_empty() {
            out = out.bytes(11, self.icon);
        }
        let mut i = 0;
        while i < self.icon_for.len() {
            let m = &self.icon_for[i];
            out = out.open(12, model_icon_len(m)).bytes(2, m.1).number(3, model_number(m.0));
            i += 1;
        }
        if !self.glyph.masks.is_empty() {
            out = out.open(14, glyph_len(&self.glyph));
            let mut i = 0;
            while i < self.glyph.masks.len() {
                let m = &self.glyph.masks[i];
                out = out
                    .open(1, mask_len(m))
                    .number(1, m.width as u64)
                    .number(2, m.height as u64);
                if !m.alpha.is_empty() {
                    out = out.bytes(3, m.alpha);
                }
                i += 1;
            }
        }
        assert!(out.at == N, "the manifest's section has another length");
        out.bytes
    }
}
