//! opendeskphone-net: the network manager on the main role's runtime.
//!
//! The network is the way into the phone, so this module is conservative:
//!
//! - The PC jack is bridged to the LAN jack as the stock settings say
//!   (`pc_port_mode`, `pc_vlan_id`, `pc_qos`, `8021x_passthrough`), as on
//!   stock.  `/nvdata/opendeskphone/net-no-bridge` holds it off whatever the settings
//!   say (`dphone net bridge off --save` writes the file, `on --save`
//!   removes it); `dphone net bridge on|off` alone changes the running
//!   arrangement only.
//! - Whatever configuration is in force, if the phone has no address
//!   [`FALLBACK_SECS`] after it was applied (or after the address was
//!   lost), the manager is switched to the plainest configuration
//!   (standalone `lan`, DHCP, no VLAN, PC jack down).  Once that one has an
//!   address (the network answers again: a cable back in, a DHCP server
//!   back up), the settings are tried once more.  If they go without an
//!   address for the same time again, the plain configuration stays until
//!   told otherwise (`dphone net configured`, a settings change, a restart
//!   of the main role, or a reboot), so settings that are themselves at
//!   fault cannot loop.
//! - A static address from the provisioning server (`opendeskphone_static_address`)
//!   is an address at once, wrong or not, so for it "has an address" is not
//!   enough: it counts once the provisioning server has answered the phone
//!   at that address (the DPMA session is attached again after the address
//!   came up).  Until then the same time runs, and the same fallback
//!   follows, with two differences that the server's part in it calls for.
//!   The second try waits until the server answers the plain arrangement,
//!   so a server that was down (a power cut that restarts the phones and
//!   the PBX together) is not taken for a bad address; and where the plain
//!   arrangement gets no address either (a network without DHCP), the
//!   static address is tried again after the same time, in turns, since
//!   nothing is lost by it.  A static address set on the phone itself is
//!   applied as stock applies it, without any of this.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use opendeskphone_net::{
    AddressSource, Ipv4Config, Ipv6Config, MacAddr, NetConfig, NetManager, NetState, PcPort, Phase, PortMode,
    VlanDiscovery,
};
use opendeskphone_config::SettingSource;
use opendeskphone_state::{HealthStatus, Subsystem, Topic};
use log::{error, info, warn};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::SharedConfig;
use crate::diag::Output;
use crate::mainrole::Context;
use crate::model::Family;

/// Seconds without an address before falling back to the plain configuration.
pub const FALLBACK_SECS: u64 = 150;

/// Flag file: keep the PC jack off (not bridged), whatever the settings say.
pub fn bridge_hold_flag(ctx: &Context) -> std::path::PathBuf {
    ctx.paths.own_dir().join("net-no-bridge")
}

/// Where the automatic fallback stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fallback {
    /// The arrangement in force was applied on purpose (or the settings
    /// have had an address since).
    None,
    /// Plain, after the settings got no address.  They are tried once more
    /// when the plain arrangement has an address.
    Waiting,
    /// The settings, tried again after [`Fallback::Waiting`].
    Retrying,
    /// Plain, after the second try failed as well: stays until told
    /// otherwise.
    Final,
}

enum Cmd {
    /// The merged configuration changed.
    Changed,
    Show(oneshot::Sender<String>),
    Bridge { on: bool, save: bool, reply: oneshot::Sender<String> },
    Plain(oneshot::Sender<String>),
    Configured(oneshot::Sender<String>),
    SyncTime,
    Browse(Duration, oneshot::Sender<Vec<opendeskphone_net::mdns::Service>>),
}

/// Handle for the rest of the main role.
#[derive(Clone)]
pub struct NetHandle {
    tx: mpsc::Sender<Cmd>,
    resolver: Arc<std::sync::Mutex<Option<opendeskphone_net::Resolver>>>,
    qos: Arc<std::sync::Mutex<opendeskphone_net::QosMarks>>,
}

impl NetHandle {
    /// The DSCP and 802.1p values for SIP and RTP sockets in force now
    /// (configuration, overridden by the switch's LLDP-MED policy).  Apply
    /// them with `opendeskphone_net::qos::mark_socket`.
    pub fn qos(&self) -> opendeskphone_net::QosMarks {
        *self.qos.lock().unwrap()
    }

    /// Re-apply the network configuration from the current settings.
    pub fn reconfigure(&self) {
        let (reply, _) = oneshot::channel();
        let _ = self.tx.try_send(Cmd::Configured(reply));
    }

    /// The merged configuration changed: re-run bring-up only if the
    /// effective network configuration differs from the one in use.
    pub fn config_changed(&self) {
        let _ = self.tx.try_send(Cmd::Changed);
    }

    pub fn sync_time(&self) {
        let _ = self.tx.try_send(Cmd::SyncTime);
    }

    /// The resolver that follows the network's DNS servers; `None` until the
    /// manager runs (and always in the simulator).
    pub fn resolver(&self) -> Option<opendeskphone_net::Resolver> {
        self.resolver.lock().unwrap().clone()
    }

    /// Look for DPMA servers announced over mDNS (`_digiumproxy._udp`).
    pub async fn browse_proxies(&self, window: Duration) -> Vec<opendeskphone_net::mdns::Service> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Browse(window, reply)).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
}

pub fn start(ctx: &Context, config: Option<SharedConfig>, hub: &super::system::ConfigHub) -> NetHandle {
    start_with_fallback(ctx, config, hub, Duration::from_secs(FALLBACK_SECS))
}

/// [`start`] with the time without an address before the fallback given
/// (host tests use a short one).
pub fn start_with_fallback(
    ctx: &Context,
    config: Option<SharedConfig>,
    hub: &super::system::ConfigHub,
    fallback: Duration,
) -> NetHandle {
    ctx.store.set_health(Subsystem::Net, HealthStatus::Starting);
    let mac = read_mac(ctx, config.as_ref());
    if let Some(m) = mac {
        info!("net: MAC {}", m.bare_hex());
    }
    ctx.store.update::<opendeskphone_state::System, _>(|s| s.mac = mac.map(|m| m.0));

    let (tx, rx) = mpsc::channel::<Cmd>(8);
    register_diag(ctx, tx.clone());
    let handle = NetHandle {
        tx,
        resolver: Arc::new(std::sync::Mutex::new(None)),
        qos: Arc::new(std::sync::Mutex::new(opendeskphone_net::QosMarks::default())),
    };

    if ctx.sim {
        info!("net: simulator; the host's network is left alone");
        ctx.store.set_health(Subsystem::Net, HealthStatus::Stopped);
        tokio::spawn(sim_task(rx));
        return handle;
    }
    ctx.health.require("net");
    tokio::spawn(run(ctx.clone(), config, hub.clone(), mac, rx, handle.clone(), fallback));
    handle
}

