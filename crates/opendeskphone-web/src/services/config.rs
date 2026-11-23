//! `ConfigService`, `ContactsService`, and the stored-configuration parts
//! of `AccountsService` and `NetworkService`.

use super::with_config;
use crate::rpc::ConnectError;
use crate::http::Ctx;
use crate::proto::opendeskphone::settings::v1 as settings_pb;
use crate::{convert, pb, App};
use opendeskphone_config::contacts::parse_contacts;
use opendeskphone_config::view::Transport;
use buffa::Enumeration as _;
use opendeskphone_config::{ConfigStore, Element, IdleFunction, IdleSoftkey, Layer, Node};
use opendeskphone_state::{self as st, Command};
use std::net::Ipv4Addr;
use std::sync::Arc;

fn config_of(app: &App) -> Arc<opendeskphone_config::Config> {
    app.config.lock().unwrap_or_else(|p| p.into_inner()).config()
}

fn flag(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

fn clean(what: &str, s: &str, max: usize) -> Result<(), ConnectError> {
    if s.len() > max {
        return Err(ConnectError::invalid_argument(format!("{what} is too long.")));
    }
    if s.chars().any(char::is_control) {
        return Err(ConnectError::invalid_argument(format!("{what} contains an invalid character.")));
    }
    Ok(())
}

// -------------------------------------------------------------- settings

/// Settings whose values are not sent to the browser.
fn is_secret(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    ["password", "passcode", "secret", "_pin", "psk"].iter().any(|w| id.contains(w))
}

fn valid_setting_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The IDs of the settings a layer holds: those for every network, and
/// those for the network profile in effect.
fn layer_ids(c: &ConfigStore, layer: Layer) -> std::collections::HashSet<String> {
    let cfg = c.config();
    let network = cfg.selected_network();
    let mut ids = std::collections::HashSet::new();
    if let Ok(Some(root)) = c.layer(layer) {
        ids.extend(
            root.children_named("setting")
                .filter(|e| e.attr("network_id").is_none_or(|n| Some(n) == network))
                .filter_map(|e| e.attr("id").map(str::to_owned)),
        );
    }
    ids
}

/// The static address in effect, when it is the provisioning server's.
fn served_static(cfg: &opendeskphone_config::Config) -> Option<opendeskphone_config::view::StaticIpv4> {
    cfg.static_ipv4().filter(|s| s.source == opendeskphone_config::SettingSource::Provisioned)
}

/// The stock settings that say how the phone gets its address.
const ADDRESSING: [&str; 6] = [
    "network_enable_dhcp",
    "network_static_ip_address",
    "network_subnet_mask",
    "network_default_gateway",
    "network_primary_dns_server",
    "network_secondary_dns_server",
];

pub async fn list_settings(ctx: Ctx, req: pb::ListSettingsRequest) -> Result<pb::ListSettingsResponse, ConnectError> {
    with_config(&ctx.app, false, move |c| {
        let cfg = c.config();
        // The provisioning server's settings, and those set on the phone,
        // which replace them unless the server froze its value.
        let mut served = layer_ids(c, Layer::Configurator);
        served.extend(layer_ids(c, Layer::Fetched));
        let local = layer_ids(c, Layer::Ui);
        let mut settings: Vec<pb::Setting> = cfg
            .settings()
            .filter(|(id, _)| id.starts_with(&req.prefix))
            .map(|(id, value)| {
                let secret = is_secret(id);
                pb::Setting {
                    id: id.to_owned(),
                    value: if secret { String::new() } else { value.to_owned() },
                    frozen: cfg.setting_frozen(id),
                    secret,
                    provisioned: served.contains(id) && (cfg.setting_frozen(id) || !local.contains(id)),
                }
            })
            .collect();
        settings.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(pb::ListSettingsResponse { settings, generation: cfg.generation() })
    })
    .await
}

pub async fn set_settings(ctx: Ctx, req: pb::SetSettingsRequest) -> Result<pb::SetSettingsResponse, ConnectError> {
    if req.changes.is_empty() {
        return Err(ConnectError::invalid_argument("no changes"));
    }
    for ch in &req.changes {
        if !valid_setting_id(&ch.id) {
            return Err(ConnectError::invalid_argument(format!("not a setting ID: {:?}", ch.id)));
        }
        if ch.id == "login_password" {
            return Err(ConnectError::invalid_argument("use ChangePassword to change the web password"));
        }
        clean(&ch.id, &ch.value, 4096)?;
    }
    with_config(&ctx.app, true, move |c| {
        let changes: Vec<(&str, &str)> = req.changes.iter().map(|ch| (ch.id.as_str(), ch.value.as_str())).collect();
        let cfg = c.set_many(&changes).map_err(crate::services::config_error)?;
        Ok(pb::SetSettingsResponse { generation: cfg.generation() })
    })
    .await
}

// ---------------------------------------------------------- idle softkeys

/// A softkey as its message (the settings file's `IdleSoftkey`, as this
/// crate generates it, with JSON).
fn pb_softkey(key: &IdleSoftkey) -> settings_pb::IdleSoftkey {
    use settings_pb::idle_softkey::{Action, Function};
    let action = match key {
        IdleSoftkey::Function(f) => Action::Function(Function::from_i32(f.number()).unwrap_or(Function::FUNCTION_UNSPECIFIED).into()),
        IdleSoftkey::App(name) => Action::App(Box::new(settings_pb::App { name: name.clone() })),
    };
    settings_pb::IdleSoftkey { action: Some(action) }
}

/// The softkey a message stands for.
fn softkey_from(key: &settings_pb::IdleSoftkey) -> Result<IdleSoftkey, ConnectError> {
    use settings_pb::idle_softkey::Action;
    match &key.action {
        Some(Action::Function(f)) => IdleFunction::from_number(f.to_i32()).map(IdleSoftkey::Function),
        Some(Action::App(app)) => (app.name.len() <= 64 && opendeskphone_config::settings::valid_app_name(&app.name))
            .then(|| IdleSoftkey::App(app.name.clone())),
        None => None,
    }
    .ok_or_else(|| ConnectError::invalid_argument("not a softkey"))
}

fn function_label(function: IdleFunction) -> &'static str {
    match function {
        IdleFunction::CallLog => "Call Log",
        IdleFunction::Contacts => "Contacts",
        IdleFunction::Status => "Status",
        IdleFunction::Menu => "Menu",
        IdleFunction::Apps => "Apps",
        IdleFunction::Dnd => "DND",
        IdleFunction::Forward => "Forward",
        IdleFunction::Redial => "Redial",
        IdleFunction::Theme => "Theme",
        IdleFunction::Voicemail => "Voicemail",
    }
}

