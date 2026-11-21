//! State store types to protobuf messages.

use crate::pb;
use opendeskphone_state as st;
use opendeskphone_state::clock::mono_ms;

fn secs_until(mono: u64) -> u32 {
    (mono.saturating_sub(mono_ms()) / 1000).min(u32::MAX as u64) as u32
}

pub fn model(m: st::Model) -> pb::Model {
    match m {
        st::Model::D40 => pb::Model::MODEL_D40,
        st::Model::D45 => pb::Model::MODEL_D45,
        st::Model::D50 => pb::Model::MODEL_D50,
        st::Model::D60 => pb::Model::MODEL_D60,
        st::Model::D62 => pb::Model::MODEL_D62,
        st::Model::D65 => pb::Model::MODEL_D65,
        st::Model::D70 => pb::Model::MODEL_D70,
        st::Model::D80 => pb::Model::MODEL_D80,
    }
}

pub fn skin(model: st::Model) -> pb::Skin {
    match model.skin() {
        st::Skin::SmallGray => pb::Skin::SmallGray,
        st::Skin::LargeGray => pb::Skin::LargeGray,
        st::Skin::Color => pb::Skin::Color,
        st::Skin::Touch => pb::Skin::Touch,
    }
}

pub fn system(s: &st::System, uptime_secs: u64) -> pb::SystemInfo {
    pb::SystemInfo {
        model: model(s.model).into(),
        version: s.version.dotted(),
        phase: match s.phase {
            st::Phase::Starting => pb::SystemPhase::Starting,
            st::Phase::NetworkWait => pb::SystemPhase::NetworkWait,
            st::Phase::Provisioning => pb::SystemPhase::Provisioning,
            st::Phase::Running => pb::SystemPhase::Running,
            st::Phase::Recovery => pb::SystemPhase::Recovery,
            st::Phase::ShuttingDown => pb::SystemPhase::ShuttingDown,
        }.into(),
        mac: s.mac_string(),
        hostname: s.hostname.clone(),
        time_synced: s.time_synced,
        timezone: s.timezone.clone(),
        last_reboot_reason: s.last_reboot_reason.clone(),
        simulated: s.simulated,
        mem_total_kib: s.mem_total_kib,
        uptime_secs,
        unix_time: st::clock::unix_secs(),
        skin: skin(s.model).into(),
        user_agent: s.model.user_agent(&s.version),
        has_bluetooth: s.model.bluetooth(),
        has_expansion: s.model.expansion_modules(),
    }
}

fn link(l: &st::PortLink) -> pb::PortLink {
    pb::PortLink { up: l.up, speed: l.speed as u32, full_duplex: l.full_duplex }
}

