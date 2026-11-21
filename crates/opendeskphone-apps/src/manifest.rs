//! The app manifest, and the app file that carries it.
//!
//! An app is one WebAssembly component.  Its manifest (the stock fields
//! `name`, `id`, `displayName`, `description`, `type`, `hasIdle`, `hidden`,
//! and the icon, plus a memory limit) is the
//! protobuf message `opendeskphone.app.v1.Manifest` in the custom section
//! `opendeskphone-app` of the app's module, which the macro `export_app!` of
//! `opendeskphone-app-sdk` writes (the format is `proto/manifest.proto` of that
//! crate).  This module finds the section in a module or a component and
//! reads it, without compiling anything.
//!
//! The phone keeps a record of each app in the app's directory
//! ([`RECORD`]): the manifest, with the interfaces the component imports
//! ([`Capability`]), which are not the app's to declare.
//!
//! A stock app package (a ZIP archive with `app.json` and JavaScript) is
//! recognized only to say that it cannot run.

use std::fmt;

/// The custom section that holds the manifest.
pub const SECTION: &str = "opendeskphone-app";
/// The manifest format this code reads.
pub const FORMAT: u32 = 1;
/// The phone's record of an app, in the app's directory.
pub const RECORD: &str = "app.pb";
/// The largest icon.
pub const MAX_ICON_BYTES: usize = 256 << 10;
/// The models of phone an icon can be for.
pub const MODELS: [&str; 8] = ["D40", "D45", "D50", "D60", "D62", "D65", "D70", "D80"];

/// A host interface an app may be granted.  `runtime` is always granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    Ui,
    Timers,
    Storage,
    Config,
    Pbx,
    Http,
    Calls,
    Presence,
    Events,
    Audio,
    L10n,
    Canvas,
    Hotdesk,
    Diagnostics,
    Lamps,
    Input,
    Alarms,
}

impl Capability {
    pub const ALL: [Capability; 17] = [
        Capability::Ui,
        Capability::Timers,
        Capability::Storage,
        Capability::Config,
        Capability::Pbx,
        Capability::Http,
        Capability::Calls,
        Capability::Presence,
        Capability::Events,
        Capability::Audio,
        Capability::L10n,
        Capability::Canvas,
        Capability::Hotdesk,
        Capability::Diagnostics,
        Capability::Lamps,
        Capability::Input,
        Capability::Alarms,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Capability::Ui => "ui",
            Capability::Timers => "timers",
            Capability::Storage => "storage",
            Capability::Config => "config",
            Capability::Pbx => "pbx",
            Capability::Http => "http",
            Capability::Calls => "calls",
            Capability::Presence => "presence",
            Capability::Events => "events",
            Capability::Audio => "audio",
            Capability::L10n => "l10n",
            Capability::Canvas => "canvas",
            Capability::Hotdesk => "hotdesk",
            Capability::Diagnostics => "diagnostics",
            Capability::Lamps => "lamps",
            Capability::Input => "input",
            Capability::Alarms => "alarms",
        }
    }

    pub fn from_name(s: &str) -> Option<Capability> {
        Capability::ALL.into_iter().find(|c| c.name() == s)
    }

    /// The capability an import of a component stands for: `Ok(None)` for
    /// the interfaces every app has, an error for something that is not
    /// an interface of the phone.  `import` is the import's name, e.g.,
    /// `opendeskphone:app/ui@3.1.0`.
    pub fn of_import(import: &str) -> Result<Option<Capability>, ManifestError> {
        let unknown = || ManifestError::UnknownImport(import.to_string());
        let interface = import.strip_prefix("opendeskphone:app/").ok_or_else(unknown)?;
        let interface = interface.split('@').next().unwrap_or(interface);
        match interface {
            "runtime" | "types" => Ok(None),
            other => Capability::from_name(other).map(Some).ok_or_else(unknown),
        }
    }
}

/// Stock manifest `type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppType {
    Foreground,
    Background,
}

