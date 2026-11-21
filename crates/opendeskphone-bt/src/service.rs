//! The Bluetooth thread: one `poll` loop over the management socket, the
//! service-discovery server, the RFCOMM link to the headset, and the voice
//! connection.
//!
//! One headset is connected at a time.  The caller drives it with
//! [`Command`]s and hears back through the [`Event`] callback, which runs on
//! this thread.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use log::{debug, info, warn};

use crate::hfp::{self, Action, CallState, Dialect, Gateway};
use crate::mgmt::{self, MgmtEvent, op};
use crate::sdp::{self, Record};
use crate::store::Store;
use crate::{Addr, sock};

/// Local RFCOMM channels of the two gateway services.
const HSP_CHANNEL: u8 = 12;
const HFP_CHANNEL: u8 = 13;

/// Class of device: major class "phone".  The minor class is left at 0
/// ("uncategorized"); the service bits come from the registered UUIDs.
const DEV_CLASS_MAJOR_PHONE: u8 = 0x02;
/// Service class hint bits: audio (0x20) and telephony (0x40), in the
/// class of device's top byte.
const SERVICE_HINT: u8 = 0x20 | 0x40;

/// Broadcom vendor command `Write_SCO_PCM_Int_Param`, whose five bytes are
/// [`Config::sco_pcm_params`].
const HCI_WRITE_SCO_PCM_INT_PARAM: u16 = 0xfc1c;
/// The D65's: voice on the PCM pins (routing 0), 1024 kbit/s (rate 3),
/// short frame sync (0), and the controller as slave for sync (0) and
/// clock (0).  The SoC's PCM/TDM interface drives both at exactly that
/// rate (`bcmring-pcm.c`).  These are the same five bytes as
/// `brcm,bt-pcm-int-params` in the devicetree, which the kernel applies
/// only after loading a patch file.
pub const SCO_PCM_PARAMS_D65: [u8; 5] = [0, 3, 0, 0, 0];
/// The D80's, which are the stock firmware's (`bt_sco_param` of its
/// `libbt-vendor.so`): as the D65's but 512 kbit/s (rate 2).  The clocks
/// are the ZL38040 voice processor's.
pub const SCO_PCM_PARAMS_D80: [u8; 5] = [0, 2, 0, 0, 0];

// What the stock D80 firmware sends the controller before each voice
// connection (`hw_set_audio_state` of its `libbt-vendor.so`), in this
// order, so that the controller codes the voice on its PCM pins itself:
// CVSD, or for wide-band speech mSBC.
/// `Enable_WBS`: on with codec 2 (mSBC), or off.
const HCI_ENABLE_WBS: u16 = 0xfc7e;
/// `Write_I2SPCM_Interface_Param`: PCM (0), the controller's clock role,
/// the sample rate (0 is 8 kHz, 1 is 16 kHz), and the clock rate.
const HCI_WRITE_I2SPCM_INTERFACE_PARAM: u16 = 0xfc6d;
/// `Write_PCM_Data_Format_Param`, with the stock firmware's five bytes.
const HCI_WRITE_PCM_DATA_FORMAT_PARAM: u16 = 0xfc1e;
const PCM_DATA_FORMAT: [u8; 5] = [0, 0, 3, 0, 0];

