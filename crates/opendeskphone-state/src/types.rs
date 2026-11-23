//! The typed state.  One struct per [`Topic`]; each is owned (written) by
//! one subsystem and read by any.
//!
//! Conventions:
//! - `*_mono_ms` fields are [`crate::clock::mono_ms`] stamps; `*_unix` fields
//!   are wall-clock seconds and may be wrong until the clock is set.
//! - Secrets (SIP passwords, the web password) are never in the state.
//! - All types are `Clone + PartialEq + Serialize` so a snapshot can be
//!   compared, dumped as JSON, and streamed.

use crate::{FirmwareVersion, Model};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Arc;

/// The topics of the store.  Subscriptions select by topic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Topic {
    System = 0,
    Network,
    Provisioning,
    Accounts,
    Keys,
    Calls,
    Audio,
    Presence,
    Mwi,
    CallLog,
    Features,
    Contacts,
    Config,
    Update,
    Apps,
    Health,
    Ui,
    /// The log ring got new entries (coalesced).
    Log,
    Paging,
    Expansion,
    Bluetooth,
    Alarms,
}

impl Topic {
    pub const ALL: [Topic; 22] = [
        Topic::System,
        Topic::Network,
        Topic::Provisioning,
        Topic::Accounts,
        Topic::Keys,
        Topic::Calls,
        Topic::Audio,
        Topic::Presence,
        Topic::Mwi,
        Topic::CallLog,
        Topic::Features,
        Topic::Contacts,
        Topic::Config,
        Topic::Update,
        Topic::Apps,
        Topic::Health,
        Topic::Ui,
        Topic::Log,
        Topic::Paging,
        Topic::Expansion,
        Topic::Bluetooth,
        Topic::Alarms,
    ];
}

/// A set of topics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopicSet(pub u32);

impl TopicSet {
    pub const NONE: TopicSet = TopicSet(0);
    /// Every topic, including [`Topic::Log`].
    pub const ALL: TopicSet = TopicSet((1 << Topic::ALL.len()) - 1);

    pub const fn of(t: Topic) -> Self {
        TopicSet(1 << t as u8)
    }
    pub const fn with(self, t: Topic) -> Self {
        TopicSet(self.0 | 1 << t as u8)
    }
    pub const fn without(self, t: Topic) -> Self {
        TopicSet(self.0 & !(1 << t as u8))
    }
    pub const fn contains(self, t: Topic) -> bool {
        self.0 & (1 << t as u8) != 0
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub const fn union(self, o: TopicSet) -> Self {
        TopicSet(self.0 | o.0)
    }
    pub const fn intersect(self, o: TopicSet) -> Self {
        TopicSet(self.0 & o.0)
    }
    pub fn iter(self) -> impl Iterator<Item = Topic> {
        Topic::ALL.into_iter().filter(move |t| self.contains(*t))
    }
}

impl From<Topic> for TopicSet {
    fn from(t: Topic) -> Self {
        TopicSet::of(t)
    }
}
impl<const N: usize> From<[Topic; N]> for TopicSet {
    fn from(ts: [Topic; N]) -> Self {
        ts.into_iter().fold(TopicSet::NONE, TopicSet::with)
    }
}
impl FromIterator<Topic> for TopicSet {
    fn from_iter<I: IntoIterator<Item = Topic>>(it: I) -> Self {
        it.into_iter().fold(TopicSet::NONE, TopicSet::with)
    }
}

/// A complete snapshot.  Cloning is cheap (one `Arc` per topic).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct State {
    pub system: Arc<System>,
    pub network: Arc<Network>,
    pub provisioning: Arc<Provisioning>,
    pub accounts: Arc<Accounts>,
    pub keys: Arc<Keys>,
    pub calls: Arc<Calls>,
    pub audio: Arc<Audio>,
    pub presence: Arc<Presence>,
    pub mwi: Arc<Mwi>,
    pub call_log: Arc<CallLog>,
    pub features: Arc<Features>,
    pub contacts: Arc<Contacts>,
    pub config: Arc<ConfigInfo>,
    pub update: Arc<Update>,
    pub apps: Arc<Apps>,
    pub health: Arc<Health>,
    pub ui: Arc<Ui>,
    pub paging: Arc<Paging>,
    pub expansion: Arc<Expansion>,
    pub bluetooth: Arc<Bluetooth>,
    pub alarms: Arc<Alarms>,
}

impl State {
    pub fn new(model: Model) -> Self {
        State {
            system: Arc::new(System::new(model)),
            network: Default::default(),
            provisioning: Default::default(),
            accounts: Default::default(),
            keys: Default::default(),
            calls: Default::default(),
            audio: Default::default(),
            presence: Default::default(),
            mwi: Default::default(),
            call_log: Default::default(),
            features: Default::default(),
            contacts: Default::default(),
            config: Default::default(),
            update: Default::default(),
            apps: Default::default(),
            health: Default::default(),
            ui: Default::default(),
            paging: Default::default(),
            expansion: Default::default(),
            bluetooth: Default::default(),
            alarms: Default::default(),
        }
    }
}

/// Implemented by each per-topic struct; lets [`crate::Store::get`] and
/// [`crate::Store::update`] be generic.
pub trait TopicState: Clone + PartialEq + Send + Sync + 'static {
    const TOPIC: Topic;
    #[doc(hidden)]
    fn slot(s: &State) -> &Arc<Self>;
    #[doc(hidden)]
    fn slot_mut(s: &mut State) -> &mut Arc<Self>;
}

macro_rules! topic_state {
    ($ty:ty, $topic:ident, $field:ident) => {
        impl TopicState for $ty {
            const TOPIC: Topic = Topic::$topic;
            fn slot(s: &State) -> &Arc<Self> {
                &s.$field
            }
            fn slot_mut(s: &mut State) -> &mut Arc<Self> {
                &mut s.$field
            }
        }
    };
}

topic_state!(System, System, system);
topic_state!(Network, Network, network);
topic_state!(Provisioning, Provisioning, provisioning);
topic_state!(Accounts, Accounts, accounts);
topic_state!(Keys, Keys, keys);
topic_state!(Calls, Calls, calls);
topic_state!(Audio, Audio, audio);
topic_state!(Presence, Presence, presence);
topic_state!(Mwi, Mwi, mwi);
topic_state!(CallLog, CallLog, call_log);
topic_state!(Features, Features, features);
topic_state!(Contacts, Contacts, contacts);
topic_state!(ConfigInfo, Config, config);
topic_state!(Update, Update, update);
topic_state!(Apps, Apps, apps);
topic_state!(Health, Health, health);
topic_state!(Ui, Ui, ui);
topic_state!(Paging, Paging, paging);
topic_state!(Expansion, Expansion, expansion);
topic_state!(Bluetooth, Bluetooth, bluetooth);
topic_state!(Alarms, Alarms, alarms);