impl AppType {
    pub fn name(self) -> &'static str {
        match self {
            AppType::Foreground => "foreground",
            AppType::Background => "background",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// The file is not WebAssembly.
    NotWasm,
    /// A stock app package: a ZIP archive with `app.json`.
    JavaScript,
    /// The WebAssembly has no manifest section (stock status -101).
    NoManifest,
    /// The section is not a `Manifest` message.
    Malformed(&'static str),
    /// A manifest format this code does not know.
    Format(u32),
    /// Name missing, too long, or malformed (stock status -102).
    BadName(&'static str),
    /// An unknown `kind`.
    BadKind(u64),
    /// An icon that is too large or not a PNG file, or one for a model
    /// that is none.
    BadIcon(&'static str),
    /// The component imports something that is not an interface of the
    /// phone.
    UnknownImport(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::NotWasm => write!(
                f,
                "not an app: an app is a WebAssembly component (a .wasm file made by wasm-tools component new)"
            ),
            ManifestError::JavaScript => write!(
                f,
                "this is a package of a stock JavaScript app; JavaScript apps cannot run on this firmware"
            ),
            ManifestError::NoManifest => write!(
                f,
                "the WebAssembly file has no app manifest (the section \"{SECTION}\", which export_app! of opendeskphone-app-sdk writes)"
            ),
            ManifestError::Malformed(why) => write!(f, "unreadable app manifest: {why}"),
            ManifestError::Format(v) => write!(
                f,
                "the app's manifest has format {v}, and this firmware knows format {FORMAT}: the app was made for another firmware"
            ),
            ManifestError::BadName(why) => write!(f, "bad app name: {why}"),
            ManifestError::BadKind(k) => write!(f, "bad app kind {k}"),
            ManifestError::BadIcon(why) => write!(f, "bad app icon: {why}"),
            ManifestError::UnknownImport(i) => write!(
                f,
                "the app imports {i:?}, which is not an interface of this firmware"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub id: String,
    /// What the phone calls the app on screen; the app's name if the
    /// manifest gives none.
    pub display_name: String,
    pub description: String,
    pub kind: AppType,
    /// The app has a window on the idle screen, and starts with the phone.
    pub has_idle: bool,
    pub hidden: bool,
    /// The name of one of the phone's icons (`app-calendar`), for an app
    /// without an icon of its own.
    pub icon: String,
    /// The app's icon, a PNG file's content (empty: none), and its icons
    /// for particular models: (model, PNG).
    pub icon_png: Vec<u8>,
    pub icons_for: Vec<(String, Vec<u8>)>,
    /// The component's file name in the app's directory: `<name>.wasm`.
    pub wasm: String,
    /// The interfaces the component imports.  Not part of an app's own
    /// manifest: empty until the phone (or the build, for a built-in app)
    /// has looked at the component.
    pub capabilities: Vec<Capability>,
    /// Requested linear-memory cap in KiB (0: the default); never above
    /// the manager's hard cap.
    pub memory_kib: u32,
}

/// The name the phone itself has in the PBX's store of app data, where
/// it keeps what follows a user from phone to phone (the theme).  No app
/// can have it, so no app can reach those keys.
pub const PHONE_DATA_NAME: &str = "opendeskphone";

/// Stock name rule: 1 to 64 characters, first a letter or digit, then
/// letters, digits, `_`, `.`, or `,`.  [`PHONE_DATA_NAME`] is the
/// phone's.
pub fn validate_name(name: &str) -> Result<(), ManifestError> {
    if name.is_empty() {
        return Err(ManifestError::BadName("empty"));
    }
    if name == PHONE_DATA_NAME {
        return Err(ManifestError::BadName("the phone's own name"));
    }
    if name.len() > 64 {
        return Err(ManifestError::BadName("longer than 64 characters"));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(ManifestError::BadName("must start with a letter or digit"));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ',')) {
        return Err(ManifestError::BadName("invalid character"));
    }
    Ok(())
}

fn validate_icon(png: &[u8]) -> Result<(), ManifestError> {
    if png.len() > MAX_ICON_BYTES {
        return Err(ManifestError::BadIcon("larger than 256 KiB"));
    }
    if !png.starts_with(b"\x89PNG") {
        return Err(ManifestError::BadIcon("not a PNG file"));
    }
    Ok(())
}

// ------------------------------------------------------------- protobuf

/// The value of one field of a protobuf message.
enum Field<'a> {
    Number(u64),
    Bytes(&'a [u8]),
}

const CUT: ManifestError = ManifestError::Malformed("cut short");

fn varint(b: &[u8], at: &mut usize) -> Result<u64, ManifestError> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at).ok_or(CUT)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(ManifestError::Malformed("a number too long"))
}

/// The `len` bytes at `at`, which moves past them.
fn take<'a>(b: &'a [u8], at: &mut usize, len: u64) -> Result<&'a [u8], ManifestError> {
    let end = usize::try_from(len)
        .ok()
        .and_then(|l| at.checked_add(l))
        .filter(|e| *e <= b.len())
        .ok_or(CUT)?;
    let bytes = &b[*at..end];
    *at = end;
    Ok(bytes)
}

/// Calls `f` with every field of the message `b`: (field number, value).
fn fields<'a>(
    b: &'a [u8],
    mut f: impl FnMut(u64, Field<'a>) -> Result<(), ManifestError>,
) -> Result<(), ManifestError> {
    let mut at = 0;
    while at < b.len() {
        let key = varint(b, &mut at)?;
        let value = match key & 7 {
            0 => Field::Number(varint(b, &mut at)?),
            2 => {
                let len = varint(b, &mut at)?;
                Field::Bytes(take(b, &mut at, len)?)
            }
            // Fixed-width fields: the format has none; skipped.
            1 => {
                take(b, &mut at, 8)?;
                continue;
            }
            5 => {
                take(b, &mut at, 4)?;
                continue;
            }
            _ => return Err(ManifestError::Malformed("an unknown wire type")),
        };
        f(key >> 3, value)?;
    }
    Ok(())
}

fn string(v: Field<'_>) -> Result<String, ManifestError> {
    match v {
        Field::Bytes(b) => std::str::from_utf8(b)
            .map(str::to_string)
            .map_err(|_| ManifestError::Malformed("text that is not UTF-8")),
        Field::Number(_) => Err(ManifestError::Malformed("a number where text belongs")),
    }
}

fn number(v: Field<'_>) -> Result<u64, ManifestError> {
    match v {
        Field::Number(n) => Ok(n),
        Field::Bytes(_) => Err(ManifestError::Malformed("bytes where a number belongs")),
    }
}

/// The element that a manifest's enum value names: the values count the
/// elements from 1.
fn numbered<T>(all: &[T], n: u64) -> Option<&T> {
    all.get(usize::try_from(n).ok()?.checked_sub(1)?)
}

fn message<'a>(v: Field<'a>) -> Result<&'a [u8], ManifestError> {
    match v {
        Field::Bytes(b) => Ok(b),
        Field::Number(_) => Err(ManifestError::Malformed("a number where a message belongs")),
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 & 0x7f | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
    put_varint(out, field << 3 | 2);
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn put_string(out: &mut Vec<u8>, field: u64, s: &str) {
    if !s.is_empty() {
        put_bytes(out, field, s.as_bytes());
    }
}

fn put_number(out: &mut Vec<u8>, field: u64, v: u64) {
    if v != 0 {
        put_varint(out, field << 3);
        put_varint(out, v);
    }
}

/// A `Text` message: the text is the message's first field.
fn put_text(out: &mut Vec<u8>, field: u64, text: &str) {
    let mut m = Vec::new();
    put_string(&mut m, 1, text);
    if !m.is_empty() {
        put_bytes(out, field, &m);
    }
}

fn read_text(b: &[u8], into: &mut String) -> Result<(), ManifestError> {
    fields(b, |n, v| {
        if n == 1 {
            *into = string(v)?;
        }
        Ok(())
    })
}

// ------------------------------------------------------------ WebAssembly

const WASM_MAGIC: &[u8] = b"\0asm";
/// The version and layer of a core module, and of a component.
const MODULE: &[u8] = &[1, 0, 0, 0];
const COMPONENT: &[u8] = &[0x0d, 0, 1, 0];

/// Appends the contents of every manifest section in `wasm` (a module, or
/// a component with the modules in it) to `found`.
fn sections(wasm: &[u8], depth: u32, found: &mut Vec<u8>) -> Result<(), ManifestError> {
    if wasm.len() < 8 || &wasm[..4] != WASM_MAGIC {
        return Err(ManifestError::NotWasm);
    }
    let component = match &wasm[4..8] {
        v if v == MODULE => false,
        v if v == COMPONENT => true,
        _ => return Err(ManifestError::NotWasm),
    };
    let cut = |_| ManifestError::Malformed("WebAssembly that is cut short");
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let len = varint(wasm, &mut at).map_err(cut)?;
        let body = take(wasm, &mut at, len).map_err(cut)?;
        match id {
            // A custom section: a name, then the contents.
            0 => {
                let mut p = 0;
                let n = varint(body, &mut p).map_err(cut)?;
                if take(body, &mut p, n).map_err(cut)? == SECTION.as_bytes() {
                    found.extend_from_slice(&body[p..]);
                }
            }
            // In a component: a core module (1) or a component (4).
            1 | 4 if component && depth < 4 => sections(body, depth + 1, found)?,
            _ => {}
        }
    }
    Ok(())
}