async fn sim_task(mut rx: mpsc::Receiver<Cmd>) {
    while let Some(cmd) = rx.recv().await {
        let text = "network not managed in the simulator\n".to_string();
        match cmd {
            Cmd::Show(r) | Cmd::Plain(r) | Cmd::Configured(r) => drop(r.send(text)),
            Cmd::Bridge { reply, .. } => drop(reply.send(text)),
            Cmd::SyncTime | Cmd::Changed => {}
            Cmd::Browse(_, reply) => drop(reply.send(Vec::new())),
        }
    }
}

/// The D40 thru D70 store their MAC address in nvdata's `etc/enet.cfg`,
/// and their kernels assign a random one.  On other models, the kernel
/// assigns the correct one.
fn read_mac(ctx: &Context, config: Option<&SharedConfig>) -> Option<MacAddr> {
    let mac = match ctx.model.family() {
        Family::Old | Family::New => {
            let mac = enet_cfg_mac(ctx, config);
            if mac.is_none() {
                warn!("net: no ENETCFG_MAC in enet.cfg; keeping the kernel's random MAC");
            }
            mac
        }
        // The simulator's interfaces are the host's.
        _ if ctx.sim => None,
        _ => {
            let ifaces = plain_config(ctx, None).ifaces;
            let name = ifaces.conduit.unwrap_or(ifaces.lan);
            let mac = kernel_mac(&name);
            if mac.is_none() {
                warn!("net: {name} has no MAC address");
            }
            mac
        }
    };
    // The simulator needs an identity too (DPMA is keyed by MAC).
    mac.or(ctx.sim.then_some(MacAddr([0x00, 0x0f, 0xd3, 0x00, 0x00, 0x01])))
}

fn enet_cfg_mac(ctx: &Context, config: Option<&SharedConfig>) -> Option<MacAddr> {
    if let Some(mac) = config
        .and_then(|c| c.lock().ok().and_then(|s| s.nvdata().mac().ok().flatten()))
    {
        return Some(MacAddr(mac));
    }
    // Without the config crate (it failed, or safe mode found it broken),
    // read the one line we need directly.
    std::fs::read_to_string(ctx.paths.nvdata.join("etc/enet.cfg"))
        .ok()
        .and_then(|text| opendeskphone_net::parse_enet_cfg_mac(&text))
}

fn kernel_mac(name: &str) -> Option<MacAddr> {
    let text = std::fs::read_to_string(format!("/sys/class/net/{name}/address")).ok()?;
    MacAddr::parse(text.trim()).filter(MacAddr::is_valid_unicast)
}

/// The plainest configuration: standalone `lan`, DHCP, no VLAN, PC jack down.
fn plain_config(ctx: &Context, mac: Option<MacAddr>) -> NetConfig {
    let mut cfg = NetConfig::new(ctx.model.name(), &crate::stock_version(), mac);
    cfg.pc_port = PcPort::Disabled;
    // Development and emulator use: `dphone.lan=<interface>` on the kernel
    // command line names a plain Ethernet interface to use in place of the
    // switch (no conduit, no PC jack).
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if let Some(name) = cmdline.split_ascii_whitespace().find_map(|w| w.strip_prefix("dphone.lan=")) {
        cfg.ifaces.conduit = None;
        cfg.ifaces.pc = None;
        cfg.ifaces.lan = name.to_string();
    }
    cfg
}

/// The configured network: the merged phone configuration mapped onto the
/// manager's input.
///
/// Without a readable configuration this is the plain configuration, PC
/// jack down included: there are then no settings to bridge by.
fn configured(ctx: &Context, config: Option<&SharedConfig>, mac: Option<MacAddr>) -> NetConfig {
    let cfg = plain_config(ctx, mac);
    let Some(merged) = config.and_then(|c| c.lock().ok().map(|s| s.config())) else {
        return cfg;
    };
    let new_family = ctx.model.family() == crate::model::Family::New;
    let hold = bridge_hold_flag(ctx).exists();
    map_config(cfg, &merged, new_family, mac, hold, last_address(ctx))
}

/// Map the merged phone configuration onto `cfg` (the plain configuration
/// for this phone).  `hold_bridge_off` is the bring-up hold: the PC jack
/// stays down whatever the settings say.
fn map_config(
    mut cfg: NetConfig,
    merged: &opendeskphone_config::Config,
    new_family: bool,
    mac: Option<MacAddr>,
    hold_bridge_off: bool,
    last_address: Option<Ipv4Addr>,
) -> NetConfig {
    let n = merged.network();
    cfg.lan_port_mode = PortMode::parse(&n.lan_port_mode);
    cfg.pc_port_mode = PortMode::parse(&n.pc_port_mode);
    // Stock's switch always forwards between the jacks; its one way to
    // turn the PC jack off is `pc_port_mode` "off", which the topology
    // honors.
    cfg.pc_port = if hold_bridge_off { PcPort::Disabled } else { PcPort::Bridged };
    cfg.pc_vlan_id = n.pc_vlan_id.filter(|v| (1..=4094).contains(v));
    cfg.pc_qos = n.pc_qos.min(7);
    // Stock, new family only: an expansion module takes the PC VLAN away,
    // and the phone joins the modules' private VLAN.
    if new_family && merged.setting_bool("expansion_enable", false) {
        cfg.pc_vlan_id = None;
        cfg.expansion_address = Some(opendeskphone_hal::expansion::private_address(mac.map(|m| m.0).unwrap_or([0; 6])));
    }
    cfg.eapol_passthrough = n.dot1x_passthrough;
    // Stock enables the logoff together with pass-through, and has a
    // setting of its own for it as well.
    cfg.eapol_logoff = n.dot1x_passthrough || n.dot1x_eapol_on_disconnect;
    cfg.vlan = match n.vlan_discovery_mode.to_ascii_uppercase().as_str() {
        "MANUAL" => n.vlan_id.map(VlanDiscovery::Manual).unwrap_or(VlanDiscovery::None),
        "LLDP" => VlanDiscovery::Lldp,
        _ => VlanDiscovery::None,
    };
    cfg.qos.rtp_pcp = Some(n.rtp_qos);
    cfg.qos.rtp_dscp = Some(n.rtp_dscp);
    cfg.qos.sip_pcp = Some(n.sip_qos);
    cfg.qos.sip_dscp = Some(n.sip_dscp);
    // Stock's network settings, or the provisioning server's address; the
    // precedence is `Config::static_ipv4`'s.
    cfg.ipv4 = match (merged.static_ipv4(), n.dhcp) {
        (Some(s), _) => Ipv4Config::Static {
            address: s.address,
            prefix_len: s.prefix_len,
            gateways: s.gateway.into_iter().collect(),
            dns: s.dns,
            domain: s.domain,
        },
        (None, false) => {
            warn!("net: static addressing configured without a usable address and mask; using DHCP");
            Ipv4Config::Dhcp { arp_check: !n.disable_arping, requested_ip: None }
        }
        (None, true) => Ipv4Config::Dhcp { arp_check: !n.disable_arping, requested_ip: last_address },
    };
    cfg.ipv6 = match (n.ipv6_enabled, n.dhcp) {
        (false, _) => Ipv6Config::Disabled,
        (true, true) => Ipv6Config::Auto,
        // Static mode: the configured IPv6 address, if there is one.
        (true, false) => match n.ipv6_static.as_deref().and_then(|a| a.trim().parse().ok()) {
            Some(address) => Ipv6Config::Static {
                address,
                prefix_len: n.ipv6_mask.as_deref().and_then(|m| m.trim().trim_start_matches('/').parse().ok()).filter(|p| *p <= 128).unwrap_or(64),
                gateway: n.ipv6_gateway.as_deref().and_then(|g| g.trim().parse().ok()),
            },
            None => Ipv6Config::Disabled,
        },
    };
    cfg.ipv6_preferred = n.ipv6_preferred;
    let t = merged.time();
    cfg.ntp.enabled = !t.time_source.eq_ignore_ascii_case("manual");
    cfg.ntp.server = Some(t.ntp_server).filter(|s| !s.is_empty());
    cfg.ntp.prefer_option42 = t.ntp_prefer_option42;
    cfg.ntp.resync = Duration::from_secs(t.ntp_resync_secs as u64);
    cfg
}