pub fn network(n: &st::Network, mac: String) -> pb::NetworkStatus {
    let opt = |a: Option<std::net::Ipv4Addr>| a.map(|a| a.to_string()).unwrap_or_default();
    let lldp = n.lldp.clone().unwrap_or_default();
    pb::NetworkStatus {
        phase: match n.phase {
            st::NetPhase::Down => pb::NetPhase::Down,
            st::NetPhase::Configuring => pb::NetPhase::Configuring,
            st::NetPhase::Up => pb::NetPhase::Up,
            st::NetPhase::Failed => pb::NetPhase::Failed,
        }.into(),
        detail: n.detail.clone(),
        lan: Some(link(&n.lan)).into(),
        pc: Some(link(&n.pc)).into(),
        pc_port_enabled: n.pc_port_enabled,
        mac,
        ipv4_source: match n.ipv4.source {
            st::AddrSource::Dhcp => pb::Ipv4Source::Dhcp,
            st::AddrSource::Static => pb::Ipv4Source::Static,
            st::AddrSource::None | st::AddrSource::Slaac | st::AddrSource::Dhcp6 => pb::Ipv4Source::Unspecified,
        }
        .into(),
        ipv4_address: opt(n.ipv4.address),
        ipv4_prefix_len: n.ipv4.prefix_len as u32,
        ipv4_gateway: opt(n.ipv4.gateway),
        lease_secs: n.ipv4.lease_secs,
        dhcp_server: opt(n.ipv4.dhcp_server),
        ipv6_addresses: n
            .ipv6
            .iter()
            .map(|a| pb::Ipv6Address { address: a.address.to_string(), prefix_len: a.prefix_len as u32 })
            .collect(),
        dns_servers: n.dns.iter().map(|a| pb::DnsServer { address: a.to_string() }).collect(),
        domain: n.domain.clone(),
        dhcp_tftp_server: n.dhcp_tftp_server.clone(),
        ntp_servers: n.ntp_servers.iter().map(|host| pb::NtpServer { host: host.clone() }).collect(),
        ntp_synced: n.ntp_synced_mono_ms.is_some(),
        vlan_source: match n.vlan.source {
            st::VlanSource::None => pb::VlanSource::Unspecified,
            st::VlanSource::Config => pb::VlanSource::Config,
            st::VlanSource::Lldp => pb::VlanSource::Lldp,
            st::VlanSource::Dhcp => pb::VlanSource::Dhcp,
        }
        .into(),
        voice_vlan: n.vlan.voice_id.map(u32::from),
        pc_vlan: n.vlan.pc_id.map(u32::from),
        vlan_priority: n.vlan.voice_priority as u32,
        dscp: n.vlan.dscp as u32,
        lldp_present: n.lldp.is_some(),
        lldp_system_name: lldp.system_name,
        lldp_port_id: lldp.port_id,
        lldp_chassis_id: lldp.chassis_id,
        lldp_port_description: lldp.port_description,
        profile: n.profile.clone(),
    }
}

fn server(s: &st::DiscoveredServer) -> pb::ProvisioningServer {
    pb::ProvisioningServer {
        source: match s.source {
            st::DiscoverySource::Saved => pb::DiscoverySource::Saved,
            st::DiscoverySource::DhcpOption66 => pb::DiscoverySource::Dhcp,
            st::DiscoverySource::Mdns => pb::DiscoverySource::Mdns,
            st::DiscoverySource::Manual => pb::DiscoverySource::Manual,
        }
        .into(),
        name: s.name.clone(),
        address: s.address.clone(),
    }
}

pub fn provisioning(p: &st::Provisioning) -> pb::ProvisioningStatus {
    use pb::ProvisionPhase as P;
    let (phase, failure_reason, failure_code) = match &p.phase {
        st::ProvisionPhase::Idle => (P::Idle, String::new(), None),
        st::ProvisionPhase::Discovering => (P::Discovering, String::new(), None),
        st::ProvisionPhase::AwaitingServer => (P::AwaitingServer, String::new(), None),
        st::ProvisionPhase::Handshaking => (P::Handshaking, String::new(), None),
        st::ProvisionPhase::AwaitingUser => (P::AwaitingUser, String::new(), None),
        st::ProvisionPhase::FetchingConfig => (P::FetchingConfig, String::new(), None),
        st::ProvisionPhase::Applying => (P::Applying, String::new(), None),
        st::ProvisionPhase::Provisioned => (P::Provisioned, String::new(), None),
        st::ProvisionPhase::Standalone => (P::Standalone, String::new(), None),
        st::ProvisionPhase::Failed { reason, code } => (P::Failed, reason.clone(), *code),
    };
    pb::ProvisioningStatus {
        phase: phase.into(),
        failure_reason,
        failure_code,
        discovered: p.discovered.iter().map(server).collect(),
        server: p.server.as_ref().map(server).into(),
        server_user_agent: p.server_user_agent.clone(),
        users: p
            .users
            .iter()
            .map(|u| pb::ProvisionUser { id: u.id.clone(), name: u.name.clone(), needs_pin: u.needs_pin })
            .collect(),
        assigned_user: p.assigned_user.clone(),
        attached: p.attached,
        config_md5: p.config_md5.clone(),
        offered_firmware: p.offered_firmware.clone().unwrap_or_default(),
        last_success_unix: p.last_success_unix.unwrap_or(0),
    }
}

pub fn transport(t: st::Transport) -> pb::SipTransport {
    match t {
        st::Transport::Udp => pb::SipTransport::Udp,
        st::Transport::Tcp => pb::SipTransport::Tcp,
        st::Transport::Tls => pb::SipTransport::Tls,
    }
}