/// The softkeys in effect and every softkey the phone offers: the
/// functions the model has (the theme selector only on a color model),
/// then an entry per app but the contacts and voicemail apps, which the
/// functions of those names stand for.
fn idle_softkeys(app: &App, cfg: &opendeskphone_config::Config) -> pb::IdleSoftkeys {
    let set = cfg.idle_softkeys();
    let choice =
        |key: &IdleSoftkey, label: &str| pb::IdleSoftkeyChoice { softkey: Some(pb_softkey(key)).into(), label: label.to_owned() };
    let functions = IdleFunction::ALL.into_iter().filter(|f| f.on_model(cfg.model()));
    let mut offered: Vec<IdleSoftkey> = functions.clone().map(IdleSoftkey::Function).collect();
    let mut choices: Vec<pb::IdleSoftkeyChoice> =
        functions.map(|f| choice(&IdleSoftkey::Function(f), function_label(f))).collect();
    // The apps the phone has (one that is switched off is not among them).
    let apps = app.store.get::<st::Apps>();
    let app_label = |a: &st::AppInfo| format!("{} (app)", if a.display_name.is_empty() { &a.name } else { &a.display_name });
    for a in apps.installed.iter().filter(|a| !matches!(a.name.as_str(), "contacts" | "voicemail")) {
        let key = IdleSoftkey::App(a.name.clone());
        choices.push(choice(&key, &app_label(a)));
        offered.push(key);
    }
    // A softkey in effect for an app that is not offered keeps its entry:
    // one for the contacts or the voicemail app, labeled as the other apps
    // are, and one for an app the phone does not have (the phone does not
    // show it), labeled with the app's name.
    for key in &set.keys {
        if let IdleSoftkey::App(name) = key
            && !offered.contains(key)
        {
            let label = apps.installed.iter().find(|a| &a.name == name).map(app_label).unwrap_or_else(|| name.clone());
            choices.push(choice(key, &label));
            offered.push(key.clone());
        }
    }
    pb::IdleSoftkeys {
        softkeys: set.keys.iter().map(pb_softkey).collect(),
        source: super::ssh::setting_source(set.source).into(),
        locked: set.locked,
        choices,
    }
}

pub async fn get_idle_softkeys(
    ctx: Ctx,
    _req: pb::GetIdleSoftkeysRequest,
) -> Result<pb::GetIdleSoftkeysResponse, ConnectError> {
    let keys = idle_softkeys(&ctx.app, &config_of(&ctx.app));
    Ok(pb::GetIdleSoftkeysResponse { idle_softkeys: Some(keys).into() })
}

