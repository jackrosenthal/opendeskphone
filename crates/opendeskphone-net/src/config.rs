//! Plain configuration input for the network manager.  The integrator
//! fills these from the merged phone configuration; nothing here knows
//! about XML or setting IDs beyond the doc comments naming their source.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::ethtool::PortMode;
use crate::sys::MacAddr;

/// Kernel interface names.  The defaults match the phone's devicetree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfaceNames {
    /// The DSA conduit (the SoC's link to the switch CPU port).
    pub conduit: Option<String>,
    /// The LAN jack.
    pub lan: String,
    /// The PC jack.
    pub pc: Option<String>,
    /// Name for the bridge this crate creates over `lan` and `pc`.
    pub bridge: String,
}

impl Default for IfaceNames {
    fn default() -> Self {
        Self {
            conduit: Some("eth0".into()),
            lan: "lan".into(),
            pc: Some("pc".into()),
            bridge: "br0".into(),
        }
    }
}

/// What to do with the PC jack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PcPort {
    /// Switch traffic between the PC jack and the LAN jack.
    #[default]
    Bridged,
    /// Keep the PC jack down.
    Disabled,
}

/// `network_vlan_discovery_mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VlanDiscovery {
    /// No voice VLAN.
    #[default]
    None,
    /// `MANUAL`: use `network_vlan_id` (only 1 to 4094 creates a VLAN).
    Manual(u16),
    /// `LLDP`: take the voice VLAN from the switch's LLDP-MED policy.
    Lldp,
}

/// Configured markings; `None` or out-of-range values fall back to the
/// stock defaults (RTP 6/46, SIP 3/26).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QosConfig {
    /// `rtp_qos`, or legacy `network_vlan_qos`.
    pub rtp_pcp: Option<u8>,
    /// `rtp_dscp`.
    pub rtp_dscp: Option<u8>,
    /// `sip_qos`.
    pub sip_pcp: Option<u8>,
    /// `sip_dscp`.
    pub sip_dscp: Option<u8>,
}

/// IPv4 addressing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ipv4Config {
    /// `network_enable_dhcp` not `0`.
    Dhcp {
        /// False when `network_disable_arping=1`.
        arp_check: bool,
        /// Address held last time (stock `/nvdata/etc/ip-addr-<if>`).
        requested_ip: Option<Ipv4Addr>,
    },
    Static {
        /// `network_static_ip_address`.
        address: Ipv4Addr,
        /// From `network_subnet_mask`.
        prefix_len: u8,
        /// `network_default_gateway`; later entries get higher metrics.
        gateways: Vec<Ipv4Addr>,
        /// `network_primary_dns_server`, `network_secondary_dns_server`.
        dns: Vec<Ipv4Addr>,
        /// `network_default_domain_name`.
        domain: Option<String>,
    },
}

impl Default for Ipv4Config {
    fn default() -> Self {
        Self::Dhcp {
            arp_check: true,
            requested_ip: None,
        }
    }
}

/// IPv6 addressing (`network_ipv6_enabled`; new family only in stock).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Ipv6Config {
    #[default]
    Disabled,
    /// Router advertisements (kernel SLAAC) plus DHCPv6.
    Auto,
    Static {
        /// `network6_static_ip_address`.
        address: Ipv6Addr,
        /// `network6_mask`.
        prefix_len: u8,
        /// `network6_default_gateway`.
        gateway: Option<Ipv6Addr>,
    },
}

/// Time synchronization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtpConfig {
    pub enabled: bool,
    /// `ntp_server`.
    pub server: Option<String>,
    /// `ntp_prefer_option42`.
    pub prefer_option42: bool,
    /// `ntp_resync`; zero means the stock default of a day.
    pub resync: Duration,
    /// Step the system clock on success.  Off in tests and the simulator.
    pub set_clock: bool,
}

impl Default for NtpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            server: None,
            prefer_option42: false,
            resync: crate::sntp::DEFAULT_RESYNC,
            set_clock: true,
        }
    }
}