/// The bytes of code in `wasm` (a module, or a component with the modules
/// in it): the code sections, which are what a compiler has to work
/// through.  The rest of an app file is mostly its data.
pub fn code_size(wasm: &[u8]) -> u64 {
    fn walk(wasm: &[u8], depth: u32) -> Option<u64> {
        if wasm.len() < 8 || &wasm[..4] != WASM_MAGIC {
            return None;
        }
        let component = &wasm[4..8] == COMPONENT;
        let (mut at, mut code) = (8, 0);
        while at < wasm.len() {
            let id = wasm[at];
            at += 1;
            let len = varint(wasm, &mut at).ok()?;
            let body = take(wasm, &mut at, len).ok()?;
            match id {
                10 if !component => code += len,
                1 | 4 if component && depth < 4 => code += walk(body, depth + 1)?,
                _ => {}
            }
        }
        Some(code)
    }
    // What cannot be walked is counted whole.
    walk(wasm, 0).unwrap_or(wasm.len() as u64)
}

/// True for a ZIP archive that holds an `app.json`: a stock app package.
fn is_stock_package(file: &[u8]) -> bool {
    file.starts_with(b"PK") && file.windows(8).any(|w| w == b"app.json")
}

impl Manifest {
    /// The manifest of an app file (a component, or the module it is made
    /// of).
    pub fn from_wasm(wasm: &[u8]) -> Result<Manifest, ManifestError> {
        let mut found = Vec::new();
        match sections(wasm, 0, &mut found) {
            Err(ManifestError::NotWasm) if is_stock_package(wasm) => {
                return Err(ManifestError::JavaScript);
            }
            other => other?,
        }
        if found.is_empty() {
            return Err(ManifestError::NoManifest);
        }
        Manifest::decode(&found, false)
    }