pub async fn set_idle_softkeys(
    ctx: Ctx,
    req: pb::SetIdleSoftkeysRequest,
) -> Result<pb::SetIdleSoftkeysResponse, ConnectError> {
    if req.softkeys.len() > 64 {
        return Err(ConnectError::invalid_argument("too many softkeys"));
    }
    let list = req.softkeys.iter().map(softkey_from).collect::<Result<Vec<IdleSoftkey>, ConnectError>>()?;
    let app = ctx.app.clone();
    let keys = with_config(&ctx.app, true, move |c| {
        if c.config().idle_softkeys().locked {
            return Err(super::ssh::locked());
        }
        let config = c.update_opendeskphone_settings(|s| s.set_idle_softkeys(&list)).map_err(crate::services::config_error)?;
        Ok(idle_softkeys(&app, &config))
    })
    .await?;
    Ok(pb::SetIdleSoftkeysResponse { idle_softkeys: Some(keys).into() })
}

// ------------------------------------------------------------------ theme

/// The theme in effect and every built-in one.  A name the firmware does
/// not have is the default theme, as on the phone's screen.
fn theme(cfg: &opendeskphone_config::Config) -> pb::Theme {
    let set = cfg.theme();
    let (name, picture) = match &set.choice {
        opendeskphone_config::ThemeChoice::Builtin(name) => (name.as_str(), false),
        opendeskphone_config::ThemeChoice::Picture => (set.builtin.as_str(), true),
    };
    let id = st::THEMES.iter().find(|w| w.id == name).map_or(st::DEFAULT_THEME, |w| w.id);
    pb::Theme {
        id: id.to_owned(),
        source: super::ssh::setting_source(set.source).into(),
        locked: set.locked,
        choices: st::THEMES
            .iter()
            .map(|w| pb::ThemeChoice { id: w.id.to_owned(), name: w.name.to_owned(), family: w.family.to_owned() })
            .collect(),
        picture,
    }
}

pub async fn get_theme(ctx: Ctx, _req: pb::GetThemeRequest) -> Result<pb::GetThemeResponse, ConnectError> {
    Ok(pb::GetThemeResponse { theme: Some(theme(&config_of(&ctx.app))).into() })
}

pub async fn set_theme(ctx: Ctx, req: pb::SetThemeRequest) -> Result<pb::SetThemeResponse, ConnectError> {
    let id = req.id;
    if !id.is_empty() && !st::THEMES.iter().any(|w| w.id == id) {
        return Err(ConnectError::invalid_argument("not a theme"));
    }
    let set = with_config(&ctx.app, true, move |c| {
        if c.config().theme().locked {
            return Err(super::ssh::locked());
        }
        let config =
            c.update_opendeskphone_settings(|s| s.theme = (!id.is_empty()).then_some(id)).map_err(crate::services::config_error)?;
        Ok(theme(&config))
    })
    .await?;
    Ok(pb::SetThemeResponse { theme: Some(set).into() })
}

// --------------------------------------------------------------- network

/// The stock setting `network_vlan_discovery_mode`, read as the network
/// subsystem reads it: anything but MANUAL and LLDP is no VLAN.
fn vlan_discovery_mode(stored: &str) -> pb::VlanDiscoveryMode {
    match stored.to_ascii_uppercase().as_str() {
        "MANUAL" => pb::VlanDiscoveryMode::Manual,
        "LLDP" => pb::VlanDiscoveryMode::Lldp,
        _ => pb::VlanDiscoveryMode::None,
    }
}

pub(crate) fn network_settings(app: &App) -> pb::NetworkSettings {
    let cfg = config_of(app);
    let n = cfg.network();
    let t = cfg.time();
    let ip = |a: Option<Ipv4Addr>| a.map(|a| a.to_string()).unwrap_or_default();
    // The server's static address, when in effect, is what the form shows.
    let served = served_static(&cfg);
    let (dhcp, static_ip, subnet_mask, gateway, dns) = match &served {
        Some(s) => {
            let mask = Ipv4Addr::from(u32::MAX.checked_shl(32 - u32::from(s.prefix_len)).unwrap_or(0));
            (false, Some(s.address), Some(mask), s.gateway, s.dns.clone())
        }
        None => (n.dhcp, n.static_ip, n.subnet_mask, n.gateway, n.dns.clone()),
    };
    pb::NetworkSettings {
        addressing_provisioned: served.is_some(),
        dhcp,
        static_ip: ip(static_ip),
        subnet_mask: ip(subnet_mask),
        gateway: ip(gateway),
        primary_dns: ip(dns.first().copied()),
        secondary_dns: ip(dns.get(1).copied()),
        domain: n.domain.unwrap_or_default(),
        vlan_discovery_mode: vlan_discovery_mode(&n.vlan_discovery_mode).into(),
        vlan_id: n.vlan_id.map(u32::from),
        pc_vlan_id: n.pc_vlan_id.map(u32::from),
        pc_qos: n.pc_qos as u32,
        pc_port_mode: n.pc_port_mode,
        lan_port_mode: n.lan_port_mode,
        sip_dscp: n.sip_dscp as u32,
        rtp_dscp: n.rtp_dscp as u32,
        sip_qos: n.sip_qos as u32,
        rtp_qos: n.rtp_qos as u32,
        dot1x_passthrough: n.dot1x_passthrough,
        ntp_server: t.ntp_server,
        time_zone: t.time_zone,
        ipv6_enabled: n.ipv6_enabled,
    }
}