/// How long a service query and an outgoing connection may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct Config {
    /// The name other devices see.
    pub name: String,
    /// Where paired devices are kept (normally [`crate::store::DEFAULT_DIR`]).
    pub store_dir: PathBuf,
    /// PIN offered to a device that pairs the old way (before Bluetooth
    /// 2.1).  `None` asks through [`Event::PinRequest`].
    pub default_pin: Option<String>,
    /// How the controller's PCM pins carry voice (`Write_SCO_PCM_Int_Param`):
    /// [`SCO_PCM_PARAMS_D65`] or [`SCO_PCM_PARAMS_D80`].
    pub sco_pcm_params: [u8; 5],
    /// The phone's Bluetooth voice path can be wide-band (the D80's, whose
    /// voice processor takes either rate from the controller): codecs are
    /// negotiated with a headset that negotiates them, and the controller
    /// is switched between the two before each voice connection.
    pub wideband: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    HandsFree,
    Headset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    StartDiscovery,
    StopDiscovery,
    /// Pair with a device and, on success, connect to it.
    Pair(Addr),
    /// Answer [`Event::PairingConfirm`].
    ConfirmPairing { addr: Addr, accept: bool },
    /// Answer [`Event::PinRequest`]; `None` refuses.
    Pin { addr: Addr, pin: Option<String> },
    /// Forget a device.
    Unpair(Addr),
    /// Connect to a paired headset.
    Connect(Addr),
    /// Drop the headset connection.
    Disconnect,
    /// The phone's call state, for the headset's indicators and ringing.
    SetCallState(CallState),
    /// Open or close the voice connection to the connected headset.
    SetAudio(bool),
    /// The earpiece volume changed on the phone (0 to 15).
    SetSpeakerVolume(u8),
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The controller appeared, disappeared, or changed power state.
    Adapter { present: bool, powered: bool, address: Addr },
    Discovering(bool),
    DeviceFound { addr: Addr, name: Option<String>, rssi: i8, class: Option<u32> },
    /// The user has to confirm pairing; with `passkey`, by comparing six
    /// digits with the other device's display.
    PairingConfirm { addr: Addr, passkey: Option<u32> },
    PinRequest { addr: Addr },
    Paired { addr: Addr, name: String },
    PairFailed { addr: Addr },
    Unpaired { addr: Addr },
    HeadsetConnected { addr: Addr, profile: Profile },
    HeadsetDisconnected { addr: Addr },
    /// Voice is flowing on the controller's PCM pins (on the D65, to the
    /// ALSA card [`crate::ALSA_CARD`]): narrow-band at 8 kHz, or with
    /// `wideband` at 16 kHz.
    AudioOpened { wideband: bool },
    AudioClosed,
    Answer,
    HangUp,
    Dial(String),
    Redial,
    Dtmf(char),
    SpeakerVolume(u8),
    MicVolume(u8),
    /// The button of a Headset-profile device: answer, or hang up.
    Button,
}

/// Handle to the Bluetooth thread.
pub struct Bluetooth {
    tx: Sender<Command>,
    wake: OwnedFd,
}

impl Bluetooth {
    /// Start the thread.  Fails when the kernel has no Bluetooth support
    /// (the modules are not loaded).
    pub fn spawn(config: Config, events: impl FnMut(Event) + Send + 'static) -> io::Result<Bluetooth> {
        let mgmt = sock::mgmt()?;
        let (wake_rx, wake_tx) = pipe()?;
        let (tx, rx) = mpsc::channel();
        let store = Store::open(&config.store_dir);
        let records = vec![Record::handsfree_ag(HFP_CHANNEL, config.wideband), Record::headset_ag(HSP_CHANNEL)];
        let mut service = Service {
            config,
            events: Box::new(events),
            rx,
            wake: wake_rx,
            mgmt,
            store,
            index: None,
            address: Addr::ANY,
            powered: false,
            listeners: None,
            sdp_clients: Vec::new(),
            query: None,
            link: None,
            voice: None,
            call_state: CallState::Idle,
            pairing: None,
            names: HashMap::new(),
            records,
        };
        std::thread::Builder::new().name("bluetooth".into()).spawn(move || service.run())?;
        Ok(Bluetooth { tx, wake: wake_tx })
    }

    pub fn send(&self, command: Command) {
        if self.tx.send(command).is_ok() {
            let _ = sock::write(self.wake.as_raw_fd(), &[0]);
        }
    }
}

impl Drop for Bluetooth {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors are new and owned by nothing else.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

struct Listeners {
    sdp: OwnedFd,
    hfp: OwnedFd,
    hsp: OwnedFd,
    sco: OwnedFd,
}

/// Finding a headset's RFCOMM channel, then connecting to it.
struct Query {
    addr: Addr,
    fd: OwnedFd,
    connecting: bool,
    profile: Profile,
    tid: u16,
    assembled: Vec<u8>,
    deadline: Instant,
}

struct Link {
    addr: Addr,
    fd: OwnedFd,
    profile: Profile,
    /// The RFCOMM connection is still being set up.
    connecting: bool,
    deadline: Instant,
    gateway: Gateway,
    /// `HeadsetConnected` has been reported.
    announced: bool,
}

struct Voice {
    fd: OwnedFd,
    connecting: bool,
    /// Wide-band speech (mSBC).
    wideband: bool,
}

struct Service {
    config: Config,
    events: Box<dyn FnMut(Event) + Send>,
    rx: Receiver<Command>,
    wake: OwnedFd,
    mgmt: OwnedFd,
    store: Store,
    index: Option<u16>,
    address: Addr,
    powered: bool,
    listeners: Option<Listeners>,
    sdp_clients: Vec<OwnedFd>,
    query: Option<Query>,
    link: Option<Link>,
    voice: Option<Voice>,
    call_state: CallState,
    /// The device we are pairing with at the user's request.
    pairing: Option<Addr>,
    /// Names heard during discovery, for the store.
    names: HashMap<Addr, String>,
    records: Vec<Record>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Wake,
    Mgmt,
    SdpListen,
    HfpListen,
    HspListen,
    ScoListen,
    SdpClient(usize),
    Query,
    Link,
    Voice,
}

impl Service {
    fn emit(&mut self, event: Event) {
        (self.events)(event);
    }

