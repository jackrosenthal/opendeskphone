//! Settings that only Open Desk Phone has:
//! `/nvdata/opendeskphone/settings.txtpb`.
//!
//! The file is a `opendeskphone.settings.v1.OpenDeskPhoneSettings` message
//! (see `proto/opendeskphone/settings/v1/settings.proto`) in protobuf text
//! format, read and written with buffa's text-format support.  Stock never
//! reads it, and stock's factory reset deletes it along with every other file
//! it does not keep, which puts these settings back to their defaults.
//!
//! Every field has explicit presence.  Only fields that are set are written;
//! an absent field takes its default, which the accessors below apply.
//!
//! Fields this firmware does not know (written by a newer one) are kept:
//! their text is carried through a read-modify-write untouched.  Comments
//! are not kept, apart from those that follow such a field.

use std::io;

use buffa::Enumeration as _;
use buffa::text::{TextDecoder, TextFormat, encode_to_string_pretty};

use crate::nvdata::Nvdata;

#[allow(unused_imports, clippy::all)]
pub(crate) mod pb {
    include!(concat!(env!("OUT_DIR"), "/opendeskphone.settings.v1.rs"));
}

pub use pb::{App, DisabledApps, OpenDeskPhoneSettings, ThemeSync};

/// The settings file, relative to the nvdata root.
pub const SETTINGS_FILE: &str = "opendeskphone/settings.txtpb";
/// Where a settings file that does not parse is moved to.
pub const SETTINGS_FILE_BAD: &str = "opendeskphone/settings.txtpb.bad";

/// Field names of [`OpenDeskPhoneSettings`]; anything else in the file is
/// carried through as text.  A test checks this list against the generated
/// code.
const KNOWN_FIELDS: &[&str] =
    &["ssh_enabled", "ignore_provisioned_firmware", "serial_console", "metrics_enabled", "idle_softkeys", "disabled_apps", "theme", "theme_sync"];

const HEADER: &str = "# Open Desk Phone settings (opendeskphone.settings.v1.OpenDeskPhoneSettings, protobuf text format).\n\
                      # A field that is absent takes its default.\n";

/// Where an effective setting value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingSource {
    /// Neither set locally nor provisioned.
    Default,
    /// Set on the phone (`/nvdata/opendeskphone/settings.txtpb`).
    Local,
    /// Supplied by the provisioning server.
    Provisioned,
}

/// An effective setting: its value, where it came from, and whether the
/// provisioning server froze it (local changes then have no effect).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: SettingSource,
    pub locked: bool,
}

/// Combine the sources the way stock layers a local UI setting over a
/// provisioned one: a frozen provisioned value wins, then the local value,
/// then an ordinary provisioned value, then the default.
pub(crate) fn resolve<T>(local: Option<T>, provisioned: Option<T>, frozen: bool, default: T) -> Resolved<T> {
    match (provisioned, local) {
        (Some(p), _) if frozen => Resolved { value: p, source: SettingSource::Provisioned, locked: true },
        (_, Some(l)) => Resolved { value: l, source: SettingSource::Local, locked: false },
        (Some(p), None) => Resolved { value: p, source: SettingSource::Provisioned, locked: false },
        (None, None) => Resolved { value: default, source: SettingSource::Default, locked: false },
    }
}

/// Setting IDs a provisioning server uses for the [`OpenDeskPhoneSettings`]
/// fields.
pub const PROVISIONED_SSH_ENABLED: &str = "opendeskphone_ssh_enabled";
pub const PROVISIONED_IGNORE_FIRMWARE: &str = "opendeskphone_ignore_provisioned_firmware";
pub const PROVISIONED_SERIAL_CONSOLE: &str = "opendeskphone_serial_console";
pub const PROVISIONED_METRICS_ENABLED: &str = "opendeskphone_metrics_enabled";
pub const PROVISIONED_IDLE_SOFTKEYS: &str = "opendeskphone_idle_softkeys";
pub const PROVISIONED_DISABLED_APPS: &str = "opendeskphone_disabled_apps";
pub const PROVISIONED_THEME: &str = "opendeskphone_theme";

