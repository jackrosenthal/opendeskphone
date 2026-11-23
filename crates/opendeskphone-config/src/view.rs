//! A typed, read-only view of one merged configuration generation.
//!
//! The merged XML stays the source of truth ([`Config::root`]); the typed
//! accessors parse what the rest of the firmware needs out of it.  Anything
//! not covered by a typed accessor is reachable through [`Config::setting`]
//! and the raw elements.

use std::net::Ipv4Addr;

use crate::model::Model;
use crate::settings::{self, OpenDeskPhoneSettings, IdleSoftkeys, Resolved, SettingSource};
use crate::xml::Element;

/// One merged configuration.  Cheap to share behind an `Arc`.
#[derive(Clone, Debug)]
pub struct Config {
    pub(crate) root: Element,
    pub(crate) generation: u64,
    pub(crate) model: Model,
    pub(crate) selected_network: Option<String>,
    pub(crate) settings: OpenDeskPhoneSettings,
}

fn truthy(v: &str) -> bool {
    matches!(v.trim(), "1" | "true" | "yes" | "on")
}

fn s(e: &Element, name: &str) -> String {
    e.attr(name).unwrap_or("").to_owned()
}

fn opt(e: &Element, name: &str) -> Option<String> {
    e.attr(name).filter(|v| !v.is_empty()).map(str::to_owned)
}

fn num<T: std::str::FromStr>(e: &Element, name: &str) -> Option<T> {
    e.attr(name).and_then(|v| v.trim().parse().ok())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Transport {
    #[default]
    Udp,
    Tcp,
    Tls,
}

impl Transport {
    fn parse(v: Option<&str>) -> Transport {
        match v.map(str::to_ascii_lowercase).as_deref() {
            Some("tcp") => Transport::Tcp,
            Some("tls") => Transport::Tls,
            _ => Transport::Udp,
        }
    }
}

/// A `host_primary` or `host_alternate` record of an account.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Host {
    pub server: String,
    pub port: u16,
    pub transport: Transport,
    /// For example, `sdes`.
    pub media_encryption: Option<String>,
    /// Registration lifetime in seconds (stock default 300).
    pub reregister: u32,
    /// Seconds between registration retries (stock default 20).
    pub retry: u32,
    pub num_retries: Option<u32>,
    /// Host-specific credentials; `None` falls back to the account's.
    pub authname: Option<String>,
    pub password: Option<String>,
    pub mwi_subscription_uri: Option<String>,
    pub need_mwi_subscription: Option<bool>,
}

/// A SIP account (`<accounts><account .../></accounts>`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Account {
    /// Position in the line keys, from `index` (0-based) or document order.
    pub index: u32,
    pub account_id: String,
    pub username: String,
    /// Authentication name: `authname`, else `username`.
    pub authname: String,
    pub password: String,
    pub passcode: String,
    pub caller_id: String,
    pub number: String,
    pub display: String,
    pub line_label: String,
    pub dial_plan: Option<String>,
    pub register: bool,
    /// `status="1"` means the account is enabled.
    pub enabled: bool,
    pub kind: String,
    pub outbound_proxy: Option<String>,
    pub outbound_port: Option<u16>,
    pub plar_number: Option<String>,
    pub subscription_extension: Option<String>,
    pub visual_voicemail: bool,
    pub voicemail: Option<String>,
    pub needs_mwi_subscription: Option<bool>,
    pub mwi_subscription_uri: Option<String>,
    pub primary: Option<Host>,
    pub alternate: Option<Host>,
    /// The account element, for attributes and children not modeled here.
    pub raw: Element,
}

fn host(e: &Element, account: &Element) -> Host {
    Host {
        server: s(e, "server"),
        port: num(e, "port").unwrap_or(match Transport::parse(e.attr("transport")) {
            Transport::Tls => 5061,
            _ => 5060,
        }),
        transport: Transport::parse(e.attr("transport").or(account.attr("transport"))),
        media_encryption: opt(e, "media_encryption"),
        reregister: num(e, "reregister").unwrap_or(300),
        retry: num(e, "retry").unwrap_or(20),
        num_retries: num(e, "num_retries"),
        authname: opt(e, "authname"),
        password: opt(e, "password"),
        mwi_subscription_uri: opt(e, "mwi_subscription_uri"),
        need_mwi_subscription: e.attr("needMwiSubscription").map(truthy),
    }
}