pub fn accounts(a: &st::Accounts) -> Vec<pb::AccountStatus> {
    a.accounts
        .iter()
        .map(|acc| {
            use pb::RegistrationState as R;
            let mut out = pb::AccountStatus {
                index: acc.id.0 as u32,
                label: if acc.label.is_empty() { acc.user.clone() } else { acc.label.clone() },
                user: acc.user.clone(),
                server: acc.server.clone(),
                dpma: acc.dpma,
                transport: transport(acc.transport).into(),
                is_default: a.default_account == Some(acc.id),
                ..Default::default()
            };
            out.state = match &acc.registration {
                st::Registration::Disabled => R::Disabled,
                st::Registration::Unregistered => R::Unregistered,
                st::Registration::Registering => R::Registering,
                st::Registration::Registered { host, expires_mono_ms } => {
                    out.on_alternate_host = *host == st::ActiveHost::Alternate;
                    out.expires_in_secs = secs_until(*expires_mono_ms);
                    R::Registered
                }
                st::Registration::Failed { code, reason, retry_mono_ms } => {
                    out.code = *code as u32;
                    out.reason = reason.clone();
                    out.retry_in_secs = secs_until(*retry_mono_ms);
                    R::Failed
                }
            }.into();
            out
        })
        .collect()
}

pub fn party(p: &st::Party) -> pb::Party {
    pb::Party { name: p.name.clone(), number: p.number.clone(), uri: p.uri.clone() }
}

pub fn audio_path(p: st::AudioPath) -> pb::AudioPath {
    match p {
        st::AudioPath::None => pb::AudioPath::None,
        st::AudioPath::Handset => pb::AudioPath::Handset,
        st::AudioPath::Headset => pb::AudioPath::Headset,
        st::AudioPath::Speaker => pb::AudioPath::Speaker,
    }
}

pub fn audio_path_from(p: buffa::EnumValue<pb::AudioPath>) -> Option<st::AudioPath> {
    match p.as_known()? {
        pb::AudioPath::Unspecified => None,
        pb::AudioPath::None => Some(st::AudioPath::None),
        pb::AudioPath::Handset => Some(st::AudioPath::Handset),
        pb::AudioPath::Headset => Some(st::AudioPath::Headset),
        pb::AudioPath::Speaker => Some(st::AudioPath::Speaker),
    }
}

fn codec(c: st::Codec) -> pb::Codec {
    match c {
        st::Codec::Pcmu => pb::Codec::CODEC_PCMU,
        st::Codec::Pcma => pb::Codec::CODEC_PCMA,
        st::Codec::G722 => pb::Codec::CODEC_G722,
        st::Codec::G726_32 => pb::Codec::CODEC_G726_32,
        st::Codec::G729 => pb::Codec::CODEC_G729,
        st::Codec::Ilbc => pb::Codec::CODEC_ILBC,
        st::Codec::Opus => pb::Codec::CODEC_OPUS,
        st::Codec::L16 => pb::Codec::CODEC_L16,
        st::Codec::L16Wide => pb::Codec::CODEC_L16_WIDE,
    }
}

/// The cause, and the SIP status code and the text of the causes that
/// carry them.
fn end_cause(c: &st::EndCause) -> (pb::EndCause, u32, String) {
    use pb::EndCause as E;
    match c {
        st::EndCause::LocalHangup => (E::LocalHangup, 0, String::new()),
        st::EndCause::RemoteHangup => (E::RemoteHangup, 0, String::new()),
        st::EndCause::Rejected => (E::Rejected, 0, String::new()),
        st::EndCause::Canceled { answered_elsewhere: true } => (E::AnsweredElsewhere, 0, String::new()),
        st::EndCause::Canceled { answered_elsewhere: false } => (E::Canceled, 0, String::new()),
        st::EndCause::Busy => (E::Busy, 0, String::new()),
        st::EndCause::NoAnswer => (E::NoAnswer, 0, String::new()),
        st::EndCause::Transferred => (E::Transferred, 0, String::new()),
        st::EndCause::Failed { code, reason } => (E::Failed, u32::from(*code), reason.clone()),
        st::EndCause::Error(e) => (E::Error, 0, e.clone()),
    }
}