/// What the theme of a color model is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    /// A built-in theme by name; empty is the default one.  The name
    /// is as it was set, trimmed and in lower case: whether the firmware
    /// has such a theme is the UI's to say (it shows the default one
    /// for a name it does not know).
    Builtin(String),
    /// The provisioning server's picture (`wallpaper_file`), with the
    /// palette file that came with it, if one did.
    Picture,
}

/// The theme in effect, where the choice came from, and whether the
/// provisioning server froze it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub choice: ThemeChoice,
    pub source: SettingSource,
    pub locked: bool,
    /// The built-in theme shown while a chosen picture cannot be (not
    /// downloaded yet, or not a picture): the name set on the phone, else
    /// the provisioned name, else empty.
    pub builtin: String,
}

/// Parse the provisioned list of app names, which is the XML
/// configuration's comma-separated value (`calendar,parking`).  Blank
/// items are skipped, and a name is kept once.
pub fn parse_app_list(list: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// A function an idle softkey can have.  The discriminants are the values
/// of the `IdleSoftkey.Function` enum of `settings.proto`, which the web
/// interface's messages use too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleFunction {
    /// The call log; labeled "Missed Calls" while there are unseen ones.
    CallLog = 1,
    Contacts = 2,
    /// Phone information.
    Status = 3,
    Menu = 4,
    /// The app launcher.
    Apps = 5,
    /// Switches Do Not Disturb.
    Dnd = 6,
    /// The call forwarding menu.
    Forward = 7,
    Redial = 8,
    /// The theme selector.  Only the color models have themes; on the
    /// others the softkey is not shown.
    Theme = 9,
    Voicemail = 10,
}

impl IdleFunction {
    pub const ALL: [IdleFunction; 10] = {
        use IdleFunction::*;
        [CallLog, Contacts, Status, Menu, Apps, Dnd, Forward, Redial, Voicemail, Theme]
    };

    /// The function's name in the provisioned (XML) list.
    pub fn name(self) -> &'static str {
        match self {
            IdleFunction::CallLog => "call_log",
            IdleFunction::Contacts => "contacts",
            IdleFunction::Status => "status",
            IdleFunction::Menu => "menu",
            IdleFunction::Apps => "apps",
            IdleFunction::Dnd => "dnd",
            IdleFunction::Forward => "forward",
            IdleFunction::Redial => "redial",
            IdleFunction::Theme => "theme",
            IdleFunction::Voicemail => "voicemail",
        }
    }

    /// The value of the protobuf `IdleSoftkey.Function` enum.
    pub fn number(self) -> i32 {
        self as i32
    }

    /// The function a protobuf `IdleSoftkey.Function` value stands for.
    pub fn from_number(number: i32) -> Option<IdleFunction> {
        Self::ALL.into_iter().find(|f| f.number() == number)
    }

    /// Whether `model` has the function: every model has every one but
    /// the theme selector, which only the color models have.
    pub fn on_model(self, model: crate::Model) -> bool {
        self != IdleFunction::Theme || model.has_themes()
    }
}

/// A softkey of the idle screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdleSoftkey {
    Function(IdleFunction),
    /// Opens the app of this name.
    App(String),
}

/// True for a name an app softkey can carry.
pub fn valid_app_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

impl IdleSoftkey {
    /// The entry of the settings file for this softkey.
    pub fn to_setting(&self) -> pb::IdleSoftkey {
        use pb::idle_softkey::{Action, Function};
        let action = match self {
            IdleSoftkey::Function(f) => {
                Action::Function(Function::from_i32(f.number()).expect("the proto enum has every function").into())
            }
            IdleSoftkey::App(name) => Action::App(Box::new(App { name: name.clone() })),
        };
        pb::IdleSoftkey { action: Some(action) }
    }

    /// The softkey an entry of the settings file stands for; `None` for
    /// an entry that is empty, or names no app.
    pub fn from_setting(entry: &pb::IdleSoftkey) -> Option<IdleSoftkey> {
        use pb::idle_softkey::Action;
        match entry.action.as_ref()? {
            Action::Function(f) => IdleFunction::from_number(f.to_i32()).map(IdleSoftkey::Function),
            Action::App(app) => valid_app_name(&app.name).then(|| IdleSoftkey::App(app.name.clone())),
        }
    }
}