/// Whether `active` has the provisioning server's static address: the one
/// that counts only once the server has answered the phone there.
fn awaits_server(active: &NetConfig, config: Option<&SharedConfig>) -> bool {
    let Ipv4Config::Static { address, .. } = &active.ipv4 else { return false };
    config
        .and_then(|c| c.lock().ok().and_then(|s| s.config().static_ipv4()))
        .is_some_and(|s| s.source == SettingSource::Provisioned && s.address == *address)
}

/// The address stock held last (`/nvdata/etc/ip-addr-eth0`), to ask the
/// DHCP server for the same one.  Read only.
fn last_address(ctx: &Context) -> Option<Ipv4Addr> {
    let text = std::fs::read_to_string(ctx.paths.nvdata.join("etc/ip-addr-eth0")).ok()?;
    text.split(|c: char| !(c.is_ascii_digit() || c == '.')).find_map(|w| w.parse().ok())
}

fn set_hostname(name: &str) {
    // SAFETY: the pointer and length describe `name`'s bytes.
    let rc = unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) };
    if rc != 0 {
        warn!("net: sethostname: {}", std::io::Error::last_os_error());
    }
}

async fn run(
    ctx: Context,
    config: Option<SharedConfig>,
    hub: super::system::ConfigHub,
    mac: Option<MacAddr>,
    mut cmds: mpsc::Receiver<Cmd>,
    // Only its resolver and marking slots are used here.
    handle: NetHandle,
    fallback: Duration,
) {
    let NetHandle { tx, resolver: resolver_slot, qos: qos_slot } = handle;
    // This task must not keep its own command channel open.
    drop(tx);
    let fallback_secs = fallback.as_secs();
    let plain = plain_config(&ctx, mac);
    // Safe mode goes straight to the plain configuration.
    let wanted = if ctx.safe { plain.clone() } else { configured(&ctx, config.as_ref(), mac) };
    if !ctx.safe && bridge_hold_flag(&ctx).exists() {
        warn!("net: {} exists; the PC jack is held off", bridge_hold_flag(&ctx).display());
    }
    set_hostname(&wanted.hostname);
    ctx.store.update::<opendeskphone_state::System, _>(|s| s.hostname = wanted.hostname.clone());

    // Start the manager; retry for as long as it takes.
    let manager = loop {
        match NetManager::start(wanted.clone()) {
            Ok(m) => break Arc::new(m),
            Err(e) => {
                error!("net: cannot start the network manager: {e}; retrying");
                ctx.store.set_health(Subsystem::Net, HealthStatus::Failed(e.to_string()));
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    };
    if wanted.ifaces.conduit.is_none() {
        warn!("net: using interface {} in place of the switch (dphone.lan= on the command line)", wanted.ifaces.lan);
    }
    info!(
        "net: manager started ({}{})",
        describe(&wanted),
        if wanted == plain { "" } else { "; falls back to plain DHCP on lan if no address" }
    );

    *resolver_slot.lock().unwrap() = Some(manager.resolver());
    let mut state_rx = manager.state();
    let mut active = wanted;
    // The arrangement the settings last gave, which a fallback leaves.
    let mut last_configured = active.clone();
    let mut fb = Fallback::None;
    // The PC jack as `dphone net bridge on|off` without `--save` left it.
    // Settings changes keep it (provisioning reports one each time the
    // network comes up, which would otherwise undo the command at once);
    // `dphone net configured`, `dphone net plain`, and a fallback end it.
    let mut pc_override: Option<PcPort> = None;
    let mut deadline = (active != plain).then(|| Instant::now() + fallback);
    let mut was_up = false;
    // The provisioning server's answer, for a static address it gave (see
    // the module documentation).  `confirmed`: the DPMA session was seen
    // detached and then attached since the present address came up.  The
    // discovery feed reports every new address to the DPMA client as the
    // network going down and up, so the session of the address before is
    // never taken for an answer at this one; a detachment missed here
    // leaves the address unconfirmed, which errs toward the fallback.
    let prov_sub = ctx.store.subscribe(Topic::Provisioning);
    let mut address: Option<Ipv4Addr> = None;
    let mut seen_detached = false;
    let mut confirmed = false;
    // The fallback in force followed a static address that the server did
    // not answer at.
    let mut static_failed = false;
    loop {
        // The server's answer, looked at after every event.
        let attached = ctx.store.read::<opendeskphone_state::Provisioning, _>(|p| p.attached);
        if !attached {
            seen_detached = true;
        } else if seen_detached && address.is_some() && !confirmed {
            confirmed = true;
            if active != plain && awaits_server(&active, config.as_ref()) {
                info!("net: the provisioning server answers at the static address ({})", describe(&active));
                deadline = None;
                if fb == Fallback::Retrying {
                    fb = Fallback::None;
                    static_failed = false;
                }
            } else if active == plain && fb == Fallback::Waiting && static_failed && !ctx.safe {
                // The server answers the plain arrangement: its silence
                // may have been its own, not the address's.
                active = configured(&ctx, config.as_ref(), mac);
                last_configured = active.clone();
                if active == plain {
                    fb = Fallback::None;
                    static_failed = false;
                } else {
                    fb = Fallback::Retrying;
                    info!(
                        "net: the provisioning server answers the plain arrangement; trying the settings once more ({})",
                        describe(&active)
                    );
                    deadline = Some(Instant::now() + fallback);
                    manager.reconfigure(active.clone()).await;
                }
            }
        }
        tokio::select! {
            _ = prov_sub.changed() => {}
            changed = state_rx.changed() => {
                if changed.is_err() {
                    error!("net: the network manager task ended");
                    ctx.store.set_health(Subsystem::Net, HealthStatus::Failed("manager task ended".into()));
                    ctx.health.fail("net");
                    return;
                }
                let state = state_rx.borrow_and_update().clone();
                *qos_slot.lock().unwrap() = state.qos;
                publish(&ctx, &state, &active);
                let up = state.has_address();
                let now_address = state.ipv4.as_ref().map(|v| v.address);
                if now_address != address {
                    // Another address (or none): the server has not
                    // answered at it yet.
                    address = now_address;
                    confirmed = false;
                    seen_detached = !ctx.store.read::<opendeskphone_state::Provisioning, _>(|p| p.attached);
                }
                // The merge selects the network profile (and with it the
                // accounts) by the phone's address, as stock does.
                if let (Some(config), Some(v4)) = (&config, state.ipv4.as_ref()) {
                    let merged = {
                        let mut c = config.lock().unwrap();
                        let before = c.config().generation();
                        match c.set_ip(Some(v4.address)) {
                            Ok(cfg) if cfg.generation() != before => Some(cfg),
                            Ok(_) => None,
                            Err(e) => {
                                warn!("net: configuration merge for {}: {e}", v4.address);
                                None
                            }
                        }
                    };
                    if let Some(cfg) = merged {
                        info!("net: network profile selected for {}", v4.address);
                        hub.changed(&ctx.store, &cfg);
                    }
                }
                if up && !was_up {
                    let addr = state.ipv4.as_ref().map(|v| format!("{}/{}", v.address, v.prefix_len)).unwrap_or_default();
                    info!("net: up on {} {addr}", state.interface);
                    ctx.health.satisfy("net");
                    ctx.store.set_health(Subsystem::Net, HealthStatus::Ok);
                    // The server's static address has more to show than
                    // being there: its time keeps running.
                    let awaited = active != plain && awaits_server(&active, config.as_ref());
                    if !awaited {
                        deadline = None;
                    } else if deadline.is_none() {
                        deadline = Some(Instant::now() + fallback);
                    }
                    match fb {
                        // After a static address the server did not answer
                        // at, the second try waits for the server.
                        Fallback::Waiting if static_failed => {}
                        Fallback::Waiting if !ctx.safe => {
                            // The network answers: the outage may have
                            // been its fault, not the settings'.
                            active = configured(&ctx, config.as_ref(), mac);
                            last_configured = active.clone();
                            if active == plain {
                                fb = Fallback::None;
                            } else {
                                fb = Fallback::Retrying;
                                info!(
                                    "net: the plain arrangement has an address, so the network answers; trying the settings once more ({})",
                                    describe(&active)
                                );
                                deadline = Some(Instant::now() + fallback);
                                manager.reconfigure(active.clone()).await;
                            }
                        }
                        Fallback::Retrying if active != plain && !awaited => {
                            info!("net: the settings have an address again ({})", describe(&active));
                            fb = Fallback::None;
                            static_failed = false;
                        }
                        _ => {}
                    }
                } else if !up && was_up {
                    warn!("net: address lost");
                    if active != plain {
                        deadline = Some(Instant::now() + fallback);
                    }
                }
                was_up = up;
            }
            _ = async { tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)).await }, if deadline.is_some() => {
                deadline = None;
                let has_address = manager.snapshot().has_address();
                if active == plain {
                    // Armed on the plain arrangement only after a static
                    // address the server did not answer at.  Nothing
                    // answers here either: back to the settings, in turns.
                    if fb == Fallback::Waiting && static_failed && !has_address && !ctx.safe {
                        active = configured(&ctx, config.as_ref(), mac);
                        last_configured = active.clone();
                        fb = Fallback::None;
                        static_failed = false;
                        if active != plain {
                            info!(
                                "net: no address after {fallback_secs} s with plain DHCP on lan either; back to the settings ({})",
                                describe(&active)
                            );
                            deadline = Some(Instant::now() + fallback);
                            manager.reconfigure(active.clone()).await;
                        }
                    }
                    continue;
                }
                let unanswered = has_address && awaits_server(&active, config.as_ref()) && !confirmed;
                // A settings change that the manager applied without a new
                // bring-up re-arms the timer while the address stays.
                if has_address && !unanswered {
                    continue;
                }
                let what = if unanswered { "no answer from the provisioning server" } else { "no address" };
                if fb == Fallback::Retrying {
                    fb = Fallback::Final;
                    warn!(
                        "net: {what} after {fallback_secs} s with {} on the second try either; falling back to plain DHCP on lan and staying there until 'dphone net configured', a settings change, or a restart",
                        describe(&active)
                    );
                } else {
                    fb = Fallback::Waiting;
                    warn!(
                        "net: {what} after {fallback_secs} s with {}; falling back to plain DHCP on lan (the settings are tried once more when {})",
                        describe(&active),
                        if unanswered { "the server answers there" } else { "that has an address" }
                    );
                }
                static_failed = unanswered;
                // Where the plain arrangement gets no address either, the
                // static address gets its turn again.
                if unanswered && fb == Fallback::Waiting {
                    deadline = Some(Instant::now() + fallback);
                }
                active = plain.clone();
                pc_override = None;
                manager.reconfigure(active.clone()).await;
            }
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    Cmd::Show(reply) => {
                        let awaited = active != plain && awaits_server(&active, config.as_ref());
                        let _ = reply.send(show(&manager.snapshot(), &active, &plain, deadline, fb, awaited.then_some(confirmed), static_failed));
                    }
                    Cmd::SyncTime => manager.sync_time().await,
                    Cmd::Browse(window, reply) => {
                        // Off this task: the browse takes its whole window.
                        let manager = manager.clone();
                        tokio::spawn(async move {
                            let found = manager.browse_digium_proxy(window).await.unwrap_or_default();
                            let _ = reply.send(found);
                        });
                    }
                    Cmd::Plain(reply) => {
                        active = plain.clone();
                        pc_override = None;
                        fb = Fallback::None;
                        static_failed = false;
                        deadline = None;
                        manager.reconfigure(active.clone()).await;
                        let _ = reply.send("plain configuration applied\n".into());
                    }
                    Cmd::Changed => {
                        let mut now = configured(&ctx, config.as_ref(), mac);
                        if let Some(pc) = pc_override {
                            now.pc_port = pc;
                        }
                        // In a fallback the plain arrangement is in force
                        // on purpose: only settings that differ from the
                        // ones that failed end it.
                        let differs = match fb {
                            Fallback::Waiting | Fallback::Final => now != last_configured,
                            Fallback::None | Fallback::Retrying => now != active,
                        };
                        if differs {
                            if fb != Fallback::None {
                                info!("net: the settings changed; the fallback ends");
                                fb = Fallback::None;
                                static_failed = false;
                            }
                            let reasons = active.restart_reasons(&now);
                            let restart = !reasons.is_empty();
                            if restart {
                                info!(
                                    "net: network settings changed ({}); bringing the network up again with {}",
                                    reasons.join(", "),
                                    describe(&now)
                                );
                            } else {
                                info!("net: network settings changed; applying in place: {}", describe(&now));
                            }
                            active = now;
                            last_configured = active.clone();
                            if restart {
                                deadline = (active != plain).then(|| Instant::now() + fallback);
                            }
                            manager.reconfigure(active.clone()).await;
                        }
                    }
                    Cmd::Configured(reply) => {
                        pc_override = None;
                        active = configured(&ctx, config.as_ref(), mac);
                        last_configured = active.clone();
                        fb = Fallback::None;
                        static_failed = false;
                        deadline = (active != plain).then(|| Instant::now() + fallback);
                        manager.reconfigure(active.clone()).await;
                        let _ = reply.send(format!("applied: {}\n", describe(&active)));
                    }
                    Cmd::Bridge { on, save, reply } => {
                        let mut text = String::new();
                        if save {
                            let flag = bridge_hold_flag(&ctx);
                            let result = if on {
                                match std::fs::remove_file(&flag) {
                                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                                    _ => Ok(()),
                                }
                            } else {
                                crate::paths::write_atomic(&flag, b"1\n", 0o644)
                            };
                            match result {
                                Ok(()) if on => text.push_str(&format!("removed {}: the PC jack follows the settings from now on\n", flag.display())),
                                Ok(()) => text.push_str(&format!("wrote {}: the PC jack stays off until 'dphone net bridge on --save'\n", flag.display())),
                                Err(e) => text.push_str(&format!("could not update {}: {e}\n", flag.display())),
                            }
                        }
                        // The configured arrangement with the PC jack as
                        // asked; from the plain fallback, "on" returns to
                        // the settings.
                        let mut next = if on && active == plain && !ctx.safe {
                            configured(&ctx, config.as_ref(), mac)
                        } else {
                            active.clone()
                        };
                        next.pc_port = if on { PcPort::Bridged } else { PcPort::Disabled };
                        // With `--save` the settings (and the hold file
                        // just written or removed) decide from now on.
                        pc_override = (!save).then_some(next.pc_port);
                        active = next;
                        fb = Fallback::None;
                        static_failed = false;
                        deadline = (active != plain).then(|| Instant::now() + fallback);
                        manager.reconfigure(active.clone()).await;
                        text.push_str(&format!(
                            "bridging {}; this session may drop while the network is rearranged{}\n",
                            if on { "on" } else { "off" },
                            if on { format!(" (reverts by itself if there is no address after {fallback_secs} s)") } else { String::new() },
                        ));
                        let _ = reply.send(text);
                    }
                }
            }
        }
    }
}