pub fn call(c: &st::Call, focused: bool) -> pb::Call {
    use pb::CallState as S;
    let (mut held_locally, mut held_remotely) = (false, false);
    let mut cause = (pb::EndCause::Unspecified, 0, String::new());
    let state = match &c.state {
        st::CallState::Dialing => S::Dialing,
        st::CallState::Proceeding => S::Proceeding,
        st::CallState::Ringback { .. } => S::Ringback,
        st::CallState::Ringing => S::Ringing,
        st::CallState::Active => S::Active,
        st::CallState::Held { local, remote } => {
            held_locally = *local;
            held_remotely = *remote;
            S::Held
        }
        st::CallState::Transferring => S::Transferring,
        st::CallState::Ended(e) => {
            cause = end_cause(e);
            S::Ended
        }
    };
    pb::Call {
        id: c.id.0,
        account: c.account.0 as u32,
        direction: match c.direction {
            st::Direction::Incoming => pb::CallDirection::Incoming,
            st::Direction::Outgoing => pb::CallDirection::Outgoing,
        }.into(),
        state: state.into(),
        remote: Some(party(&c.remote)).into(),
        started_unix: c.started_unix,
        duration_secs: c.duration_secs(mono_ms()),
        codec: c.codec.map(codec).unwrap_or_default().into(),
        srtp: c.srtp,
        muted: c.muted,
        in_conference: c.in_conference,
        held_locally,
        held_remotely,
        end_cause: cause.0.into(),
        focused,
        rx_packets: c.stats.rx_packets,
        tx_packets: c.stats.tx_packets,
        rx_lost: c.stats.rx_lost,
        jitter_ms: c.stats.jitter_ms as u32,
        paging: c.paging,
        end_code: cause.1,
        end_reason: cause.2,
    }
}

pub fn audio(a: &st::Audio) -> pb::AudioStatus {
    let (dsp, dsp_detail) = match &a.dsp {
        st::DspStatus::NotStarted => (pb::DspState::NotStarted, String::new()),
        st::DspStatus::Starting => (pb::DspState::Starting, String::new()),
        st::DspStatus::Running => (pb::DspState::Running, String::new()),
        st::DspStatus::Failed(e) => (pb::DspState::Failed, e.clone()),
    };
    pb::AudioStatus {
        path: audio_path(a.path).into(),
        off_hook: a.off_hook,
        headset_mode: a.headset_mode,
        muted: a.muted,
        handset_volume: a.handset_volume as u32,
        headset_volume: a.headset_volume as u32,
        speaker_volume: a.speaker_volume as u32,
        ringer_volume: a.ringer_volume as u32,
        ringing: a.ringing,
        dsp: dsp.into(),
        dsp_detail,
    }
}

pub fn calls_snapshot(s: &st::State) -> pb::CallsSnapshot {
    pb::CallsSnapshot {
        calls: s.calls.calls.iter().map(|c| call(c, s.calls.focused == Some(c.id))).collect(),
        audio: Some(audio(&s.audio)).into(),
        features: Some(pb::FeatureStatus {
            dnd: s.features.dnd,
            forward_always: s.features.forward.always.clone(),
            forward_busy: s.features.forward.busy.clone(),
            forward_no_answer: s.features.forward.no_answer.clone(),
            forward_no_answer_secs: s.features.forward.no_answer_secs as u32,
        }).into(),
        mwi: s
            .mwi
            .accounts
            .iter()
            .map(|m| pb::MwiStatus {
                account: m.account.0 as u32,
                waiting: m.waiting,
                new_messages: m.new,
                old_messages: m.old,
            })
            .collect(),
        missed_unseen: s.call_log.missed_unseen,
    }
}

pub fn call_log(l: &st::CallLog) -> Vec<pb::CallLogEntry> {
    l.entries
        .iter()
        .map(|e| pb::CallLogEntry {
            id: e.id,
            kind: match e.kind {
                st::CallLogKind::Missed => pb::CallLogKind::Missed,
                st::CallLogKind::Received => pb::CallLogKind::Received,
                st::CallLogKind::Placed => pb::CallLogKind::Placed,
            }.into(),
            party: Some(party(&e.party)).into(),
            account: e.account.0 as u32,
            start_unix: e.start_unix,
            duration_secs: e.duration_secs,
        })
        .collect()
}