// ---------------------------------------------------------------- System

/// Where the main role is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Starting,
    /// Waiting for link/address.
    NetworkWait,
    /// Running the provisioning state machine before accounts exist.
    Provisioning,
    /// Normal operation.
    Running,
    /// nvdata failed to mount or is unusable: recovery screen and web page.
    Recovery,
    /// Reboot or shutdown under way.
    ShuttingDown,
}

/// Identity and whole-phone status.  Owner: the `dphone` main role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct System {
    pub model: Model,
    pub version: FirmwareVersion,
    pub phase: Phase,
    pub mac: Option<[u8; 6]>,
    pub hostname: String,
    /// The wall clock has been set by SNTP (not merely restored).
    pub time_synced: bool,
    /// IANA zone or POSIX TZ string from config, for display.
    pub timezone: String,
    /// Text from `/nvdata/reboot_reason` for the previous boot.
    pub last_reboot_reason: String,
    /// Running in the workstation simulator.
    pub simulated: bool,
    /// `digium_boot_rev=` from the command line.
    pub boot_rev: Option<u32>,
    /// Usable RAM in KiB, from `/proc/meminfo`.
    pub mem_total_kib: u32,
}

impl System {
    pub fn new(model: Model) -> Self {
        System {
            model,
            version: FirmwareVersion::CURRENT,
            phase: Phase::Starting,
            mac: None,
            hostname: String::new(),
            time_synced: false,
            timezone: String::new(),
            last_reboot_reason: String::new(),
            simulated: false,
            boot_rev: None,
            mem_total_kib: 0,
        }
    }

    /// `aa:bb:cc:dd:ee:ff`, or empty when unknown.
    pub fn mac_string(&self) -> String {
        match self.mac {
            Some(m) => format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            ),
            None => String::new(),
        }
    }
}

// --------------------------------------------------------------- Network

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NetPhase {
    #[default]
    Down,
    /// Link is up; LLDP/VLAN discovery or DHCP in progress.
    Configuring,
    Up,
    Failed,
}

/// One switch port.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PortLink {
    pub up: bool,
    /// Mb/s; 0 when down.
    pub speed: u16,
    pub full_duplex: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AddrSource {
    #[default]
    None,
    Dhcp,
    Static,
    Slaac,
    Dhcp6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VlanSource {
    #[default]
    None,
    Config,
    Lldp,
    Dhcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Ipv4State {
    pub source: AddrSource,
    pub address: Option<std::net::Ipv4Addr>,
    pub prefix_len: u8,
    pub gateway: Option<std::net::Ipv4Addr>,
    /// Seconds of DHCP lease, and when it was obtained.
    pub lease_secs: u32,
    pub lease_obtained_mono_ms: u64,
    pub dhcp_server: Option<std::net::Ipv4Addr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ipv6Address {
    pub address: std::net::Ipv6Addr,
    pub prefix_len: u8,
    pub source: AddrSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VlanState {
    pub source: VlanSource,
    /// Voice VLAN the phone itself is on (`None`: untagged).
    pub voice_id: Option<u16>,
    pub voice_priority: u8,
    pub dscp: u8,
    /// VLAN applied to the PC port, if any.
    pub pc_id: Option<u16>,
}

/// What the LLDP neighbor on the LAN port announced.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LldpNeighbor {
    pub chassis_id: String,
    pub port_id: String,
    pub system_name: String,
    pub port_description: String,
}

/// Network status.  Owner: `opendeskphone-net`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Network {
    pub phase: NetPhase,
    /// Why `phase` is `Failed`, or other detail for the status page.
    pub detail: String,
    pub lan: PortLink,
    pub pc: PortLink,
    /// The PC port is administratively enabled.
    pub pc_port_enabled: bool,
    pub vlan: VlanState,
    pub ipv4: Ipv4State,
    pub ipv6: Vec<Ipv6Address>,
    pub dns: Vec<IpAddr>,
    pub domain: String,
    /// DHCP option 66 / 150 as received (provisioning discovery reads it).
    pub dhcp_tftp_server: String,
    /// DHCP option 42 or configured NTP servers in use.
    pub ntp_servers: Vec<String>,
    /// Last successful SNTP exchange.
    pub ntp_synced_mono_ms: Option<u64>,
    pub lldp: Option<LldpNeighbor>,
    /// Name of the selected network profile (`netselect.xml`).
    pub profile: String,
}

impl Network {
    pub fn is_up(&self) -> bool {
        self.phase == NetPhase::Up
    }
}

// ---------------------------------------------------------- Provisioning

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoverySource {
    Saved,
    DhcpOption66,
    Mdns,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredServer {
    pub source: DiscoverySource,
    /// Display name (mDNS instance name, or the address).
    pub name: String,
    /// `host:port` with optional transport, as dialed.
    pub address: String,
}

/// A selectable identity from the DPMA user list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionUser {
    pub id: String,
    pub name: String,
    pub needs_pin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProvisionPhase {
    /// Not started (no network yet).
    #[default]
    Idle,
    Discovering,
    /// No server saved or named yet: waiting for the user to pick one of
    /// the servers found or to enter one (`Command::StartProvisioning`).
    AwaitingServer,
    Handshaking,
    /// Waiting for `Command::SelectUser`.
    AwaitingUser,
    FetchingConfig,
    Applying,
    /// Configured by DPMA and attached for pushes.
    Provisioned,
    /// Running on locally configured SIP accounts, with no DPMA server.
    Standalone,
    Failed {
        /// What went wrong, for the phone's user: a sentence, with what
        /// to do about it.
        reason: String,
        /// The stock error code, when the server gave one.
        code: Option<i32>,
        /// What went wrong in technical terms, for an administrator
        /// (may be empty).
        detail: String,
    },
}

/// DPMA provisioning status.  Owner: the DPMA client.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Provisioning {
    pub phase: ProvisionPhase,
    pub discovered: Vec<DiscoveredServer>,
    /// The server in use.
    pub server: Option<DiscoveredServer>,
    /// `Server:`/`User-Agent` the PBX reported (stock keeps it in
    /// `/nvdata/server-user-agent`).
    pub server_user_agent: String,
    pub users: Vec<ProvisionUser>,
    /// The identity this phone is assigned (user or MAC-keyed config).
    pub assigned_user: String,
    /// The session is up and pushes can arrive.
    pub attached: bool,
    /// MD5 of the current configuration (`/nvdata/md5s/config`).
    pub config_md5: String,
    /// The server offered firmware; stored, not acted on unless enabled.
    pub offered_firmware: Option<String>,
    pub last_success_unix: Option<u64>,
    /// How the last `Command::SelectUser` ended: `Ok`, or what the PBX or
    /// the phone said against it.  `None` before the first, and while one
    /// is under way.
    pub selection: Option<Result<(), String>>,
}

// -------------------------------------------------------------- Accounts

/// Index of a SIP account: 0-based, below `Model::max_accounts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Transport {
    #[default]
    Udp,
    Tcp,
    Tls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ActiveHost {
    #[default]
    Primary,
    Alternate,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Registration {
    /// The account is configured off, or has no registrar.
    Disabled,
    #[default]
    Unregistered,
    Registering,
    Registered {
        host: ActiveHost,
        /// When the binding expires.
        expires_mono_ms: u64,
    },
    Failed {
        /// SIP status, or 0 for transport failures and timeouts.
        code: u16,
        reason: String,
        /// When the next attempt is due.
        retry_mono_ms: u64,
    },
}

impl Registration {
    pub fn is_registered(&self) -> bool {
        matches!(self, Registration::Registered { .. })
    }
}

/// One configured SIP account and its registration.  Owner:
/// `opendeskphone-sip`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    /// Stock `account id` attribute from the configuration.
    pub config_id: String,
    /// Text on the line key.
    pub label: String,
    pub display_name: String,
    /// SIP user part.
    pub user: String,
    pub auth_user: String,
    pub server: String,
    pub port: u16,
    pub transport: Transport,
    pub alternate_server: String,
    pub outbound_proxy: String,
    pub voicemail_number: String,
    /// This account carries DPMA (out-of-dialog MESSAGE to the PBX).
    pub dpma: bool,
    pub srtp: bool,
    pub registration: Registration,
}

impl Account {
    pub fn new(id: AccountId) -> Self {
        Account {
            id,
            config_id: String::new(),
            label: String::new(),
            display_name: String::new(),
            user: String::new(),
            auth_user: String::new(),
            server: String::new(),
            port: 5060,
            transport: Transport::Udp,
            alternate_server: String::new(),
            outbound_proxy: String::new(),
            voicemail_number: String::new(),
            dpma: false,
            srtp: false,
            registration: Registration::Unregistered,
        }
    }

    /// `sip:user@server` (address of record).
    pub fn aor(&self) -> String {
        format!("sip:{}@{}", self.user, self.server)
    }
}

/// All accounts.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Accounts {
    pub accounts: Vec<Account>,
    /// Account used when a call is placed without choosing a line.
    pub default_account: Option<AccountId>,
}

impl Accounts {
    pub fn get(&self, id: AccountId) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }
    pub fn get_mut(&mut self, id: AccountId) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }
    pub fn any_registered(&self) -> bool {
        self.accounts.iter().any(|a| a.registration.is_registered())
    }
}