    fn mgmt_send(&mut self, packet: Vec<u8>) {
        if let Err(e) = sock::write(self.mgmt.as_raw_fd(), &packet) {
            warn!("management command: {e}");
        }
    }

    fn run(&mut self) {
        self.mgmt_send(mgmt::command(op::READ_INDEX_LIST, mgmt::INDEX_NONE, &[]));
        loop {
            let mut fds: Vec<libc::pollfd> = Vec::new();
            let mut sources: Vec<Source> = Vec::new();
            let mut add = |fd: RawFd, events: libc::c_short, source: Source| {
                fds.push(libc::pollfd { fd, events, revents: 0 });
                sources.push(source);
            };
            add(self.wake.as_raw_fd(), libc::POLLIN, Source::Wake);
            add(self.mgmt.as_raw_fd(), libc::POLLIN, Source::Mgmt);
            if let Some(l) = &self.listeners {
                add(l.sdp.as_raw_fd(), libc::POLLIN, Source::SdpListen);
                add(l.hfp.as_raw_fd(), libc::POLLIN, Source::HfpListen);
                add(l.hsp.as_raw_fd(), libc::POLLIN, Source::HspListen);
                add(l.sco.as_raw_fd(), libc::POLLIN, Source::ScoListen);
            }
            for (i, c) in self.sdp_clients.iter().enumerate() {
                add(c.as_raw_fd(), libc::POLLIN, Source::SdpClient(i));
            }
            let in_or_out = |connecting: bool| if connecting { libc::POLLOUT } else { libc::POLLIN };
            if let Some(q) = &self.query {
                add(q.fd.as_raw_fd(), in_or_out(q.connecting), Source::Query);
            }
            if let Some(l) = &self.link {
                add(l.fd.as_raw_fd(), in_or_out(l.connecting), Source::Link);
            }
            if let Some(v) = &self.voice {
                add(v.fd.as_raw_fd(), in_or_out(v.connecting), Source::Voice);
            }

            let now = Instant::now();
            let deadline = [
                self.query.as_ref().map(|q| q.deadline),
                self.link.as_ref().and_then(|l| if l.connecting { Some(l.deadline) } else { l.gateway.deadline() }),
            ]
            .into_iter()
            .flatten()
            .min();
            let timeout = match deadline {
                Some(at) => at.saturating_duration_since(now).as_millis().min(60_000) as libc::c_int + 1,
                None => -1,
            };
            // SAFETY: `fds` is a valid array of `fds.len()` entries.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                warn!("poll: {}", io::Error::last_os_error());
                return;
            }

            // Collect first: handlers change the socket set.
            let ready: Vec<Source> =
                fds.iter().zip(&sources).filter(|(f, _)| f.revents != 0).map(|(_, s)| *s).collect();
            let mut dropped_sdp: Vec<usize> = Vec::new();
            for source in ready {
                match source {
                    Source::Wake => {
                        let mut buf = [0u8; 64];
                        while let Ok(Some(n)) = sock::read(self.wake.as_raw_fd(), &mut buf) {
                            if n == 0 {
                                break;
                            }
                        }
                        while let Ok(command) = self.rx.try_recv() {
                            if command == Command::Shutdown {
                                return;
                            }
                            self.command(command);
                        }
                    }
                    Source::Mgmt => self.read_mgmt(),
                    Source::SdpListen => self.accept_sdp(),
                    Source::HfpListen => self.accept_link(Profile::HandsFree),
                    Source::HspListen => self.accept_link(Profile::Headset),
                    Source::ScoListen => self.accept_voice(),
                    Source::SdpClient(i) => {
                        if !self.serve_sdp(i) {
                            dropped_sdp.push(i);
                        }
                    }
                    Source::Query => self.query_ready(),
                    Source::Link => self.link_ready(),
                    Source::Voice => self.voice_ready(),
                }
            }
            dropped_sdp.sort_unstable();
            for i in dropped_sdp.into_iter().rev() {
                if i < self.sdp_clients.len() {
                    self.sdp_clients.remove(i);
                }
            }
            self.timers();
        }
    }