    /// The phone's record of an app ([`RECORD`]).
    pub fn from_record(record: &[u8]) -> Result<Manifest, ManifestError> {
        Manifest::decode(record, true)
    }

    /// The icon a phone of `model` (e.g., `D60`) shows for the app: the
    /// one for its model, else the app's icon; empty if it has neither.
    pub fn icon_for(&self, model: &str) -> &[u8] {
        self.icons_for
            .iter()
            .find(|(m, _)| m.eq_ignore_ascii_case(model))
            .map(|(_, png)| &png[..])
            .unwrap_or(&self.icon_png)
    }

    fn decode(bytes: &[u8], record: bool) -> Result<Manifest, ManifestError> {
        let mut m = Manifest {
            name: String::new(),
            id: String::new(),
            display_name: String::new(),
            description: String::new(),
            kind: AppType::Foreground,
            has_idle: false,
            hidden: false,
            icon: String::new(),
            icon_png: Vec::new(),
            icons_for: Vec::new(),
            wasm: String::new(),
            capabilities: Vec::new(),
            memory_kib: 0,
        };
        let mut format = 0;
        fields(bytes, |n, v| {
            match n {
                1 => format = number(v)?,
                2 => m.name = string(v)?,
                3 => m.id = string(v)?,
                4 => read_text(message(v)?, &mut m.display_name)?,
                5 => read_text(message(v)?, &mut m.description)?,
                6 => {
                    m.kind = match number(v)? {
                        0 | 1 => AppType::Foreground,
                        2 => AppType::Background,
                        other => return Err(ManifestError::BadKind(other)),
                    }
                }
                7 => m.has_idle = number(v)? != 0,
                8 => m.hidden = number(v)? != 0,
                10 => m.memory_kib = number(v)?.min(u64::from(u32::MAX)) as u32,
                11 => m.icon_png = message(v)?.to_vec(),
                12 => {
                    let mut icon = (String::new(), Vec::new());
                    fields(message(v)?, |n, v| {
                        match n {
                            2 => icon.1 = message(v)?.to_vec(),
                            3 => {
                                icon.0 = numbered(&MODELS, number(v)?)
                                    .ok_or(ManifestError::BadIcon("for a model that is none"))?
                                    .to_string()
                            }
                            _ => {}
                        }
                        Ok(())
                    })?;
                    m.icons_for.push(icon);
                }
                // An icon this phone does not have: none.
                13 => {
                    m.icon = numbered(&crate::view::ICONS, number(v)?)
                        .map(|i| crate::view::icon_name(*i).to_string())
                        .unwrap_or_default()
                }
                // Only the phone's own record says what the app may use.
                16 if record => {
                    if let Some(c) = numbered(&Capability::ALL, number(v)?)
                        && !m.capabilities.contains(c)
                    {
                        m.capabilities.push(*c);
                    }
                }
                _ => {}
            }
            Ok(())
        })?;
        if format != u64::from(FORMAT) {
            return Err(ManifestError::Format(format.min(u64::from(u32::MAX)) as u32));
        }
        validate_name(&m.name)?;
        if !m.icon_png.is_empty() {
            validate_icon(&m.icon_png)?;
        }
        for (i, (model, png)) in m.icons_for.iter().enumerate() {
            if !MODELS.contains(&model.as_str()) {
                return Err(ManifestError::BadIcon("for a model that is none"));
            }
            if m.icons_for[..i].iter().any(|(other, _)| other == model) {
                return Err(ManifestError::BadIcon("two for one model"));
            }
            validate_icon(png)?;
        }
        if m.display_name.is_empty() {
            m.display_name = m.name.clone();
        }
        m.wasm = format!("{}.wasm", m.name);
        Ok(m)
    }