pub async fn set_network_settings(
    ctx: Ctx,
    req: pb::SetNetworkSettingsRequest,
) -> Result<pb::SetNetworkSettingsResponse, ConnectError> {
    let s = req.settings.ok_or_else(|| ConnectError::invalid_argument("no settings"))?;
    let ip = |what: &str, v: &str| -> Result<(), ConnectError> {
        if v.is_empty() || v.parse::<Ipv4Addr>().is_ok() {
            Ok(())
        } else {
            Err(ConnectError::invalid_argument(format!("{what} is not a valid IPv4 address.")))
        }
    };
    ip("The address", &s.static_ip)?;
    ip("The subnet mask", &s.subnet_mask)?;
    ip("The gateway", &s.gateway)?;
    ip("The primary DNS server", &s.primary_dns)?;
    ip("The secondary DNS server", &s.secondary_dns)?;
    if !s.dhcp {
        if s.static_ip.is_empty() || s.subnet_mask.is_empty() {
            return Err(ConnectError::invalid_argument("A static IP address and subnet mask are required."));
        }
        let mask = u32::from(s.subnet_mask.parse::<Ipv4Addr>().expect("checked"));
        if mask == 0 || mask.leading_ones() + mask.trailing_zeros() != 32 {
            return Err(ConnectError::invalid_argument("Invalid subnet mask."));
        }
    }
    let vlan = |what: &str, v: Option<u32>| match v {
        Some(id) if !(1..=4094).contains(&id) => Err(ConnectError::invalid_argument(format!("{what} must be 1 to 4094."))),
        _ => Ok(()),
    };
    vlan("The VLAN ID", s.vlan_id)?;
    vlan("The PC port VLAN ID", s.pc_vlan_id)?;
    if s.sip_dscp > 63 || s.rtp_dscp > 63 {
        return Err(ConnectError::invalid_argument("DSCP must be 0 to 63."));
    }
    if s.sip_qos > 7 || s.rtp_qos > 7 || s.pc_qos > 7 {
        return Err(ConnectError::invalid_argument("802.1p priority must be 0 to 7."));
    }
    // The stock setting's values.
    let mode = match s.vlan_discovery_mode.as_known() {
        Some(pb::VlanDiscoveryMode::None) => "NONE",
        Some(pb::VlanDiscoveryMode::Manual) => "MANUAL",
        Some(pb::VlanDiscoveryMode::Lldp) => "LLDP",
        Some(pb::VlanDiscoveryMode::Unspecified) | None => {
            return Err(ConnectError::invalid_argument("no VLAN discovery mode"));
        }
    };
    if mode == "MANUAL" && s.vlan_id.is_none() {
        return Err(ConnectError::invalid_argument("Enter a voice VLAN ID."));
    }
    for (what, v) in [
        ("The domain", &s.domain),
        ("The NTP server", &s.ntp_server),
        ("The time zone", &s.time_zone),
        ("The PC port mode", &s.pc_port_mode),
        ("The LAN port mode", &s.lan_port_mode),
    ] {
        clean(what, v, 255)?;
    }
    let num = |v: Option<u32>| v.map(|v| v.to_string()).unwrap_or_default();
    let mut changes: Vec<(&'static str, String)> = vec![
        ("network_enable_dhcp", flag(s.dhcp).into()),
        ("network_static_ip_address", s.static_ip.clone()),
        ("network_subnet_mask", s.subnet_mask.clone()),
        ("network_default_gateway", s.gateway.clone()),
        ("network_primary_dns_server", s.primary_dns.clone()),
        ("network_secondary_dns_server", s.secondary_dns.clone()),
        ("network_domain_name", s.domain.clone()),
        ("network_vlan_discovery_mode", mode.to_owned()),
        ("network_vlan_id", num(s.vlan_id)),
        ("pc_vlan_id", num(s.pc_vlan_id)),
        ("pc_qos", s.pc_qos.to_string()),
        ("sip_dscp", s.sip_dscp.to_string()),
        ("rtp_dscp", s.rtp_dscp.to_string()),
        ("sip_qos", s.sip_qos.to_string()),
        ("rtp_qos", s.rtp_qos.to_string()),
        ("8021x_passthrough", flag(s.dot1x_passthrough).into()),
        ("network_ipv6_enabled", flag(s.ipv6_enabled).into()),
    ];
    // Left alone when the form sends them empty.
    for (id, v) in [
        ("pc_port_mode", &s.pc_port_mode),
        ("lan_port_mode", &s.lan_port_mode),
        ("ntp_server", &s.ntp_server),
        ("time_zone", &s.time_zone),
    ] {
        if !v.is_empty() {
            changes.push((id, v.clone()));
        }
    }
    with_config(&ctx.app, true, move |c| {
        let cfg = c.config();
        // Write only what differs, so untouched settings stay provisioned
        // (and frozen ones do not fail the whole save).
        // While the server's static address is in effect the form shows
        // it, not the phone's own address settings, which stay as they are.
        let served = served_static(&cfg).is_some();
        let diff: Vec<(&str, &str)> = changes
            .iter()
            .filter(|(id, _)| !(served && ADDRESSING.contains(id)))
            .filter(|(id, v)| cfg.setting(id).unwrap_or("") != v.as_str())
            .map(|(id, v)| (*id, v.as_str()))
            .collect();
        if !diff.is_empty() {
            c.set_many(&diff).map_err(crate::services::config_error)?;
        }
        Ok(())
    })
    .await?;
    if req.apply {
        // Not awaited: the address may change under this very connection.
        ctx.app.store.post(Command::ReconfigureNetwork);
    }
    Ok(pb::SetNetworkSettingsResponse {})
}