fn describe(cfg: &NetConfig) -> String {
    let ip = match &cfg.ipv4 {
        Ipv4Config::Dhcp { .. } => "DHCP".to_string(),
        Ipv4Config::Static { address, prefix_len, .. } => format!("static {address}/{prefix_len}"),
    };
    let vlan = match cfg.vlan {
        VlanDiscovery::None => "no VLAN".to_string(),
        VlanDiscovery::Manual(id) => format!("VLAN {id}"),
        VlanDiscovery::Lldp => "VLAN from LLDP".to_string(),
    };
    let pc = match (cfg.pc_port, cfg.pc_port_mode) {
        (PcPort::Bridged, PortMode::Off) => "PC jack off (pc_port_mode)",
        (PcPort::Bridged, _) => "PC jack bridged",
        (PcPort::Disabled, _) => "PC jack off",
    };
    format!("{ip}, {vlan}, {pc}")
}

/// `answered`: for the provisioning server's static address, whether the
/// server has answered the phone there; `None` for any other arrangement.
/// `static_failed`: the fallback followed such an address without an answer.
fn show(
    state: &NetState,
    active: &NetConfig,
    plain: &NetConfig,
    deadline: Option<Instant>,
    fb: Fallback,
    answered: Option<bool>,
    static_failed: bool,
) -> String {
    let mut t = String::new();
    t.push_str(&format!("config:    {}{}\n", describe(active), if active == plain { " (plain)" } else { "" }));
    match answered {
        Some(true) => t.push_str("static:    from the provisioning server, which answers at this address\n"),
        Some(false) => t.push_str("static:    from the provisioning server, which has not answered at this address yet\n"),
        None => {}
    }
    if let Some(d) = deadline {
        let left = d.saturating_duration_since(Instant::now()).as_secs();
        if active == plain {
            t.push_str(&format!("fallback:  back to the settings in {left} s unless an address arrives\n"));
        } else if answered == Some(false) && state.has_address() {
            t.push_str(&format!("fallback:  to plain in {left} s unless the provisioning server answers\n"));
        } else {
            t.push_str(&format!("fallback:  to plain in {left} s unless an address arrives\n"));
        }
    }
    match (fb, static_failed) {
        (Fallback::None, _) => {}
        (Fallback::Waiting, false) => t.push_str("fallback:  plain after the settings got no address; they are tried once more when this has an address\n"),
        (Fallback::Waiting, true) => t.push_str("fallback:  plain after the provisioning server did not answer at the static address; that is tried once more when the server answers here\n"),
        (Fallback::Retrying, _) => t.push_str("fallback:  the settings are on their second try\n"),
        (Fallback::Final, false) => t.push_str("fallback:  plain after the settings got no address twice; stays until 'dphone net configured', a settings change, or a restart\n"),
        (Fallback::Final, true) => t.push_str("fallback:  plain after the provisioning server did not answer at the static address twice; stays until 'dphone net configured', a settings change, or a restart\n"),
    }
    t.push_str(&format!("phase:     {}\n", words(&state.phase)));
    t.push_str(&format!("hostname:  {}\n", active.hostname));
    if let Some(m) = state.mac {
        t.push_str(&format!("mac:       {}\n", m.bare_hex()));
    }
    let port = |p: &opendeskphone_net::PortState| {
        if !p.present {
            "absent".to_string()
        } else if !p.link {
            "no link".to_string()
        } else {
            format!(
                "{} Mb/s {}",
                p.speed.map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                match p.full_duplex {
                    Some(true) => "full",
                    Some(false) => "half",
                    None => "?",
                }
            )
        }
    };
    t.push_str(&format!("lan:       {}\n", port(&state.lan)));
    t.push_str(&format!("pc:        {}\n", port(&state.pc)));
    t.push_str(&format!("interface: {}{}\n", state.interface, if state.bridged { " (bridged)" } else { "" }));
    if let Some(v) = state.vlan_id {
        t.push_str(&format!("vlan:      {v} ({})\n", words(&state.vlan_source)));
    }
    match &state.ipv4 {
        Some(v) => {
            t.push_str(&format!("ipv4:      {}/{} ({})\n", v.address, v.prefix_len, words(&v.source)));
            let gw: Vec<String> = v.gateways.iter().map(|g| g.to_string()).collect();
            t.push_str(&format!("gateway:   {}\n", gw.join(" ")));
            if let Some(s) = v.dhcp_server {
                t.push_str(&format!("dhcp:      server {s}, lease {} s\n", v.lease_secs.unwrap_or(0)));
            }
        }
        None => t.push_str("ipv4:      none\n"),
    }
    for a in &state.ipv6 {
        t.push_str(&format!("ipv6:      {a}\n"));
    }
    match (state.expansion, active.expansion_address) {
        (Some(a), _) => t.push_str(&format!(
            "expansion: {} {a}/{} (VLAN {})\n",
            opendeskphone_net::topology::EXPANSION_IF,
            opendeskphone_net::topology::EXPANSION_PREFIX,
            opendeskphone_net::topology::EXPANSION_VID
        )),
        (None, Some(_)) => t.push_str("expansion: enabled, but the interface is not up\n"),
        (None, None) => {}
    }
    let dns: Vec<String> = state.dns.iter().map(|d| d.to_string()).collect();
    t.push_str(&format!("dns:       {}\n", dns.join(" ")));
    if let Some(s) = &state.tftp_server {
        t.push_str(&format!("option 66: {s}\n"));
    }
    t.push_str(&format!("time sync: {}\n", if state.time_synced { "yes" } else { "no" }));
    for n in &state.lldp_neighbors {
        t.push_str(&format!(
            "lldp:      {} {}\n",
            n.system_name.as_deref().unwrap_or("?"),
            n.port_description.as_deref().unwrap_or("")
        ));
    }
    if let Some(e) = &state.last_error {
        t.push_str(&format!("last error: {e}\n"));
    }
    t
}