impl OpenDeskPhoneSettings {
    /// True when the two say the same.  Not `==`: to the generated
    /// message type a message field that is present and empty
    /// (`disabled_apps {}`) equals an absent one, and those are different
    /// settings.
    pub fn same_as(&self, other: &OpenDeskPhoneSettings) -> bool {
        self == other
            && self.disabled_apps.is_set() == other.disabled_apps.is_set()
            && self.theme_sync.is_set() == other.theme_sync.is_set()
    }

    /// The idle softkeys set in the file; entries that say nothing are
    /// skipped.
    pub fn local_idle_softkeys(&self) -> Vec<IdleSoftkey> {
        self.idle_softkeys.iter().filter_map(IdleSoftkey::from_setting).collect()
    }

    /// The theme chosen on the phone, as the configuration reads it
    /// (trimmed, in lower case); `None` when none is.
    pub fn local_theme(&self) -> Option<String> {
        self.theme.as_deref().map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty())
    }

    /// Set the idle softkeys; none puts the provisioned or default ones
    /// back.
    pub fn set_idle_softkeys(&mut self, keys: &[IdleSoftkey]) {
        self.idle_softkeys = keys.iter().map(IdleSoftkey::to_setting).collect();
    }

    /// The apps the file switches off, each name once; `None` when the
    /// file does not say.
    pub fn local_disabled_apps(&self) -> Option<Vec<String>> {
        let list = self.disabled_apps.as_option()?;
        let mut names: Vec<String> = Vec::new();
        for name in list.apps.iter().map(|a| a.name.trim()).filter(|n| !n.is_empty()) {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        Some(names)
    }

    /// Set the apps that are switched off.  An empty list switches every
    /// app on, whatever the provisioning server said; `None` leaves it to
    /// the server.
    pub fn set_disabled_apps(&mut self, names: Option<&[String]>) {
        self.disabled_apps = names
            .map(|names| DisabledApps { apps: names.iter().map(|name| App { name: name.clone() }).collect() })
            .into();
    }
}

/// The D80's defaults are the home buttons that the touch skin shows
/// before the phone has a configuration (`IdleSoftkey::touch_defaults` in
/// opendeskphone-ui), so the two lists must stay the same.
pub fn default_idle_softkeys(model: crate::Model) -> Vec<IdleSoftkey> {
    use IdleFunction::*;
    let keys: &[IdleFunction] = match model {
        crate::Model::D80 => &[CallLog, Contacts, Voicemail, Redial, Dnd, Forward, Apps, Menu],
        _ => &[CallLog, Contacts, Status, Menu, Apps, Dnd, Forward, Theme],
    };
    keys.iter().copied().filter(|f| f.on_model(model)).map(IdleSoftkey::Function).collect()
}

/// The idle screen's softkeys in effect, where they came from, and whether
/// the provisioning server froze them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdleSoftkeys {
    pub keys: Vec<IdleSoftkey>,
    pub source: SettingSource,
    pub locked: bool,
    pub skipped: Vec<String>,
}

/// Parse the provisioned idle softkey list, which is the XML
/// configuration's comma-separated value (`call_log,contacts,app:parking`):
/// function names ([`IdleFunction::name`]) and `app:<name>`.  Blank items
/// are skipped, and so are items that name no softkey, which are returned
/// beside the softkeys.
pub fn parse_idle_softkeys(list: &str) -> (Vec<IdleSoftkey>, Vec<String>) {
    let mut keys = Vec::new();
    let mut skipped = Vec::new();
    for item in list.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        if let Some(f) = IdleFunction::ALL.into_iter().find(|f| f.name() == item) {
            keys.push(IdleSoftkey::Function(f));
        } else if let Some(app) = item.strip_prefix("app:").filter(|name| valid_app_name(name)) {
            keys.push(IdleSoftkey::App(app.to_owned()));
        } else {
            skipped.push(item.to_owned());
        }
    }
    (keys, skipped)
}