// -------------------------------------------------------------- accounts

fn pb_transport(t: Transport) -> buffa::EnumValue<pb::SipTransport> {
    (match t {
        Transport::Udp => pb::SipTransport::Udp,
        Transport::Tcp => pb::SipTransport::Tcp,
        Transport::Tls => pb::SipTransport::Tls,
    })
    .into()
}

fn transport_name(t: buffa::EnumValue<pb::SipTransport>) -> &'static str {
    match t.as_known().unwrap_or(pb::SipTransport::Udp) {
        pb::SipTransport::Tcp => "tcp",
        pb::SipTransport::Tls => "tls",
        _ => "udp",
    }
}

fn account_config(a: &opendeskphone_config::view::Account) -> pb::AccountConfig {
    let primary = a.primary.clone().unwrap_or_default();
    let alt = a.alternate.clone();
    pb::AccountConfig {
        index: a.index,
        account_id: a.account_id.clone(),
        enabled: a.enabled,
        register: a.register,
        username: a.username.clone(),
        authname: a.authname.clone(),
        password: String::new(),
        has_password: !a.password.is_empty() || primary.password.is_some(),
        display_name: if a.caller_id.is_empty() { a.display.clone() } else { a.caller_id.clone() },
        line_label: a.line_label.clone(),
        server: primary.server.clone(),
        port: primary.port as u32,
        transport: pb_transport(primary.transport),
        alternate_server: alt.as_ref().map(|h| h.server.clone()).unwrap_or_default(),
        alternate_port: alt.as_ref().map(|h| h.port as u32).unwrap_or(0),
        alternate_transport: alt.as_ref().map(|h| pb_transport(h.transport)).unwrap_or_default(),
        outbound_proxy: a.outbound_proxy.clone().unwrap_or_default(),
        outbound_port: a.outbound_port.map(u32::from).unwrap_or(0),
        voicemail: a.voicemail.clone().unwrap_or_default(),
        srtp: primary.media_encryption.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("sdes")),
        reregister_secs: primary.reregister,
    }
}

pub async fn list_accounts(ctx: Ctx, _req: pb::ListAccountsRequest) -> Result<pb::ListAccountsResponse, ConnectError> {
    let status = convert::accounts(&ctx.app.store.get::<st::Accounts>());
    with_config(&ctx.app, false, move |c| {
        let cfg = c.config();
        let local = matches!(c.layer(Layer::Ui), Ok(Some(root)) if root.child("accounts").is_some());
        Ok(pb::ListAccountsResponse {
            accounts: cfg.accounts().iter().map(account_config).collect(),
            status,
            max_accounts: cfg.max_accounts() as u32,
            local,
        })
    })
    .await
}

fn set_or_remove(e: &mut Element, name: &str, value: &str) {
    if value.is_empty() { e.remove_attr(name) } else { e.set_attr(name, value) }
}

fn port(what: &str, p: u32) -> Result<u32, ConnectError> {
    if p > 65535 { Err(ConnectError::invalid_argument(format!("{what} is not a valid port."))) } else { Ok(p) }
}