/// Copy the manager's state into the state store.
fn publish(ctx: &Context, state: &NetState, active: &NetConfig) {
    use opendeskphone_state as st;
    let port = |p: &opendeskphone_net::PortState| st::PortLink {
        up: p.present && p.link,
        speed: if p.link { p.speed.unwrap_or(0).min(u16::MAX as u32) as u16 } else { 0 },
        full_duplex: p.link && p.full_duplex.unwrap_or(false),
    };
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let synced = state.time_synced;
    ctx.store.update::<st::Network, _>(|n| {
        n.phase = match state.phase {
            Phase::Up => st::NetPhase::Up,
            Phase::Starting | Phase::WaitingForInterfaces | Phase::WaitingForLink => st::NetPhase::Down,
            Phase::LldpDiscovery | Phase::AcquiringAddress => st::NetPhase::Configuring,
        };
        n.detail = state.last_error.clone().unwrap_or_default();
        n.lan = port(&state.lan);
        n.pc = port(&state.pc);
        n.pc_port_enabled = active.pc_port == PcPort::Bridged && active.pc_port_mode != PortMode::Off;
        n.vlan.voice_id = state.vlan_id;
        n.vlan.source = match state.vlan_source {
            Some(opendeskphone_net::VlanSource::Manual) => st::VlanSource::Config,
            Some(opendeskphone_net::VlanSource::Lldp) => st::VlanSource::Lldp,
            None => st::VlanSource::None,
        };
        n.vlan.voice_priority = state.qos.rtp_pcp;
        n.vlan.dscp = state.qos.rtp_dscp;
        n.vlan.pc_id = active.pc_vlan_id;
        n.ipv4 = match &state.ipv4 {
            Some(v) => {
                let keep = n.ipv4.address == Some(v.address) && n.ipv4.lease_obtained_mono_ms != 0;
                st::Ipv4State {
                    source: match v.source {
                        AddressSource::Dhcp => st::AddrSource::Dhcp,
                        AddressSource::Static => st::AddrSource::Static,
                    },
                    address: Some(v.address),
                    prefix_len: v.prefix_len,
                    gateway: v.gateways.first().copied(),
                    lease_secs: v.lease_secs.unwrap_or(0),
                    lease_obtained_mono_ms: if keep { n.ipv4.lease_obtained_mono_ms } else { st::clock::mono_ms() },
                    dhcp_server: v.dhcp_server,
                }
            }
            None => st::Ipv4State::default(),
        };
        n.ipv6 = state
            .ipv6
            .iter()
            .map(|a| st::Ipv6Address { address: *a, prefix_len: 64, source: st::AddrSource::Slaac })
            .collect();
        n.dns = state.dns.clone();
        n.domain = state.domain.clone().unwrap_or_default();
        n.dhcp_tftp_server = state.tftp_server.clone().unwrap_or_default();
        n.ntp_servers = state.ntp_servers.iter().map(|a| a.to_string()).collect();
        if synced && n.ntp_synced_mono_ms.is_none() {
            n.ntp_synced_mono_ms = Some(st::clock::mono_ms());
        }
        n.lldp = state.lldp_neighbors.first().map(|l| st::LldpNeighbor {
            chassis_id: hex(&l.chassis_id),
            port_id: hex(&l.port_id),
            system_name: l.system_name.clone().unwrap_or_default(),
            port_description: l.port_description.clone().unwrap_or_default(),
        });
    });
    if synced {
        ctx.store.update::<st::System, _>(|s| s.time_synced = true);
    }
}