/// Setting IDs a provisioning server uses for a static IPv4 arrangement:
/// the address as `a.b.c.d/prefix length`, the gateway, and the DNS servers
/// as a comma-separated list.  Stock firmware does not know them.  (Stock's
/// own `network_*` IDs cannot carry a provisioned address: every phone
/// holds them in `ui-config.xml`, which is merged after the fetched
/// configuration.)
pub const PROVISIONED_STATIC_ADDRESS: &str = "opendeskphone_static_address";
pub const PROVISIONED_STATIC_GATEWAY: &str = "opendeskphone_static_gateway";
pub const PROVISIONED_STATIC_DNS: &str = "opendeskphone_static_dns";
/// Element a provisioning server uses for SSH public keys:
/// `<opendeskphone_ssh_authorized_keys><key id="1" value="ssh-ed25519 AAAA... name"/></opendeskphone_ssh_authorized_keys>`.
pub const PROVISIONED_SSH_KEYS: &str = "opendeskphone_ssh_authorized_keys";
/// Where the provisioned SSH keys are mirrored for the SSH server, relative
/// to the nvdata root, in `authorized_keys` format.  Rewritten from the
/// merged configuration on every merge and removed when there are none.
pub const PROVISIONED_SSH_KEYS_FILE: &str = "opendeskphone/provisioned_authorized_keys";

/// True for a plain public-key line: `<type> <base64> [comment]`, one line,
/// no options field.
pub fn valid_public_key_line(line: &str) -> bool {
    let mut f = line.split(' ');
    let kind = f.next().unwrap_or("");
    let blob = f.next().unwrap_or("");
    !line.contains(['\n', '\r', '\0'])
        && (kind.starts_with("ssh-") || kind.starts_with("ecdsa-sha2-") || kind.starts_with("sk-"))
        && blob.len() >= 16
        && blob.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='))
}

/// The settings file as read: the typed message plus the text of fields
/// this firmware does not know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsDocument {
    pub settings: OpenDeskPhoneSettings,
    /// Verbatim text of unknown top-level fields, one per entry.
    unknown: Vec<String>,
}

impl SettingsDocument {
    /// Parse the file's text.
    pub fn parse(text: &str) -> Result<Self, buffa::text::ParseError> {
        let mut settings = OpenDeskPhoneSettings::default();
        settings.merge_text(&mut TextDecoder::new(text))?;
        // Second pass over the top level to find where each field starts
        // (names are borrowed from the input), so unknown ones can be kept.
        let mut dec = TextDecoder::new(text);
        let mut fields: Vec<(usize, bool)> = Vec::new();
        while let Some(name) = dec.read_field_name()? {
            let start = name.as_ptr() as usize - text.as_ptr() as usize;
            fields.push((start, KNOWN_FIELDS.contains(&name)));
            dec.skip_value()?;
        }
        let mut unknown = Vec::new();
        for (i, (start, known)) in fields.iter().enumerate() {
            if !known {
                let end = fields.get(i + 1).map_or(text.len(), |f| f.0);
                unknown.push(text[*start..end].trim_end().to_owned());
            }
        }
        Ok(SettingsDocument { settings, unknown })
    }

    /// Text of the fields this firmware does not know.
    pub fn unknown_fields(&self) -> &[String] {
        &self.unknown
    }

    /// The file's text: a header comment, the set fields, then the unknown
    /// fields as they were read.
    pub fn to_text(&self) -> String {
        let mut out = String::from(HEADER);
        out.push_str(&encode_to_string_pretty(&self.settings));
        for u in &self.unknown {
            out.push_str(u);
            out.push('\n');
        }
        out
    }

    /// Read the file.  A missing file gives the defaults.  A file that does
    /// not parse is logged, moved aside to [`SETTINGS_FILE_BAD`], and
    /// replaced by the defaults, so a bad file never blocks configuration.
    pub fn load(nv: &Nvdata) -> io::Result<Self> {
        let Some(text) = nv.read_string(SETTINGS_FILE)? else { return Ok(Self::default()) };
        match Self::parse(&text) {
            Ok(doc) => Ok(doc),
            Err(e) => {
                log::error!(
                    "{SETTINGS_FILE} does not parse ({e}); kept as {SETTINGS_FILE_BAD}, using defaults"
                );
                if let Err(e) = std::fs::rename(nv.path(SETTINGS_FILE), nv.path(SETTINGS_FILE_BAD)) {
                    log::error!("could not move {SETTINGS_FILE} aside: {e}");
                }
                Ok(Self::default())
            }
        }
    }