fn account(e: &Element, position: u32) -> Account {
    let username = s(e, "username");
    let mut primary = e.child("host_primary").map(|h| host(h, e));
    if primary.is_none() && e.attr("server").is_some_and(|v| !v.is_empty()) {
        // Locally edited accounts may carry only account-level attributes.
        primary = Some(Host {
            server: s(e, "server"),
            port: num(e, "port").unwrap_or(5060),
            transport: Transport::parse(e.attr("transport")),
            reregister: 300,
            retry: 20,
            ..Host::default()
        });
    }
    Account {
        index: num(e, "index").unwrap_or(position),
        account_id: opt(e, "account_id").unwrap_or_else(|| username.clone()),
        authname: opt(e, "authname").unwrap_or_else(|| username.clone()),
        password: s(e, "password"),
        passcode: s(e, "passcode"),
        caller_id: s(e, "caller_id"),
        number: opt(e, "number").unwrap_or_else(|| username.clone()),
        display: opt(e, "display").unwrap_or_else(|| username.clone()),
        line_label: opt(e, "line_label").unwrap_or_else(|| username.clone()),
        dial_plan: opt(e, "dial_plan"),
        register: e.attr("register").is_none_or(truthy),
        enabled: e.attr("status").is_none_or(truthy),
        kind: opt(e, "type").unwrap_or_else(|| "sip".to_owned()),
        outbound_proxy: opt(e, "outbound_proxy"),
        outbound_port: num(e, "outbound_port"),
        plar_number: opt(e, "plar_number"),
        subscription_extension: opt(e, "subscription_extension"),
        visual_voicemail: e.attr("visual_voicemail").is_some_and(truthy),
        voicemail: opt(e, "voicemail"),
        needs_mwi_subscription: e.attr("needMwiSubscription").map(truthy),
        mwi_subscription_uri: opt(e, "mwi_subscription_uri"),
        primary,
        alternate: e.child("host_alternate").map(|h| host(h, e)),
        username,
        raw: e.clone(),
    }
}

/// A codec entry.  [`Config::codecs`] returns them by descending priority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Codec {
    /// Stock name: `PCMU`, `PCMA`, `G722`, `G7221`, `G726-32`, `opus`,
    /// `G729`, `iLBC`, `L16`, `L16-256`.
    pub id: String,
    pub priority: u32,
    pub enabled: bool,
    /// Packetization time in milliseconds.
    pub packetization: u32,
    pub jitter_min: u32,
    pub jitter_max: u32,
    pub jitter_target: u32,
    pub raw: Element,
}

/// Wired network settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkSettings {
    pub dhcp: bool,
    pub static_ip: Option<Ipv4Addr>,
    pub subnet_mask: Option<Ipv4Addr>,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub domain: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_preferred: bool,
    pub ipv6_static: Option<String>,
    pub ipv6_mask: Option<String>,
    pub ipv6_gateway: Option<String>,
    pub disable_arping: bool,
    /// `LLDP`, `MANUAL`, `NONE`, and so on, as stored.
    pub vlan_discovery_mode: String,
    pub vlan_id: Option<u16>,
    pub pc_vlan_id: Option<u16>,
    pub pc_qos: u8,
    pub lan_port_mode: String,
    pub pc_port_mode: String,
    pub sip_dscp: u8,
    pub rtp_dscp: u8,
    pub sip_qos: u8,
    pub rtp_qos: u8,
    /// LLDP-supplied DSCP overrides; `None` when stored as -1.
    pub sip_dscp_lldp: Option<u8>,
    pub rtp_dscp_lldp: Option<u8>,
    pub dot1x_passthrough: bool,
    pub dot1x_eapol_on_disconnect: bool,
}

/// A static IPv4 arrangement in effect; see [`Config::static_ipv4`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticIpv4 {
    pub address: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub domain: Option<String>,
    /// [`SettingSource::Local`]: stock's network settings, as the phone's
    /// menu and the web UI write them.  [`SettingSource::Provisioned`]:
    /// the server's `opendeskphone_static_address`.
    pub source: SettingSource,
    /// The server froze its address: the local settings have no effect.
    pub locked: bool,
}

/// A host's address: not unspecified, loopback, multicast, or reserved.
fn host_ipv4(text: &str) -> Option<Ipv4Addr> {
    let a: Ipv4Addr = text.trim().parse().ok()?;
    let first = a.octets()[0];
    (first != 0 && first != 127 && first < 224).then_some(a)
}

/// `a.b.c.d/n` for a host on a network: `n` is 1 to 30, and the address is
/// neither the network's own nor its broadcast address.
fn host_with_prefix(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (address, prefix) = text.trim().split_once('/')?;
    let address = host_ipv4(address)?;
    let prefix: u8 = prefix.parse().ok().filter(|p| (1..=30).contains(p))?;
    let host_mask = u32::MAX >> prefix;
    let host = u32::from(address) & host_mask;
    (host != 0 && host != host_mask).then_some((address, prefix))
}