/// LLDP settings and the inventory strings advertised over LLDP-MED.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LldpConfig {
    pub enabled: bool,
    /// PoE draw advertised; see [`stock_power_mw`].
    pub power_mw: u32,
    pub serial: String,
    pub manufacturer: String,
    pub hardware_rev: String,
}

impl Default for LldpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            power_mw: 6400,
            serial: String::new(),
            manufacturer: "Digium".into(),
            hardware_rev: String::new(),
        }
    }
}

/// The power each model's stock firmware advertises, in milliwatts.
pub fn stock_power_mw(model: &str) -> u32 {
    match model.to_ascii_uppercase().as_str() {
        "D40" | "D50" => 3900,
        "D70" => 12900,
        _ => 6400,
    }
}

/// Everything the network manager needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetConfig {
    pub ifaces: IfaceNames,
    /// `None` leaves whatever the kernel assigned.
    pub mac: Option<MacAddr>,
    /// Model name as stock writes it, e.g., `D60`.
    pub model: String,
    /// Firmware version in the stock style, e.g., `3_0_0`.
    pub version: String,
    /// Host name; stock uses `<sysname>-<MAC without colons>`.
    pub hostname: String,
    /// `lan_port_mode`.
    pub lan_port_mode: PortMode,
    /// `pc_port_mode` (`Off` takes the port down).
    pub pc_port_mode: PortMode,
    pub pc_port: PcPort,
    /// `pc_vlan_id` when 1 to 4094: untagged PC traffic is put in this VLAN.
    /// On the new family stock forces this off when `expansion_enable=1`;
    /// the integrator applies that rule.
    pub pc_vlan_id: Option<u16>,
    /// `pc_qos`: the 802.1p priority (0 to 7) of untagged frames from the
    /// PC jack while `pc_vlan_id` is in force.  Stock default 1.
    pub pc_qos: u8,
    /// `8021x_passthrough`: forward EAPOL between the PC and LAN jacks.
    pub eapol_passthrough: bool,
    /// With pass-through and a bridged PC jack: when the PC jack loses
    /// link, send an EAPOL-Logoff for the device that was behind it.  Stock
    /// turns this on together with pass-through (`eapLogoffEnable`), and
    /// has `8021x_eapol_on_disconnect` for it besides.
    pub eapol_logoff: bool,
    /// `expansion_enable`: the phone's address on the private VLAN of the
    /// expansion modules behind the PC jack (`172.20.<MAC byte 4>.<MAC
    /// byte 5>`, 16-bit prefix).  `Some` creates the interface
    /// [`crate::topology::EXPANSION_IF`].
    pub expansion_address: Option<Ipv4Addr>,
    pub vlan: VlanDiscovery,
    pub qos: QosConfig,
    pub ipv4: Ipv4Config,
    pub ipv6: Ipv6Config,
    /// `network_ipv6_preferred`: order IPv6 DNS servers first.
    pub ipv6_preferred: bool,
    /// Resolvers appended after the configured or leased ones.  Stock adds
    /// `8.8.8.8` in static mode; the default here is none.
    pub dns_fallback: Vec<IpAddr>,
    pub ntp: NtpConfig,
    pub lldp: LldpConfig,
    /// How long to wait for carrier before carrying on (stock: 20 s).
    pub link_wait: Duration,
    /// Passive LLDP listening time before the VLAN decision (stock: 5 s).
    pub lldp_wait: Duration,
}