    /// Write the file atomically (temp file, `fsync`, rename).
    pub fn save(&self, nv: &Nvdata) -> io::Result<()> {
        nv.write(SETTINGS_FILE, self.to_text().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_field_list_matches_the_message() {
        let all = OpenDeskPhoneSettings {
            ssh_enabled: Some(true),
            ignore_provisioned_firmware: Some(true),
            serial_console: Some(true),
            metrics_enabled: Some(true),
            idle_softkeys: vec![IdleSoftkey::Function(IdleFunction::Menu).to_setting()],
            disabled_apps: Some(DisabledApps::default()).into(),
            theme: Some("dusk".into()),
            theme_sync: Some(ThemeSync::default()).into(),
        };
        let text = encode_to_string_pretty(&all);
        // The top-level lines, `name: value` and `name {`; the encoder
        // writes the repeated fields last.
        let mut names: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with([' ', '}']))
            .map(|l| l.split([':', ' ']).next().unwrap())
            .collect();
        let mut known = KNOWN_FIELDS.to_vec();
        names.sort();
        known.sort();
        assert_eq!(names, known);
    }

    #[test]
    fn presence_and_defaults() {
        let d = SettingsDocument::parse("").unwrap();
        assert_eq!(d.settings, OpenDeskPhoneSettings::default());
        // Nothing set: nothing but the header is written.
        assert_eq!(d.to_text(), HEADER);

        let d = SettingsDocument::parse("ssh_enabled: false\n").unwrap();
        assert_eq!(d.settings.ssh_enabled, Some(false));
        assert_eq!(d.settings.ignore_provisioned_firmware, None);
        assert_eq!(d.to_text(), format!("{HEADER}ssh_enabled: false\n"));
    }

    #[test]
    fn unknown_fields_survive() {
        let text = "# c\nfuture_number: 7  # why\nssh_enabled: false\nfuture_msg {\n  a: \"x y\"\n  b: [1, 2]\n}\n";
        let mut d = SettingsDocument::parse(text).unwrap();
        assert_eq!(d.unknown_fields(), ["future_number: 7  # why", "future_msg {\n  a: \"x y\"\n  b: [1, 2]\n}"]);
        d.settings.ignore_provisioned_firmware = Some(true);
        let again = SettingsDocument::parse(&d.to_text()).unwrap();
        assert_eq!(again, d);
        assert_eq!(
            d.to_text(),
            format!("{HEADER}ssh_enabled: false\nignore_provisioned_firmware: true\nfuture_number: 7  # why\nfuture_msg {{\n  a: \"x y\"\n  b: [1, 2]\n}}\n")
        );
    }

    #[test]
    fn precedence() {
        use SettingSource::*;
        let r = |l, p, f| resolve(l, p, f, true);
        assert_eq!(r(None, None, false), Resolved { value: true, source: Default, locked: false });
        assert_eq!(r(None, Some(false), false), Resolved { value: false, source: Provisioned, locked: false });
        assert_eq!(r(Some(true), Some(false), false), Resolved { value: true, source: Local, locked: false });
        assert_eq!(r(Some(true), Some(false), true), Resolved { value: false, source: Provisioned, locked: true });
        assert_eq!(r(Some(false), None, true), Resolved { value: false, source: Local, locked: false });
    }

    #[test]
    fn provisioned_idle_softkey_lists() {
        use IdleFunction::*;
        let f = IdleSoftkey::Function;
        let app = |name: &str| IdleSoftkey::App(name.to_owned());
        let names = parse_idle_softkeys;
        let skipped = |items: &[&str]| items.iter().map(|i| i.to_string()).collect::<Vec<_>>();
        assert_eq!(names("call_log, contacts,,app:parking ,dnd"), (vec![f(CallLog), f(Contacts), app("parking"), f(Dnd)], skipped(&[])));
        assert_eq!(names(""), (vec![], skipped(&[])));
        assert_eq!(names("menu,lunch"), (vec![f(Menu)], skipped(&["lunch"])));
        assert_eq!(names("app:, voicemail ,app:a b"), (vec![f(Voicemail)], skipped(&["app:", "app:a b"])));
        // Stock's keymap actions are not softkey names.
        assert_eq!(names("info,missed_or_call_log"), (vec![], skipped(&["info", "missed_or_call_log"])));
        let all = IdleFunction::ALL.map(IdleFunction::name).join(",");
        assert_eq!(names(&all), (IdleFunction::ALL.map(f).to_vec(), skipped(&[])));
    }

    #[test]
    fn provisioned_app_lists() {
        assert_eq!(parse_app_list(" calendar,,parking , calendar"), ["calendar", "parking"]);
        assert!(parse_app_list(" , ").is_empty());
    }

    /// The lists as the file has them: one entry per item, written and
    /// read back, and written by hand.
    #[test]
    fn lists_in_the_file() {
        use IdleFunction::*;
        let keys = [IdleSoftkey::Function(CallLog), IdleSoftkey::App("parking".into()), IdleSoftkey::Function(Redial)];
        let mut d = SettingsDocument::default();
        d.settings.set_idle_softkeys(&keys);
        d.settings.set_disabled_apps(Some(&["calendar".to_owned(), "parking".to_owned()]));
        let text = d.to_text();
        assert_eq!(
            text,
            format!(
                "{HEADER}disabled_apps {{\n  apps {{\n    name: \"calendar\"\n  }}\n  apps {{\n    name: \"parking\"\n  }}\n}}\n\
                 idle_softkeys {{\n  function: FUNCTION_CALL_LOG\n}}\nidle_softkeys {{\n  app {{\n    name: \"parking\"\n  }}\n}}\n\
                 idle_softkeys {{\n  function: FUNCTION_REDIAL\n}}\n"
            )
        );
        let again = SettingsDocument::parse(&text).unwrap();
        assert_eq!(again, d);
        assert_eq!(again.settings.local_idle_softkeys(), keys);
        assert_eq!(again.settings.local_disabled_apps().unwrap(), ["calendar", "parking"]);

        // Every function has a value in the proto enum.
        for f in IdleFunction::ALL {
            let entry = IdleSoftkey::Function(f).to_setting();
            assert_eq!(IdleSoftkey::from_setting(&entry), Some(IdleSoftkey::Function(f)));
        }

        // By hand, on one line each; an empty entry and an app without a
        // name say nothing, and a name is kept once.
        let hand = "idle_softkeys { function: FUNCTION_MENU }\nidle_softkeys {}\nidle_softkeys { app {} }\n\
                    idle_softkeys { app { name: \"queues\" } }\ndisabled_apps { apps { name: \"status\" } apps { name: \"status\" } }\n";
        let d = SettingsDocument::parse(hand).unwrap();
        assert_eq!(d.settings.local_idle_softkeys(), [IdleSoftkey::Function(Menu), IdleSoftkey::App("queues".into())]);
        assert_eq!(d.settings.local_disabled_apps().unwrap(), ["status"]);

        // Present and empty is not absent.
        let d = SettingsDocument::parse("disabled_apps {}\n").unwrap();
        assert_eq!(d.settings.local_disabled_apps(), Some(vec![]));
        assert_eq!(d.to_text(), format!("{HEADER}disabled_apps {{}}\n"));
        let absent = SettingsDocument::parse("").unwrap().settings;
        assert_eq!(absent.local_disabled_apps(), None);
        assert!(!absent.same_as(&d.settings) && absent.same_as(&absent.clone()) && d.settings.same_as(&d.settings.clone()));

        // A function this firmware does not know, and a list as a string,
        // do not parse.
        assert!(SettingsDocument::parse("idle_softkeys { function: FUNCTION_LUNCH }").is_err());
        assert!(SettingsDocument::parse("idle_softkeys: \"menu,dnd\"").is_err());
    }

    #[test]
    fn public_key_lines() {
        assert!(valid_public_key_line("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ7x alice@laptop"));
        assert!(valid_public_key_line("ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY="));
        assert!(!valid_public_key_line("command=\"x\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ7x"));
        assert!(!valid_public_key_line("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ7x\nssh-rsa AAAA"));
        assert!(!valid_public_key_line("ssh-ed25519"));
        assert!(!valid_public_key_line(""));
    }

    #[test]
    fn malformed_text_is_an_error() {
        assert!(SettingsDocument::parse("ssh_enabled: maybe").is_err());
        assert!(SettingsDocument::parse("ssh_enabled: true\nfuture {").is_err());
        assert!(SettingsDocument::parse("<config/>").is_err());
    }
}