/// The network: status, and the PC port's bridge
///
/// Without a subcommand: status.
#[derive(Debug, Parser)]
#[command(name = "net")]
pub struct NetCommand {
    #[command(subcommand)]
    action: Option<NetAction>,
}

#[derive(Debug, Subcommand)]
enum NetAction {
    /// Show the interfaces, addresses, and what the network task is doing
    Status,
    /// Turn the bridge to the PC jack on or off
    ///
    /// Until the settings are applied again, or for good with --save.
    Bridge {
        state: crate::cli::OnOff,
        /// Keep it: 'off --save' writes the file that holds the PC jack off
        /// whatever the settings say, and 'on --save' removes it
        #[arg(long)]
        save: bool,
    },
    /// Apply the plainest arrangement: DHCP, no VLAN, PC jack down
    Plain,
    /// Apply the arrangement the settings ask for
    Configured,
    /// Synchronize the clock with the time server now
    SyncTime,
}

fn register_diag(ctx: &Context, tx: mpsc::Sender<Cmd>) {
    ctx.diag.register(move |args: NetCommand, out: Output| {
        let tx = tx.clone();
        async move {
            let (reply_tx, reply_rx) = oneshot::channel();
            let cmd = match args.action.unwrap_or(NetAction::Status) {
                NetAction::Status => Cmd::Show(reply_tx),
                NetAction::Bridge { state, save } => Cmd::Bridge { on: state.is_on(), save, reply: reply_tx },
                NetAction::Plain => Cmd::Plain(reply_tx),
                NetAction::Configured => Cmd::Configured(reply_tx),
                NetAction::SyncTime => {
                    let _ = tx.send(Cmd::SyncTime).await;
                    out.out("time synchronization requested\n").await;
                    return 0;
                }
            };
            if tx.send(cmd).await.is_err() {
                out.err("dphone net: the network task is not running\n").await;
                return 1;
            }
            match tokio::time::timeout(Duration::from_secs(10), reply_rx).await {
                Ok(Ok(text)) => {
                    out.out(&text).await;
                    0
                }
                _ => {
                    out.err("dphone net: no answer from the network task (still starting?)\n").await;
                    1
                }
            }
        }
    });
}