impl NetConfig {
    /// Defaults for a model: DHCP, bridged PC port, no VLAN.
    pub fn new(model: &str, version: &str, mac: Option<MacAddr>) -> Self {
        let hostname = match mac {
            Some(m) => format!(
                "{}-{}",
                model.to_ascii_lowercase(),
                m.bare_hex().to_ascii_uppercase()
            ),
            None => model.to_ascii_lowercase(),
        };
        Self {
            ifaces: IfaceNames::default(),
            mac,
            model: model.to_string(),
            version: version.to_string(),
            hostname,
            lan_port_mode: PortMode::Auto,
            pc_port_mode: PortMode::Auto,
            pc_port: PcPort::Bridged,
            pc_vlan_id: None,
            pc_qos: 1,
            eapol_passthrough: false,
            eapol_logoff: false,
            expansion_address: None,
            vlan: VlanDiscovery::None,
            qos: QosConfig::default(),
            ipv4: Ipv4Config::default(),
            ipv6: Ipv6Config::Disabled,
            ipv6_preferred: false,
            dns_fallback: Vec::new(),
            ntp: NtpConfig::default(),
            lldp: LldpConfig {
                power_mw: stock_power_mw(model),
                serial: mac
                    .map(|m| m.bare_hex().to_ascii_uppercase())
                    .unwrap_or_default(),
                hardware_rev: model.to_string(),
                ..Default::default()
            },
            link_wait: Duration::from_secs(20),
            lldp_wait: Duration::from_secs(5),
        }
    }

    /// Whether changing from `self` to `new` needs the network brought up
    /// again.  Markings, time settings, resolver order, the fallback
    /// resolvers, and the address to ask DHCP for first can all change
    /// under a running session; everything else decides how interfaces are
    /// arranged or addressed.
    pub fn needs_restart(&self, new: &NetConfig) -> bool {
        !self.restart_reasons(new).is_empty()
    }

    /// The settings whose change from `self` to `new` needs the network
    /// brought up again, by name; empty when `new` can be adopted in place.
    ///
    /// Settings that have no effect in either configuration do not count:
    /// `pc_qos` only applies together with a PC VLAN.
    pub fn restart_reasons(&self, new: &NetConfig) -> Vec<&'static str> {
        let pc_vlan = |c: &NetConfig| c.pc_vlan_id.filter(|v| (1..=4094).contains(v));
        let ipv4_same = match (&self.ipv4, &new.ipv4) {
            (Ipv4Config::Dhcp { arp_check: a, .. }, Ipv4Config::Dhcp { arp_check: b, .. }) => a == b,
            (a, b) => a == b,
        };
        let checks = [
            ("interface names", self.ifaces == new.ifaces),
            ("MAC address", self.mac == new.mac),
            ("model", self.model == new.model),
            ("version", self.version == new.version),
            ("host name", self.hostname == new.hostname),
            ("lan_port_mode", self.lan_port_mode == new.lan_port_mode),
            ("pc_port_mode", self.pc_port_mode == new.pc_port_mode),
            ("PC port bridging", self.pc_port == new.pc_port),
            ("pc_vlan_id", pc_vlan(self) == pc_vlan(new)),
            ("pc_qos", self.pc_qos.min(7) == new.pc_qos.min(7) || (pc_vlan(self).is_none() && pc_vlan(new).is_none())),
            ("802.1X pass-through", self.eapol_passthrough == new.eapol_passthrough),
            ("EAPOL logoff", self.eapol_logoff == new.eapol_logoff),
            ("expansion modules", self.expansion_address == new.expansion_address),
            ("voice VLAN", self.vlan == new.vlan),
            ("IPv4 addressing", ipv4_same),
            ("IPv6 addressing", self.ipv6 == new.ipv6),
            ("LLDP", self.lldp == new.lldp),
            ("link wait", self.link_wait == new.link_wait && self.lldp_wait == new.lldp_wait),
        ];
        // Not listed, because a running session adopts them: `qos`, `ntp`,
        // `ipv6_preferred`, `dns_fallback`, and the DHCP `requested_ip`.
        // The destructuring makes a new field a compile error here until
        // it is put on one side or the other.
        let NetConfig {
            ifaces: _, mac: _, model: _, version: _, hostname: _, lan_port_mode: _, pc_port_mode: _, pc_port: _,
            pc_vlan_id: _, pc_qos: _, eapol_passthrough: _, eapol_logoff: _, expansion_address: _, vlan: _, qos: _,
            ipv4: _, ipv6: _, ipv6_preferred: _, dns_fallback: _, ntp: _, lldp: _, link_wait: _, lldp_wait: _,
        } = self;
        checks.into_iter().filter(|(_, same)| !same).map(|(name, _)| name).collect()
    }

    /// The stock DHCP vendor class, e.g., `digium_D60_3_0_0`.
    pub fn vendor_class(&self) -> String {
        crate::dhcp4::Config::stock_vendor_class(&self.model, &self.version)
    }

    /// LLDP system description.
    pub fn system_description(&self) -> String {
        format!(
            "Open Desk Phone, Digium {}, firmware version {}",
            self.model, self.version
        )
    }
}