pub fn firmware(u: &st::Update, version: &st::FirmwareVersion) -> pb::FirmwareStatus {
    use pb::UpdatePhase as P;
    let (phase, percent, error) = match &u.phase {
        st::UpdatePhase::Idle => (P::Idle, 0, String::new()),
        st::UpdatePhase::Receiving { percent } => (P::Receiving, *percent, String::new()),
        st::UpdatePhase::Checking => (P::Checking, 0, String::new()),
        st::UpdatePhase::Erasing { percent } => (P::Erasing, *percent, String::new()),
        st::UpdatePhase::WritingKernel { percent } => (P::WritingKernel, *percent, String::new()),
        st::UpdatePhase::WritingRootfs { percent } => (P::WritingRootfs, *percent, String::new()),
        st::UpdatePhase::Verifying { percent } => (P::Verifying, *percent, String::new()),
        st::UpdatePhase::Committing => (P::Committing, 0, String::new()),
        st::UpdatePhase::Complete => (P::Complete, 100, String::new()),
        st::UpdatePhase::Failed(e) => (P::Failed, 0, e.clone()),
    };
    let (trial, trial_boots_left, rollback_reason) = match &u.trial {
        st::TrialBoot::None => (pb::TrialBoot::Unspecified, 0, String::new()),
        st::TrialBoot::Pending { boots_left } => (pb::TrialBoot::Pending, *boots_left as u32, String::new()),
        st::TrialBoot::RolledBack { reason } => (pb::TrialBoot::RolledBack, 0, reason.clone()),
    };
    pb::FirmwareStatus {
        phase: phase.into(),
        percent: percent as u32,
        error,
        running_version: version.dotted(),
        running_bank: match u.running_bank {
            Some(st::Bank::A) => pb::Bank::BANK_A,
            Some(st::Bank::B) => pb::Bank::BANK_B,
            None => pb::Bank::BANK_UNSPECIFIED,
        }
        .into(),
        trial: trial.into(),
        trial_boots_left,
        rollback_reason,
        incoming_version: u.incoming_version.clone(),
        incoming_is_stock: u.incoming_is_stock,
        uploaded: None.into(),
    }
}

fn subsystem(s: st::Subsystem) -> pb::Subsystem {
    match s {
        st::Subsystem::Core => pb::Subsystem::Core,
        st::Subsystem::Config => pb::Subsystem::Config,
        st::Subsystem::Net => pb::Subsystem::Net,
        st::Subsystem::Sip => pb::Subsystem::Sip,
        st::Subsystem::Media => pb::Subsystem::Media,
        st::Subsystem::Dpma => pb::Subsystem::Dpma,
        st::Subsystem::Ui => pb::Subsystem::Ui,
        st::Subsystem::Apps => pb::Subsystem::Apps,
        st::Subsystem::Web => pb::Subsystem::Web,
        st::Subsystem::Ssh => pb::Subsystem::Ssh,
        st::Subsystem::Update => pb::Subsystem::Update,
    }
}

pub fn health(h: &st::Health) -> Vec<pb::SubsystemHealth> {
    st::Subsystem::ALL
        .iter()
        .map(|s| {
            use pb::HealthState as H;
            let (state, detail) = match h.status(*s) {
                st::HealthStatus::Stopped => (H::Stopped, String::new()),
                st::HealthStatus::Starting => (H::Starting, String::new()),
                st::HealthStatus::Ok => (H::Ok, String::new()),
                st::HealthStatus::Degraded(d) => (H::Degraded, d.clone()),
                st::HealthStatus::Failed(d) => (H::Failed, d.clone()),
            };
            pb::SubsystemHealth { subsystem: subsystem(*s).into(), state: state.into(), detail }
        })
        .collect()
}