/// A provisioned network profile (`<networks><network .../></networks>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkProfile {
    pub id: String,
    pub display_name: String,
    pub cidr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SipSettings {
    pub udp_enabled: bool,
    pub udp_port: u16,
    pub tcp_enabled: bool,
    pub tcp_port: u16,
    pub tls_allowed: bool,
    pub tls_port: u16,
    pub tls_allow_wildcard_certs: bool,
    pub allow_insecure_ssl: bool,
    /// `any`, `host`, or `none`, as stored.
    pub accept_local_calls: String,
    pub default_dial_plan: String,
    pub udp_keepalive_interval: Option<u32>,
    pub enable_check_sync: bool,
    pub send_mac_with_useragent: bool,
    pub retransmit_until_response: bool,
    pub g729_authorized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeSettings {
    /// IANA zone name.
    pub time_zone: String,
    pub time_source: String,
    pub ntp_server: String,
    pub ntp_resync_secs: u32,
    pub ntp_prefer_option42: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSettings {
    /// Stock vocabulary, for example `information` or `debug`.
    pub level: String,
    pub remote_enabled: bool,
    pub server: Option<String>,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessSettings {
    pub web_ui_enabled: bool,
    pub ssh_enabled: bool,
    /// Whether a provisioned `<firmware>` entry is ignored (development
    /// aid; by default the server's firmware is followed, as on stock).
    pub ignore_provisioned_firmware: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Softkey {
    /// Softkey position, from the key's `id`.
    pub position: u32,
    pub action: String,
    pub label: Option<String>,
    pub args: Vec<String>,
}

/// A hard-key binding from the `all` keymap state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    /// For example, `line1` or `blf3`.
    pub key: String,
    /// `short` or `long`.
    pub length: Option<String>,
    pub action: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlfItem {
    /// `main` or `side`.
    pub location: String,
    pub index: u32,
    pub paging: bool,
    pub contact_id: Option<String>,
    pub app_id: Option<String>,
    pub blank: bool,
    pub raw: Element,
}

/// A contact list source (`<contacts url=".." id=".."/>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactSource {
    pub id: String,
    pub url: String,
    pub md5: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tone {
    pub id: String,
    pub display: String,
    /// `phone` for built-in tones; provisioned ones carry a `url`.
    pub kind: String,
    pub url: Option<String>,
    pub md5: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub alert_info: String,
    pub ringtone_id: String,
    /// `normal`, `answer`, `ring-answer`, and so on, as stored.
    pub ring_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ringtones {
    pub tones: Vec<Tone>,
    pub alerts: Vec<Alert>,
    /// `active_ringtone`, else `default_ringtone`.
    pub active: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firmware {
    pub model: String,
    pub version: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MulticastListener {
    pub id: String,
    pub name: String,
    pub addr: String,
    pub port: u16,
    pub priority: u32,
    /// What a page does while a call is connected: 0 does not interrupt
    /// it, 1 puts it on hold and plays the page, and 2 drops the page.
    pub interrupt_calls: u8,
    /// A page is also dropped while the connected call is with this
    /// extension (the page's own source).
    pub extension: String,
}

/// A group this phone can page: a `<broadcast>` in `<multicastpage>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MulticastBroadcast {
    pub id: String,
    pub name: String,
    pub addr: String,
    pub port: u16,
    /// `PCMU` (the default), `PCMA`, or `G722`.
    pub codec: String,
}

/// Lamp behavior for a line state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indicator {
    /// `green`, `red`, or `amber`.
    pub color: String,
    /// `off`, `on`, `slow`, or `fast`.
    pub state: String,
}

/// Parse BLF items from a `<config><smart_blf><blf_items>` document, which
/// is the shape of both `/nvdata/blf_items.xml` and the merged fallback.
pub fn parse_blf_items(config: &Element) -> Vec<BlfItem> {
    let Some(items) = config.child("smart_blf").and_then(|b| b.child("blf_items")) else {
        return Vec::new();
    };
    items
        .children_named("blf_item")
        .map(|e| BlfItem {
            location: opt(e, "location").unwrap_or_else(|| "main".to_owned()),
            index: num(e, "index").unwrap_or(0),
            paging: e.attr("paging").is_none_or(truthy),
            contact_id: opt(e, "contact_id"),
            app_id: opt(e, "app_id"),
            blank: e.attr("blank").is_some_and(truthy),
            raw: e.clone(),
        })
        .collect()
}

impl Config {
    /// Wrap an already merged tree (for tests and tools).
    pub fn from_merged(root: Element, model: Model, selected_network: Option<String>) -> Self {
        Config { root, generation: 0, model, selected_network, settings: OpenDeskPhoneSettings::default() }
    }

    /// The locally stored Open Desk Phone settings
    /// (`/nvdata/opendeskphone/settings.txtpb`) as of this generation: `None`
    /// means "not set locally".  For what is in effect, with provisioning
    /// and defaults applied, use [`Config::ssh_enabled`] and
    /// [`Config::ignore_provisioned_firmware`].
    pub fn opendeskphone_settings(&self) -> &OpenDeskPhoneSettings {
        &self.settings
    }

    fn provisioned_bool(&self, id: &str) -> (Option<bool>, bool) {
        let v = self.setting(id).filter(|v| !v.trim().is_empty()).map(truthy);
        (v, v.is_some() && self.setting_frozen(id))
    }

    /// Whether the SSH server runs (default off), and where that came from.
    pub fn ssh_enabled(&self) -> Resolved<bool> {
        let (p, frozen) = self.provisioned_bool(settings::PROVISIONED_SSH_ENABLED);
        settings::resolve(self.settings.ssh_enabled, p, frozen, false)
    }

    /// Whether provisioned firmware is ignored (default off: the server's
    /// firmware is followed, as on stock).
    pub fn ignore_provisioned_firmware(&self) -> Resolved<bool> {
        let (p, frozen) = self.provisioned_bool(settings::PROVISIONED_IGNORE_FIRMWARE);
        settings::resolve(self.settings.ignore_provisioned_firmware, p, frozen, false)
    }

    /// Whether the serial console has UART A (default off).  When off, the
    /// UART belongs to the EHS headset adapter (setting `ehs`).
    pub fn serial_console(&self) -> Resolved<bool> {
        let (p, frozen) = self.provisioned_bool(settings::PROVISIONED_SERIAL_CONSOLE);
        settings::resolve(self.settings.serial_console, p, frozen, false)
    }

    /// Whether the unauthenticated `GET /metrics` endpoint is served
    /// (default on).  Independent of `web_ui_enabled`.
    pub fn metrics_enabled(&self) -> Resolved<bool> {
        let (p, frozen) = self.provisioned_bool(settings::PROVISIONED_METRICS_ENABLED);
        settings::resolve(self.settings.metrics_enabled, p, frozen, true)
    }

    /// The idle screen's softkeys.  The sources combine as for the other
    /// Open Desk Phone settings: a frozen provisioned list wins, then the local
    /// list, then an ordinary provisioned list; with none of those, the
    /// model's default list ([`settings::default_idle_softkeys`]).  A
    /// provisioned list with no softkey left counts as not set.  The `idle`
    /// keymap state is not used: a DPMA server sends one to every phone,
    /// whatever its administrator set, so it does not count as a list that
    /// was set.
    pub fn idle_softkeys(&self) -> IdleSoftkeys {
        let id = settings::PROVISIONED_IDLE_SOFTKEYS;
        let (provisioned, skipped) = self.setting(id).map(settings::parse_idle_softkeys).unwrap_or_default();
        let provisioned = Some(provisioned).filter(|k| !k.is_empty());
        let frozen = provisioned.is_some() && self.setting_frozen(id);
        let local = Some(self.settings.local_idle_softkeys()).filter(|k| !k.is_empty());
        let (keys, source, locked) = match (provisioned, local) {
            (Some(keys), _) if frozen => (keys, SettingSource::Provisioned, true),
            (_, Some(keys)) => (keys, SettingSource::Local, false),
            (Some(keys), None) => (keys, SettingSource::Provisioned, false),
            (None, None) => (settings::default_idle_softkeys(self.model), SettingSource::Default, false),
        };
        IdleSoftkeys { keys, source, locked, skipped }
    }

    /// The built-in apps that are switched off (default: none): such an
    /// app is not in the phone's app list and cannot be opened.  The
    /// sources combine as for the other Open Desk Phone settings; a local list
    /// may be empty, which switches every app on.
    pub fn disabled_apps(&self) -> Resolved<Vec<String>> {
        let id = settings::PROVISIONED_DISABLED_APPS;
        let provisioned = self.setting(id).filter(|v| !v.trim().is_empty()).map(settings::parse_app_list);
        let frozen = provisioned.is_some() && self.setting_frozen(id);
        let local = self.settings.local_disabled_apps();
        settings::resolve(local, provisioned, frozen, Vec::new())
    }

    /// The theme of a color model.  The provisioning server can name a
    /// built-in one (`opendeskphone_theme`) or supply a picture (stock's
    /// `wallpaper_file`, with a `url`); of the two, the picture goes
    /// first.  Otherwise the sources combine as for the other Open Desk Phone
    /// settings: what the server froze wins, then the name chosen on the
    /// phone, then what the server supplies unfrozen, then the default.
    pub fn theme(&self) -> settings::Theme {
        use settings::{Theme, ThemeChoice};
        let name = |v: Option<&str>| v.map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty());
        let picture = self.setting_attr(crate::palette::WALLPAPER_SETTING, "url").is_some_and(|u| !u.trim().is_empty());
        let picture_frozen = picture && self.setting_frozen(crate::palette::WALLPAPER_SETTING);
        let provisioned = name(self.setting(settings::PROVISIONED_THEME));
        let name_frozen = provisioned.is_some() && self.setting_frozen(settings::PROVISIONED_THEME);
        let local = self.settings.local_theme();
        let builtin = local.clone().or_else(|| provisioned.clone()).unwrap_or_default();
        let (choice, source, locked) = if picture_frozen {
            (ThemeChoice::Picture, SettingSource::Provisioned, true)
        } else if let (Some(p), true) = (&provisioned, name_frozen) {
            (ThemeChoice::Builtin(p.clone()), SettingSource::Provisioned, true)
        } else if let Some(l) = local {
            (ThemeChoice::Builtin(l), SettingSource::Local, false)
        } else if picture {
            (ThemeChoice::Picture, SettingSource::Provisioned, false)
        } else if let Some(p) = provisioned {
            (ThemeChoice::Builtin(p), SettingSource::Provisioned, false)
        } else {
            (ThemeChoice::Builtin(String::new()), SettingSource::Default, false)
        };
        Theme { choice, source, locked, builtin }
    }

    /// Whether the provisioning server names a palette file for its
    /// wallpaper picture (`opendeskphone_palette_file`, with a `url`).
    pub fn palette_file_named(&self) -> bool {
        self.setting_attr(crate::palette::PALETTE_SETTING, "url").is_some_and(|u| !u.trim().is_empty())
    }

    /// The static IPv4 arrangement in effect, or `None` for DHCP.
    ///
    /// The sources combine as for the other Open Desk Phone settings: a frozen
    /// provisioned address wins, then a static address in stock's network
    /// settings (`network_enable_dhcp` off with an address and a mask),
    /// then an ordinary provisioned address.  Stock's settings saying DHCP
    /// is their default and counts as nothing set.  A provisioned address
    /// that is not `a.b.c.d/n` for a host is ignored, gateway and DNS
    /// servers with it; a gateway or DNS server that is not an address is
    /// left out.
    pub fn static_ipv4(&self) -> Option<StaticIpv4> {
        let provisioned = self.setting(settings::PROVISIONED_STATIC_ADDRESS).and_then(host_with_prefix).map(|(address, prefix_len)| {
            let mut dns: Vec<Ipv4Addr> = Vec::new();
            for server in self.setting(settings::PROVISIONED_STATIC_DNS).unwrap_or("").split(',').filter_map(host_ipv4) {
                if !dns.contains(&server) {
                    dns.push(server);
                }
            }
            StaticIpv4 {
                address,
                prefix_len,
                gateway: self.setting(settings::PROVISIONED_STATIC_GATEWAY).and_then(host_ipv4),
                dns,
                domain: None,
                source: SettingSource::Provisioned,
                locked: self.setting_frozen(settings::PROVISIONED_STATIC_ADDRESS),
            }
        });
        let n = self.network();
        let prefix = n.subnet_mask.map(u32::from).filter(|m| m.leading_ones() + m.trailing_zeros() == 32).map(|m| m.leading_ones() as u8);
        let local = match (n.dhcp, n.static_ip, prefix) {
            (false, Some(address), Some(prefix_len)) => Some(StaticIpv4 {
                address,
                prefix_len,
                gateway: n.gateway,
                dns: n.dns,
                domain: n.domain,
                source: SettingSource::Local,
                locked: false,
            }),
            _ => None,
        };
        match (provisioned, local) {
            (Some(p), _) if p.locked => Some(p),
            (_, Some(l)) => Some(l),
            (p, None) => p,
        }
    }

    /// SSH public keys supplied by the provisioning server, in
    /// `authorized_keys` line form.  They are accepted in addition to the
    /// keys added on the phone (`/nvdata/opendeskphone/authorized_keys`) and
    /// disappear when the server stops sending them.  Entries that are not
    /// plain public-key lines are left out.
    pub fn provisioned_ssh_keys(&self) -> Vec<String> {
        let Some(list) = self.root.child(settings::PROVISIONED_SSH_KEYS) else { return Vec::new() };
        let mut keys: Vec<String> = Vec::new();
        for k in list.children_named("key") {
            let line = k.attr("value").unwrap_or("").trim();
            if settings::valid_public_key_line(line) && !keys.iter().any(|x| x == line) {
                keys.push(line.to_owned());
            }
        }
        keys
    }

    /// The same configuration with the given Open Desk Phone settings (for
    /// tests and tools).
    pub fn with_opendeskphone_settings(mut self, settings: OpenDeskPhoneSettings) -> Self {
        self.settings = settings;
        self
    }

    /// The merged `<config>` element.
    pub fn root(&self) -> &Element {
        &self.root
    }

    /// Counter that increases with every merge that changed the result.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// The network profile the merge selected, if any.
    pub fn selected_network(&self) -> Option<&str> {
        self.selected_network.as_deref()
    }

    // ---- generic settings --------------------------------------------------

    fn setting_element(&self, id: &str) -> Option<&Element> {
        self.root.child_with_id("setting", id)
    }

    /// `value` of a top-level `<setting id>`.
    pub fn setting(&self, id: &str) -> Option<&str> {
        self.setting_element(id)?.attr("value")
    }

    /// Any attribute of a top-level setting (for example, `url` or `path`).
    pub fn setting_attr(&self, id: &str, attr: &str) -> Option<&str> {
        self.setting_element(id)?.attr(attr)
    }

    /// True when the setting is frozen and later layers cannot change it.
    pub fn setting_frozen(&self, id: &str) -> bool {
        self.setting_attr(id, "conflict") == Some("freeze")
    }

    pub fn setting_bool(&self, id: &str, default: bool) -> bool {
        self.setting(id).filter(|v| !v.is_empty()).map_or(default, truthy)
    }

    pub fn setting_num<T: std::str::FromStr>(&self, id: &str) -> Option<T> {
        self.setting(id)?.trim().parse().ok()
    }

    fn setting_string(&self, id: &str) -> String {
        self.setting(id).unwrap_or("").to_owned()
    }

    fn setting_opt(&self, id: &str) -> Option<String> {
        self.setting(id).filter(|v| !v.is_empty()).map(str::to_owned)
    }

    /// Every top-level setting as (id, value), in document order.
    pub fn settings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.root
            .children_named("setting")
            .filter_map(|e| Some((e.attr("id")?, e.attr("value").unwrap_or(""))))
    }

    // ---- accounts and codecs -----------------------------------------------

    /// The account limit the factory layer froze.
    pub fn max_accounts(&self) -> usize {
        self.setting_num("max_accounts").unwrap_or(self.model.max_accounts())
    }

    /// Configured accounts, in line order, cut to the model's limit.
    pub fn accounts(&self) -> Vec<Account> {
        let Some(list) = self.root.child("accounts") else { return Vec::new() };
        let mut v: Vec<Account> = list
            .children_named("account")
            .enumerate()
            .map(|(i, e)| account(e, i as u32))
            .collect();
        v.sort_by_key(|a| a.index);
        v.truncate(self.max_accounts());
        v
    }

    /// All codecs, highest priority first.
    pub fn codecs(&self) -> Vec<Codec> {
        let Some(list) = self.root.child("codecs") else { return Vec::new() };
        let mut v: Vec<Codec> = list
            .children_named("codec")
            .map(|e| Codec {
                id: s(e, "id"),
                priority: num(e, "priority").unwrap_or(0),
                enabled: e.attr("enabled").is_some_and(truthy),
                packetization: num(e, "packetization").unwrap_or(20),
                jitter_min: num(e, "jitter_min").unwrap_or(0),
                jitter_max: num(e, "jitter_max").unwrap_or(0),
                jitter_target: num(e, "jitter_target").unwrap_or(0),
                raw: e.clone(),
            })
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.priority));
        v
    }

    /// Enabled codecs, highest priority first.  G.729 is left out unless
    /// `g729_authorized` is set.
    pub fn enabled_codecs(&self) -> Vec<Codec> {
        let g729 = self.setting_bool("g729_authorized", false);
        self.codecs().into_iter().filter(|c| c.enabled && (g729 || c.id != "G729")).collect()
    }

    // ---- subsystem views ---------------------------------------------------

    pub fn network(&self) -> NetworkSettings {
        let ip = |id: &str| self.setting(id).and_then(|v| v.trim().parse::<Ipv4Addr>().ok());
        let lldp = |id: &str| self.setting_num::<i32>(id).filter(|v| (0..64).contains(v)).map(|v| v as u8);
        NetworkSettings {
            dhcp: self.setting_bool("network_enable_dhcp", true),
            static_ip: ip("network_static_ip_address"),
            subnet_mask: ip("network_subnet_mask"),
            gateway: ip("network_default_gateway"),
            dns: ["network_primary_dns_server", "network_secondary_dns_server"]
                .into_iter()
                .filter_map(ip)
                .collect(),
            domain: self.setting_opt("network_domain_name"),
            ipv6_enabled: self.setting_bool("network_ipv6_enabled", false),
            ipv6_preferred: self.setting_bool("network_ipv6_preferred", false),
            ipv6_static: self.setting_opt("network6_static_ip_address"),
            ipv6_mask: self.setting_opt("network6_mask"),
            ipv6_gateway: self.setting_opt("network6_default_gateway"),
            disable_arping: self.setting_bool("network_disable_arping", false),
            vlan_discovery_mode: self.setting_string("network_vlan_discovery_mode"),
            vlan_id: self.setting_num("network_vlan_id"),
            pc_vlan_id: self.setting_num("pc_vlan_id"),
            pc_qos: self.setting_num("pc_qos").unwrap_or(1),
            lan_port_mode: self.setting_string("lan_port_mode"),
            pc_port_mode: self.setting_string("pc_port_mode"),
            sip_dscp: self.setting_num("sip_dscp").unwrap_or(26),
            rtp_dscp: self.setting_num("rtp_dscp").unwrap_or(46),
            sip_qos: self.setting_num("sip_qos").unwrap_or(3),
            rtp_qos: self.setting_num("rtp_qos").unwrap_or(6),
            sip_dscp_lldp: lldp("sip_dscp_with_lldp_override"),
            rtp_dscp_lldp: lldp("rtp_dscp_with_lldp_override"),
            dot1x_passthrough: self.setting_bool("8021x_passthrough", false),
            dot1x_eapol_on_disconnect: self.setting_bool("8021x_eapol_on_disconnect", false),
        }
    }

    /// Provisioned network profiles.
    pub fn networks(&self) -> Vec<NetworkProfile> {
        let Some(list) = self.root.child("networks") else { return Vec::new() };
        list.children_named("network")
            .map(|e| NetworkProfile { id: s(e, "id"), display_name: s(e, "display_name"), cidr: s(e, "cidr") })
            .collect()
    }

    pub fn sip(&self) -> SipSettings {
        SipSettings {
            udp_enabled: self.setting_bool("transport_udp_enabled", true),
            udp_port: self.setting_num("transport_udp_port").unwrap_or(5060),
            tcp_enabled: self.setting_bool("transport_tcp_enabled", true),
            tcp_port: self.setting_num("transport_tcp_port").unwrap_or(5060),
            tls_allowed: self.setting_bool("transport_tls_allowed", true),
            tls_port: self.setting_num("transport_tls_port").unwrap_or(5061),
            tls_allow_wildcard_certs: self.setting_bool("tls_allow_wildcard_certs", true),
            allow_insecure_ssl: self.setting_bool("allow_insecure_ssl", false),
            accept_local_calls: self.setting_opt("accept_local_calls").unwrap_or_else(|| "any".to_owned()),
            default_dial_plan: self.setting_string("default_dial_plan"),
            udp_keepalive_interval: self.setting_num("udp_ka_interval"),
            enable_check_sync: self.setting_bool("enable_check_sync", true),
            send_mac_with_useragent: self.setting_bool("send_mac_with_useragent", false),
            retransmit_until_response: self.setting_bool("retransmit_until_response", false),
            g729_authorized: self.setting_bool("g729_authorized", false),
        }
    }

    pub fn time(&self) -> TimeSettings {
        TimeSettings {
            time_zone: self.setting_opt("time_zone").unwrap_or_else(|| "America/Los_Angeles".to_owned()),
            time_source: self.setting_opt("time_source").unwrap_or_else(|| "ntp".to_owned()),
            ntp_server: self.setting_opt("ntp_server").unwrap_or_else(|| "pool.ntp.org".to_owned()),
            ntp_resync_secs: self.setting_num("ntp_resync").unwrap_or(86_400),
            ntp_prefer_option42: self.setting_bool("ntp_prefer_option42", true),
        }
    }

    pub fn logging(&self) -> LogSettings {
        let server = self.setting_opt("log_server");
        LogSettings {
            level: self.setting_opt("log_level").unwrap_or_else(|| "information".to_owned()),
            remote_enabled: self.setting_bool("enable_logging", false) && server.is_some(),
            server,
            port: self.setting_num("log_port").unwrap_or(514),
        }
    }

    pub fn access(&self) -> AccessSettings {
        AccessSettings {
            web_ui_enabled: self.setting_bool("web_ui_enabled", true),
            ssh_enabled: self.ssh_enabled().value,
            ignore_provisioned_firmware: self.ignore_provisioned_firmware().value,
        }
    }

    // ---- keys, BLF, contacts, ringtones, firmware --------------------------

    /// Softkeys of a keymap state such as `idle` or `connected/transfer`.
    pub fn softkeys(&self, state: &str) -> Vec<Softkey> {
        let Some(st) = self.root.child("keymap").and_then(|k| k.child_with_id("state", state)) else {
            return Vec::new();
        };
        let Some(keys) = st.child("softkeys") else { return Vec::new() };
        let mut v: Vec<Softkey> = keys
            .children_named("key")
            .enumerate()
            .map(|(i, e)| Softkey {
                position: num(e, "id").unwrap_or(i as u32),
                action: s(e, "action"),
                label: opt(e, "label1"),
                args: e.children_named("arg").filter_map(|a| a.text().map(str::to_owned)).collect(),
            })
            .collect();
        v.sort_by_key(|k| k.position);
        v
    }

    /// Names of the keymap states present.
    pub fn keymap_states(&self) -> Vec<&str> {
        self.root
            .child("keymap")
            .map(|k| k.children_named("state").filter_map(|s| s.attr("id")).collect())
            .unwrap_or_default()
    }

    /// Hard-key bindings from the `all` keymap state.
    pub fn key_bindings(&self) -> Vec<KeyBinding> {
        let Some(st) = self.root.child("keymap").and_then(|k| k.child_with_id("state", "all")) else {
            return Vec::new();
        };
        st.children_named("key")
            .map(|e| KeyBinding { key: s(e, "id"), length: opt(e, "length"), action: s(e, "action") })
            .collect()
    }

    /// BLF items carried in the merged configuration.  A local
    /// `/nvdata/blf_items.xml` takes precedence; see
    /// [`crate::store::ConfigStore::blf_items`].
    pub fn blf_items(&self) -> Vec<BlfItem> {
        parse_blf_items(&self.root)
    }

    /// URL of the provisioned BLF items file, if any.
    pub fn blf_items_url(&self) -> Option<&str> {
        self.root.child("smart_blf")?.child("blf_items")?.attr("url").filter(|u| !u.is_empty())
    }

    pub fn contact_sources(&self) -> Vec<ContactSource> {
        self.root
            .children_named("contacts")
            .filter(|e| e.attr("url").is_some())
            .map(|e| ContactSource { id: s(e, "id"), url: s(e, "url"), md5: opt(e, "md5") })
            .collect()
    }

    pub fn ringtones(&self) -> Ringtones {
        let r = self.root.child("ringtones");
        let tones = r
            .and_then(|r| r.child("tones"))
            .map(|t| {
                t.children_named("tone")
                    .map(|e| Tone {
                        id: s(e, "id"),
                        display: opt(e, "display").unwrap_or_else(|| s(e, "id")),
                        kind: opt(e, "type").unwrap_or_else(|| "phone".to_owned()),
                        url: opt(e, "url"),
                        md5: opt(e, "md5"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let alerts = r
            .and_then(|r| r.child("alerts"))
            .map(|a| {
                a.children_named("alert")
                    .map(|e| Alert {
                        alert_info: s(e, "alert_info"),
                        ringtone_id: s(e, "ringtone_id"),
                        ring_type: opt(e, "ring_type").unwrap_or_else(|| "normal".to_owned()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let active = self
            .setting_opt("active_ringtone")
            .or_else(|| self.setting_opt("default_ringtone"))
            .unwrap_or_default();
        Ringtones { tones, alerts, active }
    }

    /// Provisioned firmware entries (all models).
    pub fn firmwares(&self) -> Vec<Firmware> {
        self.firmware_list("firmwares", "firmware")
    }

    pub fn public_firmwares(&self) -> Vec<Firmware> {
        self.firmware_list("public_firmwares", "public_firmware")
    }

    fn firmware_list(&self, list: &str, item: &str) -> Vec<Firmware> {
        self.root
            .child(list)
            .map(|l| {
                l.children_named(item)
                    .map(|e| Firmware { model: s(e, "model"), version: s(e, "version"), url: s(e, "url") })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The provisioned firmware entry for this phone's model.
    pub fn firmware_for_model(&self) -> Option<Firmware> {
        self.firmwares().into_iter().find(|f| f.model.eq_ignore_ascii_case(self.model.name()))
    }

    pub fn multicast_listeners(&self) -> Vec<MulticastListener> {
        self.root
            .child("multicastpage")
            .map(|m| {
                m.children_named("listener")
                    .map(|e| MulticastListener {
                        id: s(e, "id"),
                        name: s(e, "name"),
                        addr: s(e, "addr"),
                        port: num(e, "port").unwrap_or(0),
                        priority: num(e, "priority").unwrap_or(0),
                        interrupt_calls: num(e, "interrupt_calls").unwrap_or(0),
                        extension: s(e, "extension"),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Groups this phone can page.  Entries without an `id`, address, or
    /// port are dropped, and of two with the same `id` the first counts, as
    /// in stock.
    pub fn multicast_broadcasts(&self) -> Vec<MulticastBroadcast> {
        let mut out: Vec<MulticastBroadcast> = Vec::new();
        for e in self.root.child("multicastpage").into_iter().flat_map(|m| m.children_named("broadcast")) {
            let b = MulticastBroadcast {
                id: s(e, "id"),
                name: s(e, "name"),
                addr: s(e, "addr"),
                port: num(e, "port").unwrap_or(0),
                codec: e.attr("codec").filter(|c| !c.is_empty()).unwrap_or("PCMU").to_string(),
            };
            if b.id.is_empty() || b.addr.is_empty() || b.port == 0 || out.iter().any(|o| o.id == b.id) {
                continue;
            }
            out.push(b);
        }
        out
    }

    /// An `<appconfig id>` element, for the app runtime.
    pub fn appconfig(&self, id: &str) -> Option<&Element> {
        self.root.child_with_id("appconfig", id)
    }

    /// Application IDs listed for the application screen.
    pub fn appscreen_applications(&self) -> Vec<&str> {
        self.appconfig("appscreen")
            .map(|a| a.children_named("application").filter_map(|e| e.attr("id")).collect())
            .unwrap_or_default()
    }

    /// Lamp behavior for a line state (`idle`, `hold`, `ringing`, ...).
    pub fn activity_indicator(&self, state: &str) -> Option<Indicator> {
        let e = self.root.child("activity_indicator")?.child_with_id("setting", state)?;
        Some(Indicator { color: s(e, "color"), state: s(e, "state") })
    }

    /// A setting of the active locale (for example, `time_short_format`).
    pub fn locale_setting(&self, id: &str) -> Option<&str> {
        let locale = self.setting("locale").unwrap_or("en_US");
        let locales = self.root.child("locales")?;
        locales
            .child_with_id("locale", locale)
            .or_else(|| locales.child_with_id("locale", "en_US"))?
            .child_with_id("setting", id)?
            .attr("value")
    }

    /// A format of the locale (`time_short_format`, `date_short_format`,
    /// `date_time_format`, or `idle_screen_clock_format`) as the stock
    /// phone application resolves it when it loads the localization: a
    /// top-level `<setting>` (the phone menu writes one), else the active
    /// locale's, else `en_US`'s.  A locale is found by its `id` in any
    /// case.  The defaults are only in `<locales>`.
    pub fn localization_setting(&self, id: &str) -> Option<&str> {
        if let Some(value) = self.setting(id) {
            return Some(value);
        }
        let locales = self.root.child("locales")?;
        let of = |locale: &str| {
            locales
                .children_named("locale")
                .find(|l| l.attr("id").is_some_and(|i| i.eq_ignore_ascii_case(locale)))?
                .child_with_id("setting", id)?
                .attr("value")
        };
        of(self.setting("locale").unwrap_or("en_US")).or_else(|| of("en_US"))
    }
}