// ------------------------------------------------------------------ Keys

/// Lamp colors.  Amber is red plus green.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum LampColor {
    #[default]
    Off,
    Red,
    Green,
    Amber,
}

/// Blink patterns (timing is done in userspace by the HAL helper).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum Blink {
    #[default]
    Solid,
    /// About 1 Hz (stock: 501 ms on, 501 ms off): held calls.
    Slow,
    /// About 2 Hz (stock: 251 ms on, 251 ms off): ringing.
    Fast,
    /// Short flash every few seconds: e.g., message waiting.
    Wink,
}

/// What a lamp should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub struct LampState {
    pub color: LampColor,
    pub blink: Blink,
}

impl LampState {
    pub const OFF: LampState = LampState { color: LampColor::Off, blink: Blink::Solid };
    pub const fn solid(color: LampColor) -> Self {
        LampState { color, blink: Blink::Solid }
    }
    pub const fn blinking(color: LampColor, blink: Blink) -> Self {
        LampState { color, blink }
    }
}

/// The speaker, headset, and mute lamps, in that order, as stock's `phone`
/// drives them: the speaker or headset lamp is solid green while that path
/// carries a call, dialing, ringback, or the tone of a failed call, and off
/// when the phone is idle or only ringing (even though the ringer uses the
/// speaker); headset mode alone lights nothing.  The mute lamp is solid red
/// while the microphone is muted.  None of them blinks.
pub fn audio_lamps(audio: &Audio, calls: &Calls) -> (LampState, LampState, LampState) {
    // The path is open for the ringer only: nothing but alerting calls,
    // and no tone of ours.
    let ringer_only =
        audio.ringing && audio.tone.is_none() && calls.calls.iter().all(|c| c.state == CallState::Ringing);
    let lit = |path: AudioPath| {
        if audio.path == path && !ringer_only { LampState::solid(LampColor::Green) } else { LampState::OFF }
    };
    let mute = if audio.muted { LampState::solid(LampColor::Red) } else { LampState::OFF };
    (lit(AudioPath::Speaker), lit(AudioPath::Headset), mute)
}

/// What a line key (beside the screen) is bound to.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineKeyBinding {
    #[default]
    Unused,
    Account(AccountId),
    /// A BLF/rapid-dial entry (unused line keys can serve as BLF); the value
    /// is [`BlfEntry::id`].
    Blf(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LineKey {
    /// 0-based position from the top.
    pub index: u8,
    pub binding: LineKeyBinding,
    pub label: String,
    pub lamp: LampState,
    /// Calls appearing on this key, most relevant first.
    pub calls: Vec<CallId>,
}

/// One rapid-dial position (D50/D70 keys, the D70 side display, D65 pages,
/// expansion modules).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RapidDialKey {
    /// Position within the page, 0-based.
    pub index: u8,
    pub page: u8,
    /// [`BlfEntry::id`], or empty for an empty slot.
    pub entry: String,
    pub label: String,
    pub number: String,
    pub lamp: LampState,
}