/// Convert a dotted netmask to a prefix length; `None` when the mask is
/// not contiguous.
pub fn netmask_to_prefix(mask: Ipv4Addr) -> Option<u8> {
    let m = u32::from(mask);
    let ones = m.leading_ones();
    (m.checked_shl(ones).unwrap_or(0) == 0).then_some(ones as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_needs_a_restart() {
        let base = NetConfig::new("D60", "3_0_0", MacAddr::parse("00:0f:d3:01:02:03"));
        // Adopted in place.
        let mut live = base.clone();
        live.qos.rtp_dscp = Some(0);
        live.ntp.server = Some("0.pool.ntp.org".into());
        live.ipv6_preferred = true;
        live.dns_fallback = vec!["192.0.2.53".parse().unwrap()];
        live.ipv4 = Ipv4Config::Dhcp { arp_check: true, requested_ip: Some(Ipv4Addr::new(10, 0, 0, 9)) };
        // pc_qos without a PC VLAN has no effect, whatever its value.
        live.pc_qos = 0;
        live.pc_vlan_id = Some(0);
        assert_eq!(base.restart_reasons(&live), Vec::<&str>::new());
        assert!(!base.needs_restart(&live));
        // With a PC VLAN on either side, pc_qos counts.
        let mut with_vlan = base.clone();
        with_vlan.pc_vlan_id = Some(20);
        let mut other_prio = with_vlan.clone();
        other_prio.pc_qos = 5;
        assert_eq!(with_vlan.restart_reasons(&other_prio), ["pc_qos"]);
        assert_eq!(base.restart_reasons(&with_vlan), ["pc_vlan_id"]);
        // Arrangement and addressing changes.
        let mut hard = base.clone();
        hard.expansion_address = Some(Ipv4Addr::new(172, 20, 2, 3));
        hard.vlan = VlanDiscovery::Manual(100);
        hard.ipv4 = Ipv4Config::Dhcp { arp_check: false, requested_ip: None };
        assert_eq!(base.restart_reasons(&hard), ["expansion modules", "voice VLAN", "IPv4 addressing"]);
    }

    #[test]
    fn netmasks() {
        assert_eq!(netmask_to_prefix(Ipv4Addr::new(255, 255, 254, 0)), Some(23));
        assert_eq!(
            netmask_to_prefix(Ipv4Addr::new(255, 255, 255, 255)),
            Some(32)
        );
        assert_eq!(netmask_to_prefix(Ipv4Addr::new(0, 0, 0, 0)), Some(0));
        assert_eq!(netmask_to_prefix(Ipv4Addr::new(255, 0, 255, 0)), None);
    }

    #[test]
    fn defaults_follow_stock_shapes() {
        let c = NetConfig::new("D60", "3_0_0", MacAddr::parse("00:0f:d3:01:02:03"));
        assert_eq!(c.hostname, "d60-000FD3010203");
        assert_eq!(c.vendor_class(), "digium_D60_3_0_0");
        assert_eq!(stock_power_mw("d40"), 3900);
        assert_eq!(stock_power_mw("D70"), 12900);
        assert_eq!(stock_power_mw("D45"), 6400);
    }
}