/// Builds the stock `<account>` element for an edited account, starting
/// from the stored element so attributes this form does not model survive.
fn account_element(a: &pb::AccountConfig, existing: Option<&opendeskphone_config::view::Account>) -> Result<Element, ConnectError> {
    for (what, v) in [
        ("The user name", &a.username),
        ("The authentication name", &a.authname),
        ("The password", &a.password),
        ("The display name", &a.display_name),
        ("The line label", &a.line_label),
        ("The server", &a.server),
        ("The alternate server", &a.alternate_server),
        ("The outbound proxy", &a.outbound_proxy),
        ("The voicemail number", &a.voicemail),
        ("The account ID", &a.account_id),
    ] {
        clean(what, v, 255)?;
    }
    if a.username.is_empty() {
        return Err(ConnectError::invalid_argument(format!("Line {}: a username is required.", a.index + 1)));
    }
    if a.server.is_empty() {
        return Err(ConnectError::invalid_argument(format!("Line {}: a server is required.", a.index + 1)));
    }
    let mut e = existing.map(|x| x.raw.clone()).unwrap_or_else(|| Element::new("account"));
    e.set_attr("index", &a.index.to_string());
    e.set_attr("status", flag(a.enabled));
    e.set_attr("register", flag(a.register));
    e.set_attr("account_id", if a.account_id.is_empty() { &a.username } else { &a.account_id });
    e.set_attr("username", &a.username);
    set_or_remove(&mut e, "authname", if a.authname == a.username { "" } else { &a.authname });
    if !a.password.is_empty() {
        e.set_attr("password", &a.password);
    } else if existing.is_none() {
        e.set_attr("password", "");
    }
    set_or_remove(&mut e, "caller_id", &a.display_name);
    set_or_remove(&mut e, "display", &a.display_name);
    set_or_remove(&mut e, "line_label", &a.line_label);
    set_or_remove(&mut e, "voicemail", &a.voicemail);
    set_or_remove(&mut e, "outbound_proxy", &a.outbound_proxy);
    let out_port = port("The outbound proxy port", a.outbound_port)?;
    set_or_remove(&mut e, "outbound_port", &if out_port == 0 { String::new() } else { out_port.to_string() });
    // The host records carry these; drop account-level copies.
    for name in ["server", "port", "transport"] {
        e.remove_attr(name);
    }
    let reregister = if a.reregister_secs == 0 { 300 } else { a.reregister_secs.clamp(30, 86_400) };
    {
        let tls = transport_name(a.transport) == "tls";
        let p = match port("The server port", a.port)? {
            0 if tls => 5061,
            0 => 5060,
            p => p,
        };
        let h = e.child_or_insert("host_primary");
        h.set_attr("server", &a.server);
        h.set_attr("port", &p.to_string());
        h.set_attr("transport", transport_name(a.transport));
        h.set_attr("reregister", &reregister.to_string());
        if a.srtp { h.set_attr("media_encryption", "sdes") } else { h.remove_attr("media_encryption") }
    }
    if a.alternate_server.is_empty() {
        e.remove_children(|c| c.name == "host_alternate");
    } else {
        let tls = transport_name(a.alternate_transport) == "tls";
        let p = match port("The backup server port", a.alternate_port)? {
            0 if tls => 5061,
            0 => 5060,
            p => p,
        };
        let h = e.child_or_insert("host_alternate");
        h.set_attr("server", &a.alternate_server);
        h.set_attr("port", &p.to_string());
        h.set_attr("transport", transport_name(a.alternate_transport));
        if h.attr("reregister").is_none() {
            h.set_attr("reregister", &reregister.to_string());
        }
        if a.srtp { h.set_attr("media_encryption", "sdes") } else { h.remove_attr("media_encryption") }
    }
    Ok(e)
}

pub async fn set_accounts(ctx: Ctx, req: pb::SetAccountsRequest) -> Result<pb::SetAccountsResponse, ConnectError> {
    with_config(&ctx.app, true, move |c| {
        let cfg = c.config();
        let existing = cfg.accounts();
        let max = cfg.max_accounts() as u32;
        let mut seen = std::collections::HashSet::new();
        let mut elements = Vec::new();
        let mut accounts = req.accounts.clone();
        accounts.sort_by_key(|a| a.index);
        for a in &accounts {
            if a.index >= max {
                return Err(ConnectError::invalid_argument(format!("This model supports {max} accounts.")));
            }
            if !seen.insert(a.index) {
                return Err(ConnectError::invalid_argument(format!("account {} is given twice", a.index + 1)));
            }
            elements.push(account_element(a, existing.iter().find(|e| e.index == a.index))?);
        }
        c.set_local_accounts(&elements).map_err(crate::services::config_error)?;
        Ok(pb::SetAccountsResponse {})
    })
    .await
}