    fn timers(&mut self) {
        let now = Instant::now();
        if self.query.as_ref().is_some_and(|q| now >= q.deadline) {
            let q = self.query.take().unwrap();
            info!("{}: service query timed out", q.addr);
            self.emit(Event::HeadsetDisconnected { addr: q.addr });
        }
        if self.link.as_ref().is_some_and(|l| l.connecting && now >= l.deadline) {
            info!("headset connection timed out");
            self.drop_link();
        }
        if let Some(l) = &mut self.link
            && !l.connecting
        {
            l.gateway.tick(now);
            self.flush_link();
        }
    }

    // ---- commands ----------------------------------------------------------

    fn command(&mut self, command: Command) {
        debug!("command {command:?}");
        let Some(index) = self.index else {
            return;
        };
        match command {
            Command::StartDiscovery => self.mgmt_send(mgmt::start_discovery(index)),
            Command::StopDiscovery => self.mgmt_send(mgmt::stop_discovery(index)),
            Command::Pair(addr) => {
                self.pairing = Some(addr);
                self.mgmt_send(mgmt::pair_device(index, addr, mgmt::IO_DISPLAY_YES_NO));
            }
            Command::ConfirmPairing { addr, accept } => {
                let opcode = if accept { op::USER_CONFIRM_REPLY } else { op::USER_CONFIRM_NEG_REPLY };
                self.mgmt_send(mgmt::for_device(opcode, index, addr));
            }
            Command::Pin { addr, pin } => match pin {
                Some(pin) => self.mgmt_send(mgmt::pin_code_reply(index, addr, pin.as_bytes())),
                None => self.mgmt_send(mgmt::for_device(op::PIN_CODE_NEG_REPLY, index, addr)),
            },
            Command::Unpair(addr) => {
                if self.link.as_ref().is_some_and(|l| l.addr == addr) {
                    self.drop_link();
                }
                self.mgmt_send(mgmt::unpair_device(index, addr, true));
                match self.store.remove(addr) {
                    Ok(_) => self.emit(Event::Unpaired { addr }),
                    Err(e) => warn!("removing {addr} from the store: {e}"),
                }
            }
            Command::Connect(addr) => self.connect(addr),
            Command::Disconnect => self.drop_link(),
            Command::SetCallState(state) => {
                self.call_state = state.clone();
                if let Some(l) = &mut self.link {
                    l.gateway.set_call_state(Instant::now(), state);
                    self.flush_link();
                }
            }
            Command::SetAudio(on) => self.set_audio(on),
            Command::SetSpeakerVolume(level) => {
                if let Some(l) = &mut self.link {
                    l.gateway.set_speaker_volume(level);
                    self.flush_link();
                }
            }
            Command::Shutdown => {}
        }
    }

    fn power_on(&mut self, index: u16) {
        self.mgmt_send(mgmt::set_mode(op::SET_SSP, index, 1));
        self.mgmt_send(mgmt::set_mode(op::SET_BONDABLE, index, 1));
        self.mgmt_send(mgmt::set_mode(op::SET_CONNECTABLE, index, 1));
        self.mgmt_send(mgmt::set_mode(op::SET_IO_CAPABILITY, index, mgmt::IO_DISPLAY_YES_NO));
        let name = self.config.name.clone();
        self.mgmt_send(mgmt::set_local_name(index, &name));
        self.mgmt_send(mgmt::set_dev_class(index, DEV_CLASS_MAJOR_PHONE, 0));
        self.mgmt_send(mgmt::add_uuid16(index, sdp::uuid::HANDSFREE_AG, SERVICE_HINT));
        self.mgmt_send(mgmt::add_uuid16(index, sdp::uuid::HEADSET_AG, SERVICE_HINT));
        let keys = self.store.keys();
        self.mgmt_send(mgmt::load_link_keys(index, &keys));
        self.mgmt_send(mgmt::set_mode(op::SET_POWERED, index, 1));
    }

    // ---- management events -------------------------------------------------

    fn read_mgmt(&mut self) {
        let mut buf = [0u8; 1024];
        while let Ok(Some(n)) = sock::read(self.mgmt.as_raw_fd(), &mut buf) {
            if n == 0 {
                break;
            }
            match mgmt::parse_event(&buf[..n]) {
                Some((index, event)) => self.mgmt_event(index, event),
                None => debug!("unparsed management packet of {n} bytes"),
            }
        }
    }

    fn set_powered(&mut self, powered: bool) {
        if powered == self.powered {
            return;
        }
        self.powered = powered;
        if powered {
            if let Some(index) = self.index
                && let Err(e) = sock::send_hci_command(index, HCI_WRITE_SCO_PCM_INT_PARAM, &self.config.sco_pcm_params)
            {
                warn!("setting the controller's PCM parameters: {e}");
            }
            match open_listeners() {
                Ok(l) => self.listeners = Some(l),
                Err(e) => warn!("opening the Bluetooth service sockets: {e}"),
            }
        } else {
            self.drop_link();
            self.listeners = None;
            self.sdp_clients.clear();
        }
        let address = self.address;
        self.emit(Event::Adapter { present: true, powered, address });
    }