/// The kind of an app, which the app manager gives by the names of the
/// manifest's kinds.
fn app_kind(kind: st::AppKind) -> pb::AppKind {
    match kind {
        st::AppKind::Foreground => pb::AppKind::Foreground,
        st::AppKind::Background => pb::AppKind::Background,
    }
}

pub fn apps(a: &st::Apps) -> pb::AppsSnapshot {
    // One list by name, so that an app keeps its place when it is switched
    // on or off.
    let mut all: Vec<(&st::AppInfo, bool)> =
        a.installed.iter().map(|i| (i, false)).chain(a.disabled.iter().map(|i| (i, true))).collect();
    all.sort_by(|x, y| x.0.name.cmp(&y.0.name));
    pb::AppsSnapshot {
        apps: all
            .into_iter()
            .map(|(i, disabled)| pb::App {
                name: i.name.clone(),
                id: i.id.clone(),
                display_name: if i.display_name.is_empty() { i.name.clone() } else { i.display_name.clone() },
                description: i.description.clone(),
                kind: app_kind(i.kind).into(),
                has_idle: i.has_idle,
                hidden: i.hidden,
                builtin: i.builtin,
                running: i.running,
                foreground: a.foreground.as_deref() == Some(i.name.as_str()),
                disabled,
            })
            .collect(),
        foreground: a.foreground.clone().unwrap_or_default(),
        idle_app: a.idle_app.clone().unwrap_or_default(),
    }
}

pub fn log_level(l: st::Level) -> pb::LogLevel {
    match l {
        st::Level::Error => pb::LogLevel::Error,
        st::Level::Warn => pb::LogLevel::Warn,
        st::Level::Info => pb::LogLevel::Info,
        st::Level::Debug => pb::LogLevel::Debug,
        st::Level::Trace => pb::LogLevel::Trace,
    }
}

pub fn log_entry(e: &st::LogEntry) -> pb::LogEntry {
    pb::LogEntry {
        seq: e.seq,
        unix_ms: e.unix_ms,
        mono_ms: e.mono_ms,
        level: log_level(e.level).into(),
        target: e.target.clone(),
        message: e.message.clone(),
    }
}

pub fn lamp_color(c: st::LampColor) -> pb::LampColor {
    match c {
        st::LampColor::Off => pb::LampColor::Off,
        st::LampColor::Red => pb::LampColor::Red,
        st::LampColor::Green => pb::LampColor::Green,
        st::LampColor::Amber => pb::LampColor::Amber,
    }
}

pub fn lamp_blink(b: st::Blink) -> pb::LampBlink {
    match b {
        st::Blink::Solid => pb::LampBlink::Solid,
        st::Blink::Slow => pb::LampBlink::Slow,
        st::Blink::Fast => pb::LampBlink::Fast,
        st::Blink::Wink => pb::LampBlink::Wink,
    }
}

pub fn key(k: st::Key) -> pb::Key {
    use pb::key::Kind as K;
    let (kind, index, module) = match k {
        st::Key::Digit(d) => (K::Digit, u32::from(d), 0),
        st::Key::Star => (K::Star, 0, 0),
        st::Key::Pound => (K::Pound, 0, 0),
        st::Key::Up => (K::Up, 0, 0),
        st::Key::Down => (K::Down, 0, 0),
        st::Key::Left => (K::Left, 0, 0),
        st::Key::Right => (K::Right, 0, 0),
        st::Key::Select => (K::Select, 0, 0),
        st::Key::Cancel => (K::Cancel, 0, 0),
        st::Key::VolumeUp => (K::VolumeUp, 0, 0),
        st::Key::VolumeDown => (K::VolumeDown, 0, 0),
        st::Key::Soft(n) => (K::Soft, u32::from(n), 0),
        st::Key::Line(n) => (K::Line, u32::from(n), 0),
        st::Key::Blf(n) => (K::Blf, u32::from(n), 0),
        st::Key::Mute => (K::Mute, 0, 0),
        st::Key::Speaker => (K::Speaker, 0, 0),
        st::Key::Headset => (K::Headset, 0, 0),
        st::Key::Hold => (K::Hold, 0, 0),
        st::Key::Transfer => (K::Transfer, 0, 0),
        st::Key::Redial => (K::Redial, 0, 0),
        st::Key::Messages => (K::Messages, 0, 0),
        st::Key::Conference => (K::Conference, 0, 0),
        st::Key::Info => (K::Info, 0, 0),
        st::Key::BlfPageLeft => (K::BlfPageLeft, 0, 0),
        st::Key::BlfPageRight => (K::BlfPageRight, 0, 0),
        st::Key::Status => (K::Status, 0, 0),
        st::Key::Contacts => (K::Contacts, 0, 0),
        st::Key::Apps => (K::Apps, 0, 0),
        st::Key::Menu => (K::Menu, 0, 0),
        st::Key::Expansion { module, key } => (K::Expansion, u32::from(key), u32::from(module)),
        st::Key::Unknown(code) => (K::Raw, u32::from(code), 0),
    };
    pb::Key { kind: kind.into(), index, module }
}