    /// The phone's record of the app: the manifest with the capabilities.
    pub fn record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_number(&mut out, 1, u64::from(FORMAT));
        put_string(&mut out, 2, &self.name);
        put_string(&mut out, 3, &self.id);
        put_text(&mut out, 4, &self.display_name);
        put_text(&mut out, 5, &self.description);
        put_number(
            &mut out,
            6,
            match self.kind {
                AppType::Foreground => 1,
                AppType::Background => 2,
            },
        );
        put_number(&mut out, 7, u64::from(self.has_idle));
        put_number(&mut out, 8, u64::from(self.hidden));
        put_number(&mut out, 10, u64::from(self.memory_kib));
        let icon = crate::view::ICONS.iter().position(|i| crate::view::icon_name(*i) == self.icon);
        put_number(&mut out, 13, icon.map_or(0, |n| n as u64 + 1));
        if !self.icon_png.is_empty() {
            put_bytes(&mut out, 11, &self.icon_png);
        }
        for (model, png) in &self.icons_for {
            let mut i = Vec::new();
            put_bytes(&mut i, 2, png);
            let n = MODELS.iter().position(|m| m == model).map_or(0, |n| n as u64 + 1);
            put_number(&mut i, 3, n);
            put_bytes(&mut out, 12, &i);
        }
        for c in &self.capabilities {
            let n = Capability::ALL.iter().position(|a| a == c).map_or(0, |n| n as u64 + 1);
            put_number(&mut out, 16, n);
        }
        out
    }

    /// A manifest section with these fields: what `export_app!` writes.
    /// For tools and tests that make one app file from another.
    pub fn section(&self) -> Vec<u8> {
        let mut plain = self.clone();
        plain.capabilities.clear();
        plain.record()
    }

    pub fn has(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}