    fn mgmt_event(&mut self, index: u16, event: MgmtEvent) {
        match event {
            MgmtEvent::IndexAdded if self.index.is_none() => {
                self.mgmt_send(mgmt::command(op::READ_INDEX_LIST, mgmt::INDEX_NONE, &[]));
            }
            MgmtEvent::IndexRemoved if self.index == Some(index) => {
                self.index = None;
                self.powered = false;
                self.drop_link();
                self.listeners = None;
                self.emit(Event::Adapter { present: false, powered: false, address: Addr::ANY });
            }
            MgmtEvent::Complete { opcode: op::READ_INDEX_LIST, status: 0, params } => {
                if self.index.is_none()
                    && let Some(first) = mgmt::parse_index_list(&params).and_then(|l| l.first().copied())
                {
                    self.index = Some(first);
                    self.mgmt_send(mgmt::command(op::READ_INFO, first, &[]));
                }
            }
            MgmtEvent::Complete { opcode: op::READ_INFO, status: 0, params } => {
                if let Some(info) = mgmt::parse_info(&params) {
                    self.address = info.addr;
                    self.powered = false;
                    let powered = info.current_settings & mgmt::SETTING_POWERED != 0;
                    self.emit(Event::Adapter { present: true, powered, address: info.addr });
                    // Bluetooth is on whenever there is a controller.
                    self.power_on(index);
                    if powered {
                        self.set_powered(true);
                    }
                }
            }
            MgmtEvent::Complete { opcode: op::SET_POWERED, status: 0, params } => {
                if let Some(bytes) = params.get(..4) {
                    let settings = u32::from_le_bytes(bytes.try_into().unwrap());
                    self.set_powered(settings & mgmt::SETTING_POWERED != 0);
                }
            }
            MgmtEvent::Complete { opcode: op::PAIR_DEVICE, status, params } => {
                let addr = params.get(..6).map(|b| Addr(b.try_into().unwrap())).or(self.pairing);
                self.pairing = None;
                if let Some(addr) = addr {
                    if status == 0 {
                        self.connect(addr);
                    } else {
                        info!("pairing with {addr} failed: status {status}");
                        self.emit(Event::PairFailed { addr });
                    }
                }
            }
            MgmtEvent::Complete { opcode, status, .. } if status != 0 => {
                info!("management command {opcode:#06x} failed: status {status}");
            }
            MgmtEvent::NewSettings(settings) => self.set_powered(settings & mgmt::SETTING_POWERED != 0),
            MgmtEvent::Discovering(on) => self.emit(Event::Discovering(on)),
            MgmtEvent::Found { addr, rssi, name, class } => {
                if let Some(name) = &name {
                    self.names.insert(addr, name.clone());
                }
                self.emit(Event::DeviceFound { addr, name, rssi, class });
            }
            MgmtEvent::ConfirmRequest { addr, hint, value } => {
                if hint && self.pairing == Some(addr) {
                    // Nothing to compare, and the user chose this device.
                    self.mgmt_send(mgmt::for_device(op::USER_CONFIRM_REPLY, index, addr));
                } else {
                    self.emit(Event::PairingConfirm { addr, passkey: (!hint).then_some(value) });
                }
            }
            MgmtEvent::PinRequest { addr } => match self.config.default_pin.clone() {
                Some(pin) => self.mgmt_send(mgmt::pin_code_reply(index, addr, pin.as_bytes())),
                None => self.emit(Event::PinRequest { addr }),
            },
            MgmtEvent::NewLinkKey { store, key } => {
                if store {
                    let addr = key.addr;
                    let name = self.names.get(&addr).cloned();
                    if let Err(e) = self.store.put(key, name) {
                        warn!("saving the link key of {addr}: {e}");
                    }
                    let name = self.store.get(addr).map(|d| d.name.clone()).unwrap_or_default();
                    self.emit(Event::Paired { addr, name });
                }
            }
            MgmtEvent::AuthFailed { addr, status } => {
                info!("authentication with {addr} failed: status {status}");
                if self.pairing == Some(addr) {
                    self.pairing = None;
                }
                self.emit(Event::PairFailed { addr });
            }
            MgmtEvent::Connected { addr, name: Some(name) } => {
                self.names.insert(addr, name.clone());
                if let Err(e) = self.store.set_name(addr, &name) {
                    warn!("saving the name of {addr}: {e}");
                }
            }
            MgmtEvent::Disconnected { addr } | MgmtEvent::ConnectFailed { addr, .. } => {
                if self.link.as_ref().is_some_and(|l| l.addr == addr) {
                    self.drop_link();
                }
                if self.query.as_ref().is_some_and(|q| q.addr == addr) {
                    self.query = None;
                    self.emit(Event::HeadsetDisconnected { addr });
                }
            }
            _ => {}
        }
    }