/// Line keys and rapid-dial keys.  Owner: the call-control core (it derives
/// these from accounts, calls, presence, and configuration); the UI draws
/// them and the lamp driver follows `lamp`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Keys {
    pub line_keys: Vec<LineKey>,
    pub rapid_dial: Vec<RapidDialKey>,
    /// Rapid-dial page shown now, and the number of pages in use.
    pub rapid_page: u8,
    pub rapid_pages: u8,
    /// Lamps not tied to a line or BLF key.
    pub message_lamp: LampState,
    pub mute_lamp: LampState,
    pub headset_lamp: LampState,
    pub speaker_lamp: LampState,
}

// ----------------------------------------------------------------- Calls

/// Identifies a call for its whole life; never reused within a boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CallId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Incoming,
    Outgoing,
}

/// Why a call ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndCause {
    LocalHangup,
    RemoteHangup,
    /// We declined an incoming call.
    Rejected,
    /// The caller gave up, or the call was answered elsewhere.
    Canceled { answered_elsewhere: bool },
    Busy,
    NoAnswer,
    /// Completed transfer.
    Transferred,
    /// SIP failure response.
    Failed { code: u16, reason: String },
    /// Transport or media failure.
    Error(String),
}

/// The call state machine.
///
/// ```text
/// outgoing: Dialing -> Proceeding -> Ringback -> Active
/// incoming: Ringing -> Active
/// Active <-> Held;  Active/Held -> Transferring -> Ended
/// any -> Ended
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    /// Outgoing: INVITE not yet sent or no response yet.
    Dialing,
    /// Outgoing: 100 Trying received.
    Proceeding,
    /// Outgoing: 180/183.  With `early_media` the far end's audio plays
    /// instead of local ringback.
    Ringback { early_media: bool },
    /// Incoming and alerting.
    Ringing,
    Active,
    /// On hold.  Either side, or both, may hold.
    Held { local: bool, remote: bool },
    /// A transfer of this call is in progress (REFER sent).
    Transferring,
    /// Finished; the entry lingers briefly so screens can show the cause,
    /// then is removed.
    Ended(EndCause),
}

impl CallState {
    pub fn is_ended(&self) -> bool {
        matches!(self, CallState::Ended(_))
    }
    /// Media is (or may be) flowing: active or early media.
    pub fn has_media(&self) -> bool {
        matches!(self, CallState::Active | CallState::Ringback { early_media: true })
    }
    pub fn is_held(&self) -> bool {
        matches!(self, CallState::Held { .. })
    }
    /// Answered at some point (connected), as opposed to still setting up.
    pub fn is_established(&self) -> bool {
        matches!(self, CallState::Active | CallState::Held { .. } | CallState::Transferring)
    }
}

/// The other party.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Party {
    pub name: String,
    /// User part / dialed digits.
    pub number: String,
    /// Full SIP URI when known.
    pub uri: String,
}

impl Party {
    /// The name if present, else the number.
    pub fn display(&self) -> &str {
        if self.name.is_empty() { &self.number } else { &self.name }
    }
}

/// Voice codecs (plan section 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codec {
    Pcmu,
    Pcma,
    G722,
    G726_32,
    G729,
    Ilbc,
    Opus,
    L16,
    L16Wide,
}

impl Codec {
    pub const fn name(self) -> &'static str {
        match self {
            Codec::Pcmu => "PCMU",
            Codec::Pcma => "PCMA",
            Codec::G722 => "G722",
            Codec::G726_32 => "G726-32",
            Codec::G729 => "G729",
            Codec::Ilbc => "iLBC",
            Codec::Opus => "opus",
            Codec::L16 => "L16",
            Codec::L16Wide => "L16-256",
        }
    }
}

/// Receive-side media quality, updated a few times per second at most.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaStats {
    pub rx_packets: u32,
    pub tx_packets: u32,
    pub rx_lost: u32,
    pub jitter_ms: u16,
}

/// One call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub id: CallId,
    pub account: AccountId,
    pub direction: Direction,
    pub state: CallState,
    pub remote: Party,
    /// Created (dialed or first seen).
    pub started_mono_ms: u64,
    pub started_unix: u64,
    /// Answered; the call timer counts from here.
    pub connected_mono_ms: Option<u64>,
    pub codec: Option<Codec>,
    pub srtp: bool,
    /// Microphone muted for this call.
    pub muted: bool,
    /// Member of the local three-way conference.
    pub in_conference: bool,
    /// The consultation call of an attended transfer points at the call
    /// being transferred.
    pub transfer_of: Option<CallId>,
    /// Paging or intercom: auto-answered, one-way where so marked.
    pub auto_answer: bool,
    pub paging: bool,
    /// A call that belongs to an app (stock's "hidden call", e.g., the
    /// PBX calling back to play a voicemail message): it does not ring,
    /// has no call screen or line lamp, and is not logged.
    #[serde(default)]
    pub hidden: bool,
    /// SIP Call-ID, for logs and diagnostics.
    pub sip_call_id: String,
    pub stats: MediaStats,
}

impl Call {
    pub fn new(id: CallId, account: AccountId, direction: Direction, remote: Party) -> Self {
        Call {
            id,
            account,
            direction,
            state: match direction {
                Direction::Incoming => CallState::Ringing,
                Direction::Outgoing => CallState::Dialing,
            },
            remote,
            started_mono_ms: crate::clock::mono_ms(),
            started_unix: crate::clock::unix_secs(),
            connected_mono_ms: None,
            codec: None,
            srtp: false,
            muted: false,
            in_conference: false,
            transfer_of: None,
            auto_answer: false,
            paging: false,
            hidden: false,
            sip_call_id: String::new(),
            stats: MediaStats::default(),
        }
    }

    /// Seconds since answer, for the call timer.
    pub fn duration_secs(&self, now_mono_ms: u64) -> Option<u64> {
        self.connected_mono_ms.map(|c| now_mono_ms.saturating_sub(c) / 1000)
    }
}

/// All calls.  Owner: `opendeskphone-sip` (call control); media fields
/// (`codec`, `stats`) are filled by it from `opendeskphone-media` reports.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Calls {
    pub calls: Vec<Call>,
    /// The call the user is talking on or looking at; key presses such as
    /// Hold apply to it.
    pub focused: Option<CallId>,
}