pub async fn clear_local_accounts(
    ctx: Ctx,
    _req: pb::ClearLocalAccountsRequest,
) -> Result<pb::ClearLocalAccountsResponse, ConnectError> {
    with_config(&ctx.app, true, |c| {
        c.clear_local_accounts().map_err(crate::services::config_error)?;
        Ok(pb::ClearLocalAccountsResponse {})
    })
    .await
}

// -------------------------------------------------------------- contacts

fn list_name(name: &str) -> Result<(), ConnectError> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && !name.ends_with(".tmp")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok { Ok(()) } else { Err(ConnectError::invalid_argument("List names may contain only letters, digits, '-', '_', and '.'.")) }
}

fn read_list(c: &ConfigStore, name: &str) -> Result<String, ConnectError> {
    let data = c
        .nvdata()
        .contact_list(name)
        .map_err(|e| ConnectError::internal(e.to_string()))?
        .ok_or_else(|| ConnectError::not_found(format!("no contact list named {name}")))?;
    String::from_utf8(data).map_err(|_| ConnectError::new(crate::rpc::Code::DataLoss, "the list is not text"))
}

pub async fn list_contact_lists(
    ctx: Ctx,
    _req: pb::ListContactListsRequest,
) -> Result<pb::ListContactListsResponse, ConnectError> {
    with_config(&ctx.app, false, |c| {
        let mut names = c.nvdata().contact_list_names().unwrap_or_default();
        names.retain(|n| list_name(n).is_ok());
        names.sort();
        // A list the server supplies is stored under its ID, spelled as
        // stock spells an ID in a file name.
        let served: Vec<String> = c
            .config()
            .contact_sources()
            .iter()
            .map(|s| s.id.replace('%', "%25").replace('.', "%2E").replace('/', "%2F"))
            .collect();
        let lists = names
            .into_iter()
            .map(|name| match read_list(c, &name).and_then(|t| parse_contacts(&t).map_err(|e| ConnectError::invalid_argument(e.to_string()))) {
                Ok(groups) => pb::ContactListInfo {
                    provisioned: served.contains(&name),
                    name,
                    groups: groups.len() as u32,
                    contacts: groups.iter().map(|g| g.contacts.len() as u32).sum(),
                    error: String::new(),
                },
                Err(e) => pb::ContactListInfo {
                    provisioned: served.contains(&name),
                    name,
                    groups: 0,
                    contacts: 0,
                    error: e.message.unwrap_or_default(),
                },
            })
            .collect();
        Ok(pb::ListContactListsResponse { lists })
    })
    .await
}