/// The key a message names; `None` for one that no phone has (an
/// unspecified kind, or an index out of range).  `index` and `module` are
/// ignored where the kind has no use for them.
pub fn key_from(k: &pb::Key) -> Option<st::Key> {
    use pb::key::Kind as K;
    let numbered = |max: u32| (1..=max).contains(&k.index).then_some(k.index as u8);
    Some(match k.kind.as_known()? {
        K::Unspecified => return None,
        K::Digit => st::Key::Digit((k.index <= 9).then_some(k.index as u8)?),
        K::Star => st::Key::Star,
        K::Pound => st::Key::Pound,
        K::Up => st::Key::Up,
        K::Down => st::Key::Down,
        K::Left => st::Key::Left,
        K::Right => st::Key::Right,
        K::Select => st::Key::Select,
        K::Cancel => st::Key::Cancel,
        K::VolumeUp => st::Key::VolumeUp,
        K::VolumeDown => st::Key::VolumeDown,
        K::Soft => st::Key::Soft(numbered(4)?),
        K::Line => st::Key::Line(numbered(6)?),
        K::Blf => st::Key::Blf(numbered(10)?),
        K::Mute => st::Key::Mute,
        K::Speaker => st::Key::Speaker,
        K::Headset => st::Key::Headset,
        K::Hold => st::Key::Hold,
        K::Transfer => st::Key::Transfer,
        K::Redial => st::Key::Redial,
        K::Messages => st::Key::Messages,
        K::Conference => st::Key::Conference,
        K::Info => st::Key::Info,
        K::BlfPageLeft => st::Key::BlfPageLeft,
        K::BlfPageRight => st::Key::BlfPageRight,
        K::Status => st::Key::Status,
        K::Contacts => st::Key::Contacts,
        K::Apps => st::Key::Apps,
        K::Menu => st::Key::Menu,
        K::Expansion => st::Key::Expansion { module: u8::try_from(k.module).ok()?, key: u8::try_from(k.index).ok()? },
        K::Raw => st::Key::Unknown(u16::try_from(k.index).ok()?),
    })
}

fn bluetooth_device(d: &st::BluetoothDevice) -> pb::BluetoothDevice {
    pb::BluetoothDevice { address: d.addr.clone(), name: d.name.clone(), rssi: d.rssi as i32 }
}

pub fn bluetooth(b: &st::Bluetooth) -> pb::BluetoothSnapshot {
    pb::BluetoothSnapshot {
        present: b.present,
        powered: b.powered,
        address: b.address.clone(),
        discovering: b.discovering,
        found: b.found.iter().map(bluetooth_device).collect(),
        paired: b.paired.iter().map(bluetooth_device).collect(),
        pairing_with: b.pairing_with.clone(),
        pairing: b
            .pairing
            .as_ref()
            .map(|p| pb::BluetoothPairing {
                address: p.addr.clone(),
                name: p.name.clone(),
                passkey: p.passkey,
                pin_needed: p.pin_needed,
            })
            .into(),
        connected: b.connected.clone(),
        audio_open: b.audio_open,
        error: b.error.clone(),
    }
}

fn page_group(g: &st::PageGroup) -> pb::PageGroup {
    pb::PageGroup { id: g.id.clone(), name: g.name.clone(), address: g.addr.clone(), port: g.port as u32 }
}