impl Calls {
    pub fn get(&self, id: CallId) -> Option<&Call> {
        self.calls.iter().find(|c| c.id == id)
    }
    pub fn get_mut(&mut self, id: CallId) -> Option<&mut Call> {
        self.calls.iter_mut().find(|c| c.id == id)
    }
    pub fn focused(&self) -> Option<&Call> {
        self.focused.and_then(|id| self.get(id))
    }
    /// Calls that are not ended.
    pub fn live(&self) -> impl Iterator<Item = &Call> {
        self.calls.iter().filter(|c| !c.state.is_ended())
    }
    /// Calls alerting the user (an app's hidden call does not).
    pub fn ringing(&self) -> impl Iterator<Item = &Call> {
        self.calls.iter().filter(|c| c.state == CallState::Ringing && !c.hidden)
    }
    /// Live calls the user sees (all but apps' hidden calls).
    pub fn visible(&self) -> impl Iterator<Item = &Call> {
        self.live().filter(|c| !c.hidden)
    }
    pub fn is_idle(&self) -> bool {
        self.live().next().is_none()
    }
}

// ----------------------------------------------------------------- Audio

/// Where call audio goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum AudioPath {
    /// No path open (idle).
    #[default]
    None,
    Handset,
    Headset,
    Speaker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HeadsetKind {
    /// Plain wired headset on the RJ9 jack.
    #[default]
    Wired,
    /// Electronic hook switch adapter on the serial port.
    Ehs,
    Bluetooth,
}

/// Local tones the ARM generates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    Dial,
    Ringback,
    Busy,
    Reorder,
    CallWaiting,
    /// Confirmation beep.
    Confirm,
    /// Key feedback for a DTMF digit.
    Dtmf(char),
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DspStatus {
    #[default]
    NotStarted,
    Starting,
    Running,
    Failed(String),
}

/// What a volume setting applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VolumeTarget {
    Handset,
    Headset,
    Speaker,
    Ringer,
    /// Whatever is sounding now: the active path in a call, else the ringer.
    Current,
}

/// Volume steps run 0..=`VOLUME_MAX`.
pub const VOLUME_MAX: u8 = 10;

/// Audio routing and levels.  Owner: `opendeskphone-media`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    pub path: AudioPath,
    /// Handset lifted.
    pub off_hook: bool,
    /// Headset mode is latched on (calls go to the headset by default).
    pub headset_mode: bool,
    pub headset_kind: HeadsetKind,
    /// Microphone muted (applies to the current path).
    pub muted: bool,
    pub handset_volume: u8,
    pub headset_volume: u8,
    pub speaker_volume: u8,
    pub ringer_volume: u8,
    /// The ringer is sounding.
    pub ringing: bool,
    pub tone: Option<Tone>,
    pub dsp: DspStatus,
}

impl Default for Audio {
    fn default() -> Self {
        Audio {
            path: AudioPath::None,
            off_hook: false,
            headset_mode: false,
            headset_kind: HeadsetKind::Wired,
            muted: false,
            handset_volume: 5,
            headset_volume: 5,
            speaker_volume: 5,
            ringer_volume: 5,
            ringing: false,
            tone: None,
            dsp: DspStatus::NotStarted,
        }
    }
}

impl Audio {
    /// The volume the volume keys adjust now (never `Current`): that of
    /// the path that carries a call, dialing, or a tone, and the ringer's
    /// when the phone is idle or the ringer is what sounds (the ringer
    /// uses the speaker).
    pub fn current_target(&self) -> VolumeTarget {
        match self.path {
            AudioPath::Handset => VolumeTarget::Handset,
            AudioPath::Headset => VolumeTarget::Headset,
            AudioPath::Speaker if !self.ringing => VolumeTarget::Speaker,
            AudioPath::Speaker | AudioPath::None => VolumeTarget::Ringer,
        }
    }

    pub fn volume(&self, target: VolumeTarget) -> u8 {
        match target {
            VolumeTarget::Handset => self.handset_volume,
            VolumeTarget::Headset => self.headset_volume,
            VolumeTarget::Speaker => self.speaker_volume,
            VolumeTarget::Ringer => self.ringer_volume,
            VolumeTarget::Current => self.volume(self.current_target()),
        }
    }
}

// -------------------------------------------------------------- Presence

/// Line state of a watched contact, from its `presence` subscription:
/// the states stock's `core` reduces a presence document to, and one more
/// for a contact whose device is not reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum BlfState {
    /// No NOTIFY yet, the subscription failed, or the document said
    /// nothing we know.
    #[default]
    Unknown,
    /// Stock's "Ready".
    Idle,
    Ringing,
    /// Stock's "On the phone".
    InUse,
    OnHold,
    /// A call on hold and another one ringing.
    OnHoldRinging,
    /// The document is closed: no device is registered.  Stock has no such
    /// state (it shows these contacts as unknown).
    Unavailable,
}

impl BlfState {
    /// The lamp of a BLF or rapid-dial key, by stock's default behavior
    /// table (the same for the expansion modules): nothing for an idle,
    /// unknown, or unavailable contact, fast green while it rings, solid
    /// green while it is on the phone, slow red while it holds a call, and
    /// fast red while it holds one call and another rings.
    pub fn lamp(self) -> LampState {
        match self {
            BlfState::Ringing => LampState::blinking(LampColor::Green, Blink::Fast),
            BlfState::InUse => LampState::solid(LampColor::Green),
            BlfState::OnHold => LampState::blinking(LampColor::Red, Blink::Slow),
            BlfState::OnHoldRinging => LampState::blinking(LampColor::Red, Blink::Fast),
            BlfState::Idle | BlfState::Unknown | BlfState::Unavailable => LampState::OFF,
        }
    }
}

/// User presence (the `digium_presence` element of a presence document,
/// and the Status app).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum PresenceStatus {
    #[default]
    Unknown,
    Available,
    Chat,
    Away,
    Dnd,
    ExtendedAway,
    Unavailable,
}

impl PresenceStatus {
    /// The token used on the wire and in the stock app API.
    pub const fn token(self) -> &'static str {
        match self {
            PresenceStatus::Unknown => "",
            PresenceStatus::Available => "available",
            PresenceStatus::Chat => "chat",
            PresenceStatus::Away => "away",
            PresenceStatus::Dnd => "dnd",
            PresenceStatus::ExtendedAway => "xa",
            PresenceStatus::Unavailable => "unavailable",
        }
    }
    pub fn from_token(s: &str) -> PresenceStatus {
        match s.trim().to_ascii_lowercase().as_str() {
            "available" => PresenceStatus::Available,
            "chat" => PresenceStatus::Chat,
            "away" => PresenceStatus::Away,
            "dnd" => PresenceStatus::Dnd,
            "xa" => PresenceStatus::ExtendedAway,
            "unavailable" => PresenceStatus::Unavailable,
            _ => PresenceStatus::Unknown,
        }
    }
}