    // ---- service discovery server ------------------------------------------

    fn accept_sdp(&mut self) {
        let Some(l) = &self.listeners else { return };
        match sock::accept(l.sdp.as_raw_fd(), true) {
            Ok(Some((fd, _))) if self.sdp_clients.len() < 4 => self.sdp_clients.push(fd),
            Ok(_) => {}
            Err(e) => debug!("service discovery accept: {e}"),
        }
    }

    /// Answer one request; `false` when the client is gone.
    fn serve_sdp(&mut self, i: usize) -> bool {
        let Some(fd) = self.sdp_clients.get(i).map(|f| f.as_raw_fd()) else { return true };
        let mut buf = [0u8; 1024];
        match sock::read(fd, &mut buf) {
            Ok(Some(0)) | Err(_) => false,
            Ok(None) => true,
            Ok(Some(n)) => {
                // 48 is the smallest MTU an L2CAP peer may have; replies
                // that do not fit continue.
                match sdp::serve(&self.records, &buf[..n], 48) {
                    Some(reply) => sock::write(fd, &reply).is_ok(),
                    None => true,
                }
            }
        }
    }

    // ---- connecting to a headset -------------------------------------------

    fn connect(&mut self, addr: Addr) {
        if !self.powered || self.link.is_some() || self.query.is_some() {
            return;
        }
        self.start_query(addr, Profile::HandsFree);
    }

    fn start_query(&mut self, addr: Addr, profile: Profile) {
        match sock::l2cap_connect(addr, sdp::PSM) {
            Ok(fd) => {
                self.query = Some(Query {
                    addr,
                    fd,
                    connecting: true,
                    profile,
                    tid: 1,
                    assembled: Vec::new(),
                    deadline: Instant::now() + CONNECT_TIMEOUT,
                });
            }
            Err(e) => {
                info!("{addr}: service query: {e}");
                self.emit(Event::HeadsetDisconnected { addr });
            }
        }
    }

    fn query_class(profile: Profile) -> u16 {
        match profile {
            Profile::HandsFree => sdp::uuid::HANDSFREE,
            Profile::Headset => sdp::uuid::HEADSET,
        }
    }