pub fn paging(p: &st::Paging) -> pb::PagingSnapshot {
    let (activity, group, name) = match &p.activity {
        st::PageActivity::Idle => (pb::PageActivity::PAGE_ACTIVITY_IDLE, "", ""),
        st::PageActivity::Receiving { group, name } => (pb::PageActivity::PAGE_ACTIVITY_RECEIVING, group.as_str(), name.as_str()),
        st::PageActivity::Blocked { group, name } => (pb::PageActivity::PAGE_ACTIVITY_BLOCKED, group.as_str(), name.as_str()),
        st::PageActivity::Sending { group, name } => (pb::PageActivity::PAGE_ACTIVITY_SENDING, group.as_str(), name.as_str()),
    };
    pb::PagingSnapshot {
        listeners: p.listeners.iter().map(page_group).collect(),
        broadcasts: p.broadcasts.iter().map(page_group).collect(),
        activity: activity.into(),
        active_group: group.to_owned(),
        active_name: name.to_owned(),
        error: p.error.clone(),
    }
}

pub fn alarm(a: &st::Alarm) -> pb::Alarm {
    pb::Alarm {
        id: a.id,
        hour: a.hour.into(),
        minute: a.minute.into(),
        days: a.days.into(),
        tone: a.tone.clone(),
        enabled: a.enabled,
    }
}

pub fn alarms(a: &st::Alarms) -> pb::AlarmsSnapshot {
    let time = |t: Option<(u32, u64)>| t.map(|(id, unix_secs)| pb::AlarmTime { id, unix_secs }).into();
    pb::AlarmsSnapshot {
        alarms: a.alarms.iter().map(alarm).collect(),
        tones: a.tones.clone(),
        next: time(a.next),
        ringing: a.ringing.as_ref().map(|r| r.id),
        snoozed: time(a.snoozed),
        snooze_secs: a.snooze_secs,
        max_alarms: st::Alarms::MAX as u32,
    }
}

pub fn expansion(e: &st::Expansion) -> pb::ExpansionSnapshot {
    pb::ExpansionSnapshot {
        enabled: e.enabled,
        modules: e
            .modules
            .iter()
            .map(|m| pb::ExpansionModule {
                node: m.node as u32,
                address: m.addr.clone(),
                version: m.version.clone(),
                page: m.page as u32,
            })
            .collect(),
        error: e.error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        let keys = [
            st::Key::Digit(0), st::Key::Digit(9), st::Key::Star, st::Key::Pound, st::Key::Up, st::Key::Down,
            st::Key::Left, st::Key::Right, st::Key::Select, st::Key::Cancel, st::Key::VolumeUp,
            st::Key::VolumeDown, st::Key::Soft(1), st::Key::Soft(4), st::Key::Line(6), st::Key::Blf(10),
            st::Key::Mute, st::Key::Speaker, st::Key::Headset, st::Key::Hold, st::Key::Transfer,
            st::Key::Redial, st::Key::Messages, st::Key::Conference, st::Key::Info, st::Key::BlfPageLeft,
            st::Key::BlfPageRight, st::Key::Status, st::Key::Contacts, st::Key::Apps, st::Key::Menu,
            st::Key::Expansion { module: 1, key: 17 }, st::Key::Unknown(700),
        ];
        for k in keys {
            assert_eq!(key_from(&key(k)), Some(k), "{k:?}");
        }
        let kind = |kind: pb::key::Kind, index: u32| pb::Key { kind: kind.into(), index, module: 0 };
        for bad in [
            pb::Key::default(),
            kind(pb::key::Kind::KIND_DIGIT, 10),
            kind(pb::key::Kind::KIND_SOFT, 0),
            kind(pb::key::Kind::KIND_SOFT, 5),
            kind(pb::key::Kind::KIND_LINE, 7),
            kind(pb::key::Kind::KIND_BLF, 11),
            kind(pb::key::Kind::KIND_EXPANSION, 256),
            kind(pb::key::Kind::KIND_RAW, 65536),
        ] {
            assert_eq!(key_from(&bad), None, "{bad:?}");
        }
    }
}