/// One watched contact.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlfEntry {
    /// Stable identifier: the contact ID from the configuration.
    pub id: String,
    pub label: String,
    /// What pressing the key dials.
    pub number: String,
    /// Subscription target URI.
    pub uri: String,
    pub account: Option<AccountId>,
    pub state: BlfState,
    pub status: PresenceStatus,
    /// The user's free-text message from the supplement.
    pub status_text: String,
    /// A press while the contact rings picks the call up instead of
    /// dialing the contact (stock `can_pickup_ringing`).
    pub pickup_ringing: bool,
    /// The subscription is established.
    pub subscribed: bool,
}

/// BLF and presence.  Owner: `opendeskphone-sip`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Presence {
    pub entries: Vec<BlfEntry>,
    /// This user's own published status.
    pub own_status: PresenceStatus,
    pub own_status_text: String,
}

impl Presence {
    pub fn get(&self, id: &str) -> Option<&BlfEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

// ------------------------------------------------------------------- MWI

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MwiAccount {
    pub account: AccountId,
    pub waiting: bool,
    pub new: u32,
    pub old: u32,
    pub urgent_new: u32,
    pub urgent_old: u32,
}

/// Message waiting per account.  Owner: `opendeskphone-sip`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Mwi {
    pub accounts: Vec<MwiAccount>,
}

impl Mwi {
    pub fn any_waiting(&self) -> bool {
        self.accounts.iter().any(|a| a.waiting || a.new > 0)
    }
    pub fn total_new(&self) -> u32 {
        self.accounts.iter().map(|a| a.new).sum()
    }
}

// -------------------------------------------------------------- Call log

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallLogKind {
    Missed,
    Received,
    Placed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallLogEntry {
    /// Increases with each entry.
    pub id: u64,
    pub kind: CallLogKind,
    pub party: Party,
    pub account: AccountId,
    pub start_unix: u64,
    pub duration_secs: u32,
}

/// Recent calls, newest first.  Owner: the call-control core.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CallLog {
    pub entries: VecDeque<CallLogEntry>,
    /// Missed calls not yet looked at.
    pub missed_unseen: u32,
}

impl CallLog {
    /// Entries kept (stock keeps a bounded list too).
    pub const CAPACITY: usize = 100;

    /// Adds an entry at the front, assigning its ID and trimming the tail.
    pub fn push(&mut self, mut e: CallLogEntry) {
        e.id = self.entries.front().map(|f| f.id + 1).unwrap_or(1);
        if e.kind == CallLogKind::Missed {
            self.missed_unseen += 1;
        }
        self.entries.push_front(e);
        self.entries.truncate(Self::CAPACITY);
    }
}

// -------------------------------------------------------------- Features

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Forward {
    /// Forward every call here (empty: off).
    pub always: String,
    pub busy: String,
    pub no_answer: String,
    pub no_answer_secs: u16,
}

impl Forward {
    pub fn any(&self) -> bool {
        !self.always.is_empty() || !self.busy.is_empty() || !self.no_answer.is_empty()
    }
}

/// Do-not-disturb and call forward.  Owner: the call-control core.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Features {
    pub dnd: bool,
    pub forward: Forward,
    pub auto_answer: bool,
    pub call_waiting: bool,
}

// -------------------------------------------------------------- Contacts

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContactList {
    pub id: String,
    pub name: String,
    pub count: u32,
    /// File name under `/nvdata/contact_lists/`, or empty for built-in.
    pub file: String,
}

/// Summary of the loaded contact lists (the contacts themselves are read
/// through `opendeskphone-config`).  Owner: the configuration subsystem.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Contacts {
    pub lists: Vec<ContactList>,
    pub total: u32,
    /// Bumped whenever any list changes.
    pub generation: u64,
}

// ---------------------------------------------------------------- Config

/// Signals configuration changes; the values live in `opendeskphone-config`.
/// Owner: the configuration subsystem.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigInfo {
    /// Bumped after every completed merge.
    pub generation: u64,
    pub last_merge_unix: u64,
    /// A setting changed that only takes effect after a restart.
    pub restart_needed: bool,
    /// The last merge failed; the previous configuration stays in force.
    pub error: String,
}

// ---------------------------------------------------------------- Update

/// Flash bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bank {
    /// `k0` + `root0` (`root=/dev/mtdblock6`).
    A,
    /// `k1` + `root1` (`root=/dev/mtdblock7`).
    B,
}

impl Bank {
    pub const fn other(self) -> Bank {
        match self {
            Bank::A => Bank::B,
            Bank::B => Bank::A,
        }
    }
    pub const fn index(self) -> u8 {
        match self {
            Bank::A => 0,
            Bank::B => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Receiving { percent: u8 },
    Checking,
    Erasing { percent: u8 },
    WritingKernel { percent: u8 },
    WritingRootfs { percent: u8 },
    Verifying { percent: u8 },
    Committing,
    /// Written and committed; a reboot starts the trial.
    Complete,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TrialBoot {
    /// Not in a trial.
    #[default]
    None,
    /// Booted a new image that has not yet been declared healthy.
    Pending { boots_left: u8 },
    /// The previous trial failed and the phone went back to the other bank.
    RolledBack { reason: String },
}

/// Firmware update status.  Owner: `opendeskphone-update` (the init role seeds
/// `running_bank` and `trial`).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Update {
    pub phase: UpdatePhase,
    pub running_bank: Option<Bank>,
    pub trial: TrialBoot,
    /// Version string of the image being installed (from its label), if any.
    pub incoming_version: String,
    /// The incoming image is stock firmware (going back).
    pub incoming_is_stock: bool,
}

// ------------------------------------------------------------------ Apps

/// A picture: the content of a PNG file.  It prints as its length, and is
/// not part of what is written out of the state.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Picture(pub Arc<[u8]>);

impl Picture {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({} bytes)", self.0.len())
    }
}

/// A mask holds a picture in one color as coverage alone, and whoever
/// draws it picks the color.  Its `alpha` has the format of `GlyphMask` in
/// the SDK's `proto/manifest.proto`.  A mask prints as its size, not its
/// pixels.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Mask {
    pub width: u16,
    pub height: u16,
    pub alpha: Arc<[u8]>,
}

impl std::fmt::Debug for Mask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mask({}x{})", self.width, self.height)
    }
}