    fn query_ready(&mut self) {
        let Some(q) = &mut self.query else { return };
        let fd = q.fd.as_raw_fd();
        let outcome: Result<Option<Option<u8>>, io::Error> = (|| {
            if q.connecting {
                sock::take_error(fd)?;
                q.connecting = false;
                sock::write(fd, &sdp::search_attr_request(q.tid, Self::query_class(q.profile), &[]))?;
                return Ok(None);
            }
            let mut buf = [0u8; 2048];
            let n = match sock::read(fd, &mut buf)? {
                None => return Ok(None),
                Some(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Some(n) => n,
            };
            let Some((bytes, cont)) = sdp::parse_search_attr_response(q.tid, &buf[..n]) else {
                return Err(io::ErrorKind::InvalidData.into());
            };
            q.assembled.extend_from_slice(bytes);
            if q.assembled.len() > 16 * 1024 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            if cont.is_empty() {
                return Ok(Some(sdp::rfcomm_channel(&q.assembled)));
            }
            let cont = cont.to_vec();
            q.tid += 1;
            sock::write(fd, &sdp::search_attr_request(q.tid, Self::query_class(q.profile), &cont))?;
            Ok(None)
        })();
        match outcome {
            Ok(None) => {}
            Ok(Some(Some(channel))) => {
                let q = self.query.take().unwrap();
                self.open_link(q.addr, q.profile, channel);
            }
            Ok(Some(None)) => {
                // No Hands-Free service: try the Headset profile.
                let q = self.query.take().unwrap();
                if q.profile == Profile::HandsFree {
                    self.start_query(q.addr, Profile::Headset);
                } else {
                    info!("{}: neither a hands-free unit nor a headset", q.addr);
                    self.emit(Event::HeadsetDisconnected { addr: q.addr });
                }
            }
            Err(e) => {
                let q = self.query.take().unwrap();
                info!("{}: service query: {e}", q.addr);
                self.emit(Event::HeadsetDisconnected { addr: q.addr });
            }
        }
    }

    fn new_link(&mut self, addr: Addr, fd: OwnedFd, profile: Profile, connecting: bool) {
        let dialect = match profile {
            Profile::HandsFree => Dialect::HandsFree,
            Profile::Headset => Dialect::Headset,
        };
        let mut gateway = Gateway::new(dialect).with_wideband(self.config.wideband);
        gateway.set_call_state(Instant::now(), self.call_state.clone());
        self.link = Some(Link {
            addr,
            fd,
            profile,
            connecting,
            deadline: Instant::now() + CONNECT_TIMEOUT,
            gateway,
            announced: false,
        });
        if !connecting {
            self.link_up();
        }
    }

    fn open_link(&mut self, addr: Addr, profile: Profile, channel: u8) {
        match sock::rfcomm_connect(addr, channel) {
            Ok(fd) => self.new_link(addr, fd, profile, true),
            Err(e) => {
                info!("{addr}: connecting: {e}");
                self.emit(Event::HeadsetDisconnected { addr });
            }
        }
    }

    fn accept_link(&mut self, profile: Profile) {
        let Some(l) = &self.listeners else { return };
        let listener = match profile {
            Profile::HandsFree => l.hfp.as_raw_fd(),
            Profile::Headset => l.hsp.as_raw_fd(),
        };
        match sock::accept(listener, false) {
            // One headset at a time: a second one is turned away by
            // closing its connection.
            Ok(Some((fd, addr))) if self.link.is_none() => {
                self.query = None;
                self.new_link(addr, fd, profile, false);
            }
            Ok(_) => {}
            Err(e) => debug!("headset accept: {e}"),
        }
    }

    /// The RFCOMM connection is up.  A Headset-profile device is usable at
    /// once; a hands-free unit after its set-up dialogue.
    fn link_up(&mut self) {
        self.flush_link();
        self.announce();
    }

    fn announce(&mut self) {
        if let Some(l) = &mut self.link
            && !l.announced
            && l.gateway.is_connected()
        {
            l.announced = true;
            let (addr, profile) = (l.addr, l.profile);
            info!("{addr}: headset connected ({profile:?})");
            self.emit(Event::HeadsetConnected { addr, profile });
        }
    }

    fn flush_link(&mut self) {
        let Some(l) = &mut self.link else { return };
        if l.connecting {
            return;
        }
        let out = l.gateway.take_output();
        if !out.is_empty()
            && let Err(e) = sock::write(l.fd.as_raw_fd(), &out)
        {
            info!("{}: write: {e}", l.addr);
            self.drop_link();
        }
    }

    fn link_ready(&mut self) {
        let Some(l) = &mut self.link else { return };
        let fd = l.fd.as_raw_fd();
        if l.connecting {
            match sock::take_error(fd) {
                Ok(()) => {
                    l.connecting = false;
                    self.link_up();
                }
                Err(e) => {
                    info!("{}: connecting: {e}", l.addr);
                    self.drop_link();
                }
            }
            return;
        }
        let mut buf = [0u8; 512];
        match sock::read(fd, &mut buf) {
            Ok(None) => {}
            Ok(Some(0)) | Err(_) => self.drop_link(),
            Ok(Some(n)) => {
                l.gateway.receive(&buf[..n]);
                let actions = l.gateway.take_actions();
                self.flush_link();
                for action in actions {
                    match action {
                        Action::Connected => self.announce(),
                        Action::Answer => self.emit(Event::Answer),
                        Action::HangUp => self.emit(Event::HangUp),
                        Action::Dial(number) => self.emit(Event::Dial(number)),
                        Action::Redial => self.emit(Event::Redial),
                        Action::Dtmf(c) => self.emit(Event::Dtmf(c)),
                        Action::SpeakerVolume(v) => self.emit(Event::SpeakerVolume(v)),
                        Action::MicVolume(v) => self.emit(Event::MicVolume(v)),
                        Action::Button => self.emit(Event::Button),
                        Action::OpenAudio => self.set_audio(true),
                        Action::CodecSelected(codec) => self.connect_voice(codec == hfp::Codec::Msbc),
                    }
                }
            }
        }
    }

    fn drop_link(&mut self) {
        self.close_voice();
        if let Some(l) = self.link.take() {
            self.emit(Event::HeadsetDisconnected { addr: l.addr });
        }
    }

    // ---- voice ---------------------------------------------------------------

    fn set_audio(&mut self, on: bool) {
        if !on {
            self.close_voice();
            return;
        }
        if self.voice.is_some() {
            return;
        }
        let Some(l) = &mut self.link else { return };
        if l.connecting {
            return;
        }
        if l.gateway.negotiates() {
            // The headset agrees to the codec first; the connection is
            // opened when it has (`Action::CodecSelected`).
            let codec = l.gateway.best_codec();
            l.gateway.offer_codec(codec);
            self.flush_link();
        } else {
            self.connect_voice(false);
        }
    }

    /// Sets the controller up for the voice connection's codec, as the
    /// stock D80 firmware does.  Only where voice can be wide-band: a
    /// controller that never leaves narrow-band keeps what it was given
    /// when it was powered.
    fn prepare_controller(&self, wideband: bool) {
        let (Some(index), true) = (self.index, self.config.wideband) else { return };
        let pcm = self.config.sco_pcm_params;
        let commands: [(u16, &[u8]); 4] = [
            (HCI_ENABLE_WBS, if wideband { &[1, 2, 0] } else { &[0, 1, 0] }),
            (HCI_WRITE_I2SPCM_INTERFACE_PARAM, &[0, pcm[4], u8::from(wideband), pcm[1]]),
            (HCI_WRITE_SCO_PCM_INT_PARAM, &pcm),
            (HCI_WRITE_PCM_DATA_FORMAT_PARAM, &PCM_DATA_FORMAT),
        ];
        for (opcode, params) in commands {
            if let Err(e) = sock::send_hci_command(index, opcode, params) {
                warn!("setting the controller up for {} voice ({opcode:#06x}): {e}", if wideband { "wide-band" } else { "narrow-band" });
            }
            // Vendor commands are not held back until the one before has
            // completed, as the stock firmware holds each back.
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn connect_voice(&mut self, wideband: bool) {
        if self.voice.is_some() {
            return;
        }
        let Some(l) = &self.link else { return };
        let addr = l.addr;
        self.prepare_controller(wideband);
        match sock::sco_connect(addr, wideband) {
            Ok(fd) => self.voice = Some(Voice { fd, connecting: true, wideband }),
            Err(e) => {
                info!("{addr}: voice connection: {e}");
                self.voice_failed(wideband);
            }
        }
    }

    /// A voice connection did not open.  A wide-band one is tried again
    /// narrow-band, which every headset has, after the headset agrees.
    fn voice_failed(&mut self, wideband: bool) {
        if wideband && let Some(l) = &mut self.link {
            info!("{}: no wide-band voice connection; trying narrow-band", l.addr);
            l.gateway.offer_codec(hfp::Codec::Cvsd);
            self.flush_link();
        } else {
            self.emit(Event::AudioClosed);
        }
    }

    fn accept_voice(&mut self) {
        let Some(l) = &self.listeners else { return };
        match sock::accept(l.sco.as_raw_fd(), false) {
            Ok(Some((fd, addr))) if self.voice.is_none() && self.link.as_ref().is_some_and(|l| l.addr == addr) => {
                // A headset opens the connection itself only without
                // codec negotiation: narrow-band.
                self.prepare_controller(false);
                self.voice = Some(Voice { fd, connecting: false, wideband: false });
                self.emit(Event::AudioOpened { wideband: false });
            }
            Ok(_) => {}
            Err(e) => debug!("voice accept: {e}"),
        }
    }

    fn voice_ready(&mut self) {
        let Some(v) = &mut self.voice else { return };
        let fd = v.fd.as_raw_fd();
        if v.connecting {
            match sock::take_error(fd) {
                Ok(()) => {
                    v.connecting = false;
                    let wideband = v.wideband;
                    self.emit(Event::AudioOpened { wideband });
                }
                Err(e) => {
                    info!("voice connection: {e}");
                    let wideband = v.wideband;
                    self.voice = None;
                    self.voice_failed(wideband);
                }
            }
            return;
        }
        // Voice travels on the controller's PCM pins, so nothing useful
        // arrives here; a read tells us when the connection ends.
        let mut buf = [0u8; 256];
        match sock::read(fd, &mut buf) {
            Ok(Some(0)) | Err(_) => self.close_voice(),
            _ => {}
        }
    }

    fn close_voice(&mut self) {
        if let Some(v) = self.voice.take() {
            let was_open = !v.connecting;
            drop(v);
            if was_open {
                self.emit(Event::AudioClosed);
            }
        }
    }
}

fn open_listeners() -> io::Result<Listeners> {
    Ok(Listeners {
        sdp: sock::l2cap_listen(sdp::PSM)?,
        hfp: sock::rfcomm_listen(HFP_CHANNEL)?,
        hsp: sock::rfcomm_listen(HSP_CHANNEL)?,
        sco: sock::sco_listen()?,
    })
}