/// A state name for people, from an enum's variant name: "LldpDiscovery"
/// becomes "LLDP discovery", "Dhcp" becomes "DHCP".
fn words(value: &impl std::fmt::Debug) -> String {
    const ACRONYMS: [&str; 6] = ["LLDP", "DHCP", "VLAN", "DNS", "NTP", "IP"];
    let name = format!("{value:?}");
    let mut parts: Vec<String> = Vec::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() || parts.is_empty() {
            parts.push(String::new());
        }
        parts.last_mut().unwrap().push(c);
    }
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        match ACRONYMS.iter().find(|a| a.eq_ignore_ascii_case(part)) {
            Some(a) => out.push_str(a),
            None if i == 0 => out.push_str(part),
            None => out.push_str(&part.to_ascii_lowercase()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_config::{ConfigStore, Model, Nvdata};

    /// The layout a D60 gets from its DPMA server: one network profile
    /// selected by address, and settings that only count on that network.
    /// (The ids and the `network_id` attributes are the ones on the
    /// bring-up phone; the addresses are examples.)
    const FETCHED: &str = r#"<config>
<networks>
<network id="lan" display_name="LAN" cidr="198.18.14.0/23" />
</networks>
<setting id="config_server_url" value="sip:proxy@198.18.14.3:5060;transport=udp" network_id="lan" />
<setting id="enable_logging" value="0" network_id="lan" />
<setting id="ntp_server" value="0.pool.ntp.org" network_id="lan" />
<setting id="pc_qos" value="0" network_id="lan" />
<setting id="pc_vlan_id" value="0" network_id="lan" />
<setting id="rtp_dscp" value="0" network_id="lan" />
<setting id="sip_dscp" value="0" network_id="lan" />
<setting id="udp_ka_interval" value="0" network_id="lan" />
<setting id="network_enable_dhcp" value="1" />
<setting id="network_vlan_discovery_mode" value="LLDP" />
<setting id="time_source" value="ntp" />
</config>"#;

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let nv = Nvdata::new(dir.path().join("nvdata"));
        nv.ensure_layout().unwrap();
        let mut store = ConfigStore::open(nv, Model::D60, dir.path().join("tmp")).unwrap();
        store.apply_fetched(FETCHED).unwrap();
        (dir, store)
    }

    fn mapped(store: &ConfigStore) -> NetConfig {
        let mac = MacAddr::parse("00:0f:d3:01:02:03");
        let mut plain = NetConfig::new("D60", "3_0_0", mac);
        plain.pc_port = PcPort::Disabled;
        map_config(plain, &store.config(), true, mac, false, None)
    }

    /// A phone with nothing but the built-in defaults bridges the PC jack,
    /// as stock does, and the stock settings steer it.
    #[test]
    fn pc_jack_is_bridged_by_default_and_follows_the_stock_settings() {
        use opendeskphone_net::topology;
        let dir = tempfile::tempdir().unwrap();
        let nv = Nvdata::new(dir.path().join("nvdata"));
        nv.ensure_layout().unwrap();
        let mut store = ConfigStore::open(nv, Model::D60, dir.path().join("tmp")).unwrap();
        let cfg = mapped(&store);
        assert_eq!(cfg.pc_port, PcPort::Bridged);
        assert_eq!((cfg.pc_port_mode, cfg.pc_vlan_id, cfg.pc_qos), (PortMode::Auto, None, 1));
        assert!(!cfg.eapol_passthrough && !cfg.eapol_logoff);
        let plan = topology::plan(&cfg, true, true, None);
        assert_eq!(plan.bridge.map(|b| (b.vlan_filtering, b.group_fwd_mask)), Some((false, 0)));
        assert!(plan.pc_up);
        assert_eq!(describe(&cfg), "DHCP, VLAN from LLDP, PC jack bridged");

        // 802.1X pass-through, the PC VLAN, and its priority.
        store.set_many(&[("8021x_passthrough", "1"), ("pc_vlan_id", "20"), ("pc_qos", "5")]).unwrap();
        let cfg = mapped(&store);
        assert!(cfg.eapol_passthrough && cfg.eapol_logoff);
        assert_eq!((cfg.pc_vlan_id, cfg.pc_qos), (Some(20), 5));
        let plan = topology::plan(&cfg, true, true, None);
        assert_eq!(
            plan.bridge.map(|b| (b.vlan_filtering, b.group_fwd_mask)),
            Some((true, topology::GROUP_FWD_EAPOL))
        );
        assert_eq!(plan.pc_prio, 5);

        // Stock's own switch for the PC jack.
        store.set("pc_port_mode", "off").unwrap();
        let cfg = mapped(&store);
        let plan = topology::plan(&cfg, true, true, None);
        assert_eq!((plan.bridge, plan.pc_up), (None, false));
        assert_eq!(describe(&cfg), "DHCP, VLAN from LLDP, PC jack off (pc_port_mode)");
        store.set("pc_port_mode", "auto").unwrap();

        // The bring-up hold wins over the settings.
        let mac = MacAddr::parse("00:0f:d3:01:02:03");
        let held = map_config(NetConfig::new("D60", "3_0_0", mac), &store.config(), true, mac, true, None);
        assert_eq!(held.pc_port, PcPort::Disabled);
        assert_eq!(topology::plan(&held, true, true, None).bridge, None);
    }

    /// What the phone does at every boot: bring-up starts before there is
    /// an address, the lease selects the network profile, and the settings
    /// tied to that profile change.  None of that may restart bring-up.
    #[test]
    fn profile_selection_after_the_lease_applies_in_place() {
        let (_dir, mut store) = store();
        let before = mapped(&store);
        let generation = store.config().generation();
        let selected = store.set_ip(Some(Ipv4Addr::new(198, 18, 15, 87))).unwrap();
        assert_ne!(selected.generation(), generation, "the lease should select the profile");
        let after = mapped(&store);
        // The profile's settings took effect...
        assert_eq!(after.ntp.server.as_deref(), Some("0.pool.ntp.org"));
        assert_eq!((after.qos.rtp_dscp, after.qos.sip_dscp), (Some(0), Some(0)));
        // pc_qos is the one that used to force a restart: it changes from
        // the default to the profile's value, with no PC VLAN to apply to.
        assert_eq!((before.pc_qos, after.pc_qos, after.pc_vlan_id), (1, 0, None));
        assert_ne!(before, after);
        // ...and all of them are ones a running session adopts.
        assert_eq!(before.restart_reasons(&after), Vec::<&str>::new());
    }

    /// The provisioning server's static address: it applies over the DHCP
    /// setting every phone holds in ui-config.xml, arriving and leaving are
    /// both reasons to bring the network up again, and only that address
    /// (not one set on the phone) waits for the server's answer.
    #[test]
    fn provisioned_static_address_maps_and_restarts() {
        const STATIC: &str = r#"<setting id="opendeskphone_static_address" value="198.18.14.120/23" />
<setting id="opendeskphone_static_gateway" value="198.18.14.1" />
<setting id="opendeskphone_static_dns" value="198.18.14.1,198.18.14.2" />
</config>"#;
        let (_dir, mut store) = store();
        store.set_many(&[("network_enable_dhcp", "1"), ("network_static_ip_address", ""), ("network_subnet_mask", "")]).unwrap();
        let dhcp = mapped(&store);
        assert!(matches!(dhcp.ipv4, Ipv4Config::Dhcp { .. }));
        store.apply_fetched(&FETCHED.replace("</config>", STATIC)).unwrap();
        let fixed = mapped(&store);
        assert_eq!(
            fixed.ipv4,
            Ipv4Config::Static {
                address: Ipv4Addr::new(198, 18, 14, 120),
                prefix_len: 23,
                gateways: vec![Ipv4Addr::new(198, 18, 14, 1)],
                dns: vec![Ipv4Addr::new(198, 18, 14, 1), Ipv4Addr::new(198, 18, 14, 2)],
                domain: None,
            }
        );
        assert_eq!(describe(&fixed), "static 198.18.14.120/23, VLAN from LLDP, PC jack bridged");
        assert_eq!(dhcp.restart_reasons(&fixed), ["IPv4 addressing"]);
        let shared: SharedConfig = Arc::new(std::sync::Mutex::new(store));
        assert!(awaits_server(&fixed, Some(&shared)));
        assert!(!awaits_server(&dhcp, Some(&shared)));
        assert!(!awaits_server(&fixed, None));

        // A static address set on the phone wins, and is applied as stock
        // applies it.
        let mut store = shared.lock().unwrap();
        store
            .set_many(&[("network_enable_dhcp", "0"), ("network_static_ip_address", "192.168.7.9"), ("network_subnet_mask", "255.255.255.0")])
            .unwrap();
        let local = mapped(&store);
        assert_eq!(describe(&local), "static 192.168.7.9/24, VLAN from LLDP, PC jack bridged");
        drop(store);
        assert!(!awaits_server(&local, Some(&shared)));
        let mut store = shared.lock().unwrap();
        store.set("network_enable_dhcp", "1").unwrap();

        // The server takes its address away: DHCP again.
        store.apply_fetched(FETCHED).unwrap();
        let back = mapped(&store);
        assert!(matches!(back.ipv4, Ipv4Config::Dhcp { .. }));
        assert_eq!(fixed.restart_reasons(&back), ["IPv4 addressing"]);
    }

    #[test]
    fn expansion_enable_asks_for_the_interface() {
        let (_dir, mut store) = store();
        store.set_ip(Some(Ipv4Addr::new(198, 18, 15, 87))).unwrap();
        let before = mapped(&store);
        assert_eq!(before.expansion_address, None);
        store.set("expansion_enable", "1").unwrap();
        let after = mapped(&store);
        assert_eq!(after.expansion_address, Some(Ipv4Addr::new(172, 20, 2, 3)));
        assert_eq!(before.restart_reasons(&after), ["expansion modules"]);
        // Old-family phones have no expansion modules.
        let mac = MacAddr::parse("00:0f:d3:01:02:03");
        let old = map_config(NetConfig::new("D40", "3_0_0", mac), &store.config(), false, mac, false, None);
        assert_eq!(old.expansion_address, None);
    }
}