/// When an app runs (stock manifest `type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AppKind {
    /// While it is on screen.
    #[default]
    Foreground,
    /// From the phone's start, without being on screen.
    Background,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AppInfo {
    /// Package name (stock manifest `name`); the key for commands.
    pub name: String,
    /// Stock manifest `id`.
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// Stock manifest `type`.
    pub kind: AppKind,
    pub has_idle: bool,
    pub hidden: bool,
    pub builtin: bool,
    /// The name of one of the phone's icons (e.g., `app-calendar`) that
    /// the app asks for, if it has no icon of its own (the manifest's
    /// `phone_icon`).
    pub icon: String,
    /// The app's own icon for this model of phone, from its manifest
    /// (empty: it has none).
    #[serde(skip)]
    pub icon_png: Picture,
    /// This is the app's glyph as the phone draws it, which the app
    /// manager picks from the manifest's masks.
    #[serde(skip)]
    pub glyph: Option<Mask>,
    pub running: bool,
}

/// Installed and running apps.  Owner: `opendeskphone-apps`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Apps {
    /// The apps the phone offers.
    pub installed: Vec<AppInfo>,
    /// Built-in apps the configuration switches off.  They are not in
    /// `installed` and cannot be opened.
    #[serde(default)]
    pub disabled: Vec<AppInfo>,
    /// App on screen now (`None`: a native screen).
    pub foreground: Option<String>,
    /// App providing the idle screen, if one does.
    pub idle_app: Option<String>,
}

// ---------------------------------------------------------------- Health

/// The subsystems of the main role, for liveness and for attributing
/// commands and logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Subsystem {
    Core = 0,
    Config,
    Net,
    Sip,
    Media,
    Dpma,
    Ui,
    Apps,
    Web,
    Ssh,
    Update,
}

impl Subsystem {
    pub const ALL: [Subsystem; 11] = [
        Subsystem::Core,
        Subsystem::Config,
        Subsystem::Net,
        Subsystem::Sip,
        Subsystem::Media,
        Subsystem::Dpma,
        Subsystem::Ui,
        Subsystem::Apps,
        Subsystem::Web,
        Subsystem::Ssh,
        Subsystem::Update,
    ];

    /// The name to show a person: acronyms are written as acronyms.
    pub const fn label(self) -> &'static str {
        match self {
            Subsystem::Core => "Core",
            Subsystem::Config => "Configuration",
            Subsystem::Net => "Network",
            Subsystem::Sip => "SIP",
            Subsystem::Media => "Media",
            Subsystem::Dpma => "DPMA",
            Subsystem::Ui => "UI",
            Subsystem::Apps => "Apps",
            Subsystem::Web => "Web",
            Subsystem::Ssh => "SSH",
            Subsystem::Update => "Update",
        }
    }
}

impl std::fmt::Display for Subsystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Not started (yet, or not in this build/mode).
    #[default]
    Stopped,
    Starting,
    Ok,
    /// Working with a fault worth showing.
    Degraded(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubsystemHealth {
    pub subsystem: Subsystem,
    pub status: HealthStatus,
}

/// Declared status per subsystem.  Each subsystem writes its own entry with
/// [`crate::Store::set_health`].  Heartbeats are separate (they would
/// otherwise wake every subscriber): see [`crate::Store::heartbeat`].
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Health {
    pub subsystems: Vec<SubsystemHealth>,
    /// The main role has declared itself healthy (network up, UI running);
    /// this is what ends a trial boot.
    pub healthy: bool,
}

impl Health {
    pub fn status(&self, s: Subsystem) -> &HealthStatus {
        static STOPPED: HealthStatus = HealthStatus::Stopped;
        self.subsystems
            .iter()
            .find(|h| h.subsystem == s)
            .map(|h| &h.status)
            .unwrap_or(&STOPPED)
    }
}

// -------------------------------------------------------------------- Ui

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// A message for the user that any subsystem can raise
/// ([`crate::Command::Notify`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub id: u64,
    pub level: NoticeLevel,
    pub title: String,
    pub text: String,
    /// Dismiss by itself after this long (0: stays until dismissed).
    pub timeout_ms: u32,
}

/// What is on screen.  Owner: `opendeskphone-ui`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Ui {
    /// Name of the top native screen (`idle`, `dialer`, `call`, `menu`, ...)
    /// or `app:<name>`.
    pub screen: String,
    pub backlight_on: bool,
    pub backlight_level: u8,
    pub contrast: u8,
    pub notices: Vec<Notice>,
    /// Digits typed into the dialer so far (shown by the web UI too).
    pub dial_buffer: String,
}

// ---------------------------------------------------------------- Paging

/// A multicast paging group from the configuration (`<multicastpage>`).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PageGroup {
    pub id: String,
    pub name: String,
    /// Group address and port.
    pub addr: String,
    pub port: u16,
}

/// What multicast paging is doing.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PageActivity {
    #[default]
    Idle,
    /// A page is playing; `group` is the listener's `id`.
    Receiving { group: String, name: String },
    /// A page arrived and was not played because a call is up and the
    /// group may not interrupt calls.
    Blocked { group: String, name: String },
    /// This phone is paging; `group` is the broadcast's `id`.
    Sending { group: String, name: String },
}

/// Multicast paging.  Owner: the `dphone` main role (paging subsystem).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Paging {
    /// Groups this phone listens to.
    pub listeners: Vec<PageGroup>,
    /// Groups this phone can page (`Command::StartPage`).
    pub broadcasts: Vec<PageGroup>,
    pub activity: PageActivity,
    /// Why the last page could not be sent or played; empty when fine.
    pub error: String,
}

// ---------------------------------------------------------------- Alarms

/// One alarm of the alarm clock.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Alarm {
    /// Identifies the alarm; never 0 for an alarm the phone has.
    pub id: u32,
    /// The time of day, in the phone's time zone.
    pub hour: u8,
    pub minute: u8,
    /// The days of the week it rings on: bit 0 is Sunday, bit 6 Saturday.
    /// 0 is a one-time alarm: it rings the next time the clock shows its
    /// time, and is then switched off.
    pub days: u8,
    /// The alarm tone it rings with (one of [`Alarms::tones`]).
    pub tone: String,
    pub enabled: bool,
}

impl Alarm {
    /// Every day of the week.
    pub const EVERY_DAY: u8 = 0x7f;
}

/// The alarm that is ringing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlarmRinging {
    pub id: u32,
    /// The alarm's time of day, for the screen.
    pub hour: u8,
    pub minute: u8,
}