pub async fn get_contact_list(
    ctx: Ctx,
    req: pb::GetContactListRequest,
) -> Result<pb::GetContactListResponse, ConnectError> {
    list_name(&req.name)?;
    with_config(&ctx.app, false, move |c| {
        let text = read_list(c, &req.name)?;
        let groups = parse_contacts(&text).map_err(|e| ConnectError::new(crate::rpc::Code::DataLoss, e.to_string()))?;
        Ok(pb::GetContactListResponse {
            groups: groups
                .into_iter()
                .map(|g| pb::ContactGroup {
                    id: g.id,
                    name: g.group_name,
                    contacts: g
                        .contacts
                        .into_iter()
                        .map(|k| pb::Contact {
                            id: k.id,
                            first_name: k.first_name,
                            last_name: k.last_name,
                            organization: k.organization,
                            contact_type: k.contact_type,
                            subscribe_to: k.subscribe_to.unwrap_or_default(),
                            numbers: k
                                .numbers
                                .into_iter()
                                .map(|n| pb::ContactNumber { dial: n.dial, label: n.label, primary: n.primary })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
    })
    .await
}

/// Serializes groups in the stock shape, reusing stored elements so that
/// attributes this form does not model survive an edit.
fn contacts_document(groups: &[pb::ContactGroup], old: &[opendeskphone_config::contacts::ContactGroup]) -> Result<Element, ConnectError> {
    let mut root = Element::new("phonebooks");
    let mut used_ids = std::collections::HashSet::new();
    for (gi, g) in groups.iter().enumerate() {
        clean("A group name", &g.name, 255)?;
        clean("A group ID", &g.id, 255)?;
        let mut ge = Element::new("contacts");
        ge.set_attr("group_name", &g.name);
        ge.set_attr("editable", "1");
        ge.set_attr("id", if g.id.is_empty() { "local" } else { &g.id });
        let old_group = old.iter().find(|o| o.id == g.id).or(old.get(gi));
        for k in &g.contacts {
            for (what, v) in [
                ("A first name", &k.first_name),
                ("A last name", &k.last_name),
                ("An organization", &k.organization),
                ("A contact ID", &k.id),
                ("A contact type", &k.contact_type),
                ("A presence extension", &k.subscribe_to),
            ] {
                clean(what, v, 255)?;
            }
            if k.first_name.is_empty() && k.last_name.is_empty() && k.organization.is_empty() {
                return Err(ConnectError::invalid_argument("Enter a name or organization."));
            }
            let old_contact =
                old_group.and_then(|og| og.contacts.iter().find(|oc| !k.id.is_empty() && oc.id == k.id));
            let mut ce = old_contact.map(|oc| oc.raw.clone()).unwrap_or_else(|| Element::new("contact"));
            let mut id = k.id.clone();
            if id.is_empty() || !used_ids.insert(id.clone()) {
                // New contact (or a duplicated ID): give it a fresh one.
                id = format!("web-{}", &crate::auth::random_token()[..12]);
                used_ids.insert(id.clone());
            }
            ce.set_attr("id", &id);
            ce.set_attr("first_name", &k.first_name);
            ce.set_attr("last_name", &k.last_name);
            ce.set_attr("organization", &k.organization);
            ce.set_attr("contact_type", if k.contact_type.is_empty() { "sip" } else { &k.contact_type });
            set_or_remove(&mut ce, "subscribe_to", &k.subscribe_to);
            ce.remove_children(|c| c.name == "numbers");
            let mut numbers = Element::new("numbers");
            let primary_index = k.numbers.iter().position(|n| n.primary).unwrap_or(0);
            for (ni, n) in k.numbers.iter().enumerate() {
                clean("A number", &n.dial, 255)?;
                clean("A number label", &n.label, 255)?;
                if n.dial.is_empty() {
                    continue;
                }
                let mut ne = old_contact
                    .and_then(|oc| oc.numbers.iter().find(|on| on.dial == n.dial))
                    .map(|on| on.raw.clone())
                    .unwrap_or_else(|| Element::new("number"));
                ne.set_attr("dial", &n.dial);
                ne.set_attr("label", &n.label);
                if ni == primary_index { ne.set_attr("primary", "1") } else { ne.remove_attr("primary") }
                numbers.children.push(Node::Element(ne));
            }
            ce.children.push(Node::Element(numbers));
            ge.children.push(Node::Element(ce));
        }
        root.children.push(Node::Element(ge));
    }
    Ok(root)
}

pub async fn put_contact_list(
    ctx: Ctx,
    req: pb::PutContactListRequest,
) -> Result<pb::PutContactListResponse, ConnectError> {
    list_name(&req.name)?;
    let app = ctx.app.clone();
    with_config(&ctx.app, false, move |c| {
        let old = read_list(c, &req.name).ok().and_then(|t| parse_contacts(&t).ok()).unwrap_or_default();
        let doc = contacts_document(&req.groups, &old)?;
        c.nvdata()
            .write_contact_list(&req.name, doc.to_document().as_bytes())
            .map_err(|e| ConnectError::internal(e.to_string()))?;
        Ok(())
    })
    .await?;
    app.store.post(Command::ReloadContacts);
    Ok(pb::PutContactListResponse {})
}

pub async fn delete_contact_list(
    ctx: Ctx,
    req: pb::DeleteContactListRequest,
) -> Result<pb::DeleteContactListResponse, ConnectError> {
    list_name(&req.name)?;
    let app = ctx.app.clone();
    with_config(&ctx.app, false, move |c| {
        read_list(c, &req.name)?;
        c.nvdata()
            .remove(&format!("contact_lists/{}", req.name))
            .map_err(|e| ConnectError::internal(e.to_string()))
    })
    .await?;
    app.store.post(Command::ReloadContacts);
    Ok(pb::DeleteContactListResponse {})
}

pub async fn export_contact_list(
    ctx: Ctx,
    req: pb::ExportContactListRequest,
) -> Result<pb::ExportContactListResponse, ConnectError> {
    list_name(&req.name)?;
    with_config(&ctx.app, false, move |c| Ok(pb::ExportContactListResponse { xml: read_list(c, &req.name)? })).await
}

pub async fn import_contact_list(
    ctx: Ctx,
    req: pb::ImportContactListRequest,
) -> Result<pb::ImportContactListResponse, ConnectError> {
    list_name(&req.name)?;
    let groups = parse_contacts(&req.xml).map_err(|e| ConnectError::invalid_argument(format!("Invalid contact list: {e}")))?;
    if groups.is_empty() {
        return Err(ConnectError::invalid_argument("No contacts found in the file."));
    }
    let contacts = groups.iter().map(|g| g.contacts.len() as u32).sum();
    let app = ctx.app.clone();
    with_config(&ctx.app, false, move |c| {
        c.nvdata().write_contact_list(&req.name, req.xml.as_bytes()).map_err(|e| ConnectError::internal(e.to_string()))
    })
    .await?;
    app.store.post(Command::ReloadContacts);
    Ok(pb::ImportContactListResponse { contacts })
}