/// `wasm` (a module or a component) with the contents of its manifest
/// section replaced by `section`.  For tools and tests that make one app
/// file from another.
pub fn with_section(wasm: &[u8], section: &[u8]) -> Result<Vec<u8>, ManifestError> {
    if wasm.len() < 8 || &wasm[..4] != WASM_MAGIC {
        return Err(ManifestError::NotWasm);
    }
    let component = &wasm[4..8] == COMPONENT;
    let mut out = wasm[..8].to_vec();
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let len = varint(wasm, &mut at)?;
        let body = take(wasm, &mut at, len)?;
        let mut p = 0;
        let new_body = match id {
            0 => {
                let n = varint(body, &mut p)?;
                if take(body, &mut p, n)? == SECTION.as_bytes() {
                    let mut b = body[..p].to_vec();
                    b.extend_from_slice(section);
                    b
                } else {
                    body.to_vec()
                }
            }
            1 | 4 if component => with_section(body, section)?,
            _ => body.to_vec(),
        };
        out.push(id);
        put_varint(&mut out, new_body.len() as u64);
        out.extend(new_body);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_stock_rule() {
        assert!(validate_name("voicemail").is_ok());
        assert!(validate_name("a_b.c,d9").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("my-app").is_err());
        // The phone's own name in the PBX's store is not an app's.
        assert!(validate_name(PHONE_DATA_NAME).is_err());
        assert!(validate_name("dphone2").is_ok());
        assert!(validate_name("_x").is_err());
        assert!(validate_name(&"a".repeat(65)).is_err());
        assert!(validate_name("a/b").is_err());
    }

    fn module_with(section: &[u8]) -> Vec<u8> {
        let mut body = vec![SECTION.len() as u8];
        body.extend_from_slice(SECTION.as_bytes());
        body.extend_from_slice(section);
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        m.push(0);
        put_varint(&mut m, body.len() as u64);
        m.extend(body);
        m
    }

    fn sample() -> Manifest {
        Manifest::decode(&[8, 1, 0x12, 2, b'v', b'm'], false).unwrap()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    #[test]
    fn a_record_round_trips_and_only_a_record_carries_capabilities() {
        let mut m = sample();
        m.display_name = "Voicemail".into();
        m.kind = AppType::Background;
        m.has_idle = true;
        m.icon = "voicemail".into();
        m.icon_png = PNG.to_vec();
        m.icons_for = vec![("D60".into(), [PNG, b"color"].concat())];
        m.capabilities = vec![Capability::Ui, Capability::Pbx];
        let back = Manifest::from_record(&m.record()).unwrap();
        assert_eq!(back.display_name, "Voicemail");
        assert_eq!((back.kind, back.has_idle), (AppType::Background, true));
        assert!(back.has(Capability::Pbx) && !back.has(Capability::Calls));
        assert_eq!(back.icon, "voicemail");
        // A phone shows its model's icon, else the app's.
        assert_eq!(back.icon_for("D60"), [PNG, b"color"].concat());
        assert_eq!(back.icon_for("D40"), PNG);
        // The same bytes as an app's own section: the capabilities are not
        // the app's to declare.
        let own = Manifest::from_wasm(&module_with(&m.record())).unwrap();
        assert!(own.capabilities.is_empty());
    }

    #[test]
    fn the_section_is_found_and_replaced_in_a_component_too() {
        let m = sample();
        let mut module = module_with(&m.section());
        // A code section of five bytes.
        module.extend([10, 5, 1, 2, 3, 4, 5]);
        let mut component = b"\0asm\x0d\0\x01\0".to_vec();
        component.push(1);
        put_varint(&mut component, module.len() as u64);
        component.extend(&module);
        let found = Manifest::from_wasm(&component).unwrap();
        assert_eq!(found.name, "vm");
        assert_eq!(found.wasm, "vm.wasm");
        assert_eq!(code_size(&component), 5);
        assert_eq!(code_size(b"not WebAssembly"), 15);

        let mut other = found.clone();
        other.name = "other".into();
        let renamed = with_section(&component, &other.section()).unwrap();
        assert_eq!(Manifest::from_wasm(&renamed).unwrap().name, "other");
        assert_eq!(code_size(&renamed), 5);
    }

    #[test]
    fn what_is_not_an_app_is_told_apart() {
        assert_eq!(Manifest::from_wasm(b"hello"), Err(ManifestError::NotWasm));
        assert_eq!(
            Manifest::from_wasm(b"PK\x03\x04....demo/app.json...."),
            Err(ManifestError::JavaScript)
        );
        assert_eq!(
            Manifest::from_wasm(b"\0asm\x01\0\0\0"),
            Err(ManifestError::NoManifest)
        );
        assert_eq!(
            Manifest::from_wasm(&module_with(&[8, 2, 0x12, 1, b'x'])),
            Err(ManifestError::Format(2))
        );
        assert!(matches!(
            Manifest::from_wasm(&module_with(&[8, 1, 0x12, 1, b'-'])),
            Err(ManifestError::BadName(_))
        ));
        // Kind 3 was "idle", which `has_idle` says.
        assert_eq!(
            Manifest::from_wasm(&module_with(&[8, 1, 0x12, 1, b'x', 0x30, 3])),
            Err(ManifestError::BadKind(3))
        );
        let mut m = sample();
        m.icon_png = b"GIF89a".to_vec();
        assert!(matches!(
            Manifest::from_wasm(&module_with(&m.section())),
            Err(ManifestError::BadIcon(_))
        ));
        let mut m = sample();
        m.icons_for = vec![("D99".into(), PNG.to_vec())];
        assert!(matches!(
            Manifest::from_wasm(&module_with(&m.section())),
            Err(ManifestError::BadIcon(_))
        ));
    }

    #[test]
    fn imports_name_capabilities() {
        assert_eq!(
            Capability::of_import("opendeskphone:app/ui@3.1.0"),
            Ok(Some(Capability::Ui))
        );
        assert_eq!(Capability::of_import("opendeskphone:app/runtime@3.1.0"), Ok(None));
        assert!(Capability::of_import("wasi:io/streams@0.2.0").is_err());
        assert!(Capability::of_import("opendeskphone:app/root@3.1.0").is_err());
    }
}