/// The alarm clock.  Owner: the `dphone` main role (alarms subsystem).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Alarms {
    pub alarms: Vec<Alarm>,
    /// The tones an alarm can ring with: the alarm clock's own, not the
    /// ringtones.
    pub tones: Vec<String>,
    /// The idle screen shows the alarm icon now: an alarm will ring
    /// within [`Alarms::UPCOMING_SECS`] or is snoozed.
    pub icon: bool,
    /// The next time an alarm rings (a snoozed alarm included): the alarm
    /// and the Unix time.
    pub next: Option<(u32, u64)>,
    /// The alarm that is ringing now.
    pub ringing: Option<AlarmRinging>,
    /// The snoozed alarm, and the Unix time at which it rings again.
    pub snoozed: Option<(u32, u64)>,
    /// How long a snooze lasts, in seconds.
    pub snooze_secs: u32,
}

impl Alarms {
    /// The most alarms the phone keeps.
    pub const MAX: usize = 16;
    /// An alarm is upcoming, for the idle screen's icon, when it rings
    /// within this many seconds.
    pub const UPCOMING_SECS: u64 = 24 * 3600;

    pub fn get(&self, id: u32) -> Option<&Alarm> {
        self.alarms.iter().find(|a| a.id == id)
    }
}

// ------------------------------------------------------------- Expansion

/// One expansion module (EXP100 or EXP150) attached to the PC port.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExpansionModule {
    /// Node number, 0 for the module nearest the phone.
    pub node: u8,
    /// The module's address on the private expansion network.
    pub addr: String,
    /// Version text from its discovery message.
    pub version: String,
    /// The key page it shows, from 0.
    pub page: u8,
}

/// Expansion modules.  Owner: the `dphone` main role (expansion subsystem).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Expansion {
    /// The configuration enables expansion modules.
    pub enabled: bool,
    pub modules: Vec<ExpansionModule>,
    /// Why the subsystem is not running; empty when fine.
    pub error: String,
}

// ------------------------------------------------------------- Bluetooth

/// A Bluetooth device, found or paired.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BluetoothDevice {
    /// Address as text, `00:1B:DC:0F:A1:02`.
    pub addr: String,
    /// Empty when the device has not said.
    pub name: String,
    /// Signal strength in dBm when it was found; 0 for a paired device
    /// that is not in range.
    pub rssi: i8,
}

/// A pairing that waits for the user.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BluetoothPairing {
    pub addr: String,
    pub name: String,
    /// Six digits to compare with the other device's display, when it has
    /// one.
    pub passkey: Option<u32>,
    /// The device pairs the old way and needs a PIN typed in.
    pub pin_needed: bool,
}

/// Bluetooth headsets (D65).  Owner: the `dphone` main role (Bluetooth
/// subsystem).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Bluetooth {
    /// This model has Bluetooth and its controller answered.
    pub present: bool,
    /// The controller is on; the phone switches it on once it answers.
    pub powered: bool,
    /// The controller's own address.
    pub address: String,
    pub discovering: bool,
    /// Devices seen by the current or last discovery, strongest first.
    pub found: Vec<BluetoothDevice>,
    pub paired: Vec<BluetoothDevice>,
    /// Address of the pairing in progress, before the user is asked.
    pub pairing_with: String,
    /// A question for the user: accept this pairing?
    pub pairing: Option<BluetoothPairing>,
    /// Address of the connected headset; empty when none.
    pub connected: String,
    /// Call audio is on the headset.
    pub audio_open: bool,
    /// Why the last operation failed; empty when fine.
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: u32, state: CallState) -> Call {
        let mut c = Call::new(CallId(id), AccountId(0), Direction::Incoming, Party::default());
        c.state = state;
        c
    }

    /// Stock's rules for the speaker, headset, and mute lamps.
    #[test]
    fn audio_lamps_follow_the_path_in_use() {
        let green = LampState::solid(LampColor::Green);
        let red = LampState::solid(LampColor::Red);
        let off = LampState::OFF;
        let mut audio = Audio::default();
        let mut calls = Calls::default();
        // Idle, also with headset mode latched on: nothing.
        audio.headset_mode = true;
        assert_eq!(audio_lamps(&audio, &calls), (off, off, off));
        audio.headset_mode = false;

        // Ringing through the speaker: still nothing.
        calls.calls.push(call(1, CallState::Ringing));
        audio.path = AudioPath::Speaker;
        audio.ringing = true;
        assert_eq!(audio_lamps(&audio, &calls), (off, off, off));

        // Answered on the speaker; then a second call alerts.
        calls.calls[0].state = CallState::Active;
        audio.ringing = false;
        assert_eq!(audio_lamps(&audio, &calls), (green, off, off));
        calls.calls.push(call(2, CallState::Ringing));
        audio.ringing = true;
        assert_eq!(audio_lamps(&audio, &calls), (green, off, off));
        calls.calls.pop();
        audio.ringing = false;

        // Muted, moved to the headset, then to the handset.
        audio.muted = true;
        assert_eq!(audio_lamps(&audio, &calls), (green, off, red));
        audio.path = AudioPath::Headset;
        assert_eq!(audio_lamps(&audio, &calls), (off, green, red));
        audio.path = AudioPath::Handset;
        assert_eq!(audio_lamps(&audio, &calls), (off, off, red));

        // On-hook dialing: dial tone on the speaker, no call yet.
        let calls = Calls::default();
        let audio = Audio { path: AudioPath::Speaker, tone: Some(Tone::Dial), ..Audio::default() };
        assert_eq!(audio_lamps(&audio, &calls), (green, off, off));
    }

    #[test]
    fn blf_lamps_are_stocks_defaults() {
        assert_eq!(BlfState::Idle.lamp(), LampState::OFF);
        assert_eq!(BlfState::Unknown.lamp(), LampState::OFF);
        assert_eq!(BlfState::Unavailable.lamp(), LampState::OFF);
        assert_eq!(BlfState::Ringing.lamp(), LampState::blinking(LampColor::Green, Blink::Fast));
        assert_eq!(BlfState::InUse.lamp(), LampState::solid(LampColor::Green));
        assert_eq!(BlfState::OnHold.lamp(), LampState::blinking(LampColor::Red, Blink::Slow));
        assert_eq!(BlfState::OnHoldRinging.lamp(), LampState::blinking(LampColor::Red, Blink::Fast));
    }
}
