//! Bluetooth headsets on the D65: the phone's side of `opendeskphone_bt`.
//!
//! `opendeskphone_bt` owns the controller and the headset profiles; this file
//! connects it to the phone:
//!
//! - its events become the `Bluetooth` state topic (what the phone's
//!   Bluetooth application and the web UI show) and call commands: a
//!   headset can answer, hang up, dial, redial, send digits, and set its
//!   volume;
//! - the phone's call state goes to the headset (so it rings and shows a
//!   call), and its voice link opens while a call is on the headset path;
//!   while the link is up the audio service uses the Bluetooth voice device
//!   for that path;
//! - the commands of the Bluetooth domain (discovery, pairing with the
//!   user's confirmation, connecting) are passed on.
//!
//! Bluetooth is on whenever the phone has a controller.  Paired devices are
//! kept by `opendeskphone_bt` in `/nvdata/opendeskphone/bluetooth`.

use std::path::PathBuf;

use opendeskphone_bt::{Addr, CallState as BtCall, Command as Bt, Event as BtEvent};
use opendeskphone_state::{
    Audio, AudioPath, BluetoothDevice, BluetoothPairing, CallState, Calls, Command, CommandError, Direction, Domain, HeadsetKind,
    Reply, Topic, VolumeTarget,
};
use log::{info, warn};
use tokio::sync::mpsc;

use super::audio::AudioService;
use crate::mainrole::Context;

type State = opendeskphone_state::Bluetooth;

/// Headset volume steps (the Hands-Free profile's 0 to 15).
const HEADSET_STEPS: u32 = 15;

/// The phone's call state as a headset sees it: the alerting call first,
/// then a call being set up, then a connected one.
pub fn headset_call_state(calls: &Calls) -> BtCall {
    let live = || calls.calls.iter().filter(|c| !c.state.is_ended());
    if let Some(c) = live().find(|c| c.state == CallState::Ringing && c.direction == Direction::Incoming) {
        let number = Some(c.remote.number.clone()).filter(|n| !n.is_empty());
        return BtCall::Incoming { number };
    }
    if live().any(|c| matches!(c.state, CallState::Ringback { .. })) {
        return BtCall::Alerting;
    }
    if live().any(|c| matches!(c.state, CallState::Dialing | CallState::Proceeding)) {
        return BtCall::Dialing;
    }
    if live().any(|c| matches!(c.state, CallState::Active | CallState::Held { .. } | CallState::Transferring)) {
        return BtCall::Active;
    }
    BtCall::Idle
}

/// Phone volume step to headset volume step and back.
pub fn to_headset_volume(level: u8, max: u8) -> u8 {
    ((level.min(max) as u32 * HEADSET_STEPS + max as u32 / 2) / max.max(1) as u32) as u8
}

pub fn from_headset_volume(level: u8, max: u8) -> u8 {
    ((level.min(HEADSET_STEPS as u8) as u32 * max as u32 + HEADSET_STEPS / 2) / HEADSET_STEPS) as u8
}

fn device(addr: Addr, name: &str, rssi: i8) -> BluetoothDevice {
    BluetoothDevice { addr: addr.to_string(), name: name.to_string(), rssi }
}

struct Service {
    ctx: Context,
    audio: AudioService,
    bt: opendeskphone_bt::Bluetooth,
    dir: PathBuf,
    /// What the headset was last told, to send only changes.
    told_call: BtCall,
    told_audio: bool,
    told_volume: Option<u8>,
}

impl Service {
    fn update(&self, f: impl FnOnce(&mut State)) {
        self.ctx.store.update::<State, _>(f);
    }

    fn publish_paired(&self) {
        let paired: Vec<BluetoothDevice> =
            opendeskphone_bt::store::Store::open(&self.dir).devices().iter().map(|d| device(d.key.addr, &d.name, 0)).collect();
        self.update(|s| s.paired = paired);
    }

    fn name_of(&self, addr: Addr) -> String {
        let text = addr.to_string();
        self.ctx
            .store
            .read::<State, _>(|s| s.found.iter().chain(s.paired.iter()).find(|d| d.addr == text).map(|d| d.name.clone()))
            .unwrap_or_default()
    }

    fn on_event(&mut self, event: BtEvent) {
        let store = &self.ctx.store;
        match event {
            BtEvent::Adapter { present, powered, address } => {
                info!("bluetooth: controller {}{}", if present { "present" } else { "absent" }, if powered { ", on" } else { "" });
                self.update(|s| {
                    s.present = present;
                    s.powered = powered;
                    s.address = if present { address.to_string() } else { String::new() };
                    if !powered {
                        s.discovering = false;
                        s.connected.clear();
                        s.audio_open = false;
                    }
                });
            }
            BtEvent::Discovering(on) => self.update(|s| s.discovering = on),
            BtEvent::DeviceFound { addr, name, rssi, .. } => self.update(|s| {
                let text = addr.to_string();
                match s.found.iter_mut().find(|d| d.addr == text) {
                    Some(d) => {
                        d.rssi = rssi;
                        if let Some(name) = name {
                            d.name = name;
                        }
                    }
                    None => s.found.push(device(addr, name.as_deref().unwrap_or(""), rssi)),
                }
                s.found.sort_by_key(|d| std::cmp::Reverse(d.rssi));
            }),
            BtEvent::PairingConfirm { addr, passkey } => {
                let name = self.name_of(addr);
                self.update(|s| s.pairing = Some(BluetoothPairing { addr: addr.to_string(), name, passkey, pin_needed: false }));
                // The question is for the user: bring it to the screen.
                store.post(Command::ShowScreen { name: "bluetooth".into() });
            }
            BtEvent::PinRequest { addr } => {
                let name = self.name_of(addr);
                self.update(|s| s.pairing = Some(BluetoothPairing { addr: addr.to_string(), name, passkey: None, pin_needed: true }));
                store.post(Command::ShowScreen { name: "bluetooth".into() });
            }
            BtEvent::Paired { addr, name } => {
                info!("bluetooth: paired with {addr} '{name}'");
                self.update(|s| {
                    s.pairing = None;
                    s.pairing_with.clear();
                    s.error.clear();
                });
                self.publish_paired();
            }
            BtEvent::PairFailed { addr } => {
                warn!("bluetooth: pairing with {addr} failed");
                let name = self.name_of(addr);
                let who = if name.is_empty() { addr.to_string() } else { name };
                self.update(|s| {
                    s.pairing = None;
                    s.pairing_with.clear();
                    s.error = format!("pairing with {who} failed");
                });
            }
            BtEvent::Unpaired { .. } => self.publish_paired(),
            BtEvent::HeadsetConnected { addr, profile } => {
                info!("bluetooth: headset {addr} connected ({profile:?})");
                self.update(|s| s.connected = addr.to_string());
                store.update::<Audio, _>(|a| a.headset_kind = HeadsetKind::Bluetooth);
                // Tell it where the phone is.
                self.told_call = BtCall::Idle;
                self.told_audio = false;
                self.told_volume = None;
                self.sync();
            }
            BtEvent::HeadsetDisconnected { addr } => {
                info!("bluetooth: headset {addr} disconnected");
                self.update(|s| {
                    s.connected.clear();
                    s.audio_open = false;
                });
                self.audio.bluetooth_voice(false, false);
                store.update::<Audio, _>(|a| {
                    if a.headset_kind == HeadsetKind::Bluetooth {
                        a.headset_kind = HeadsetKind::Wired;
                    }
                });
            }
            BtEvent::AudioOpened { wideband } => {
                self.update(|s| s.audio_open = true);
                self.audio.bluetooth_voice(true, wideband);
            }
            BtEvent::AudioClosed => {
                self.update(|s| s.audio_open = false);
                self.audio.bluetooth_voice(false, false);
            }
            BtEvent::Answer => self.answer(),
            BtEvent::HangUp => self.hang_up(),
            BtEvent::Button => {
                // The one button of a Headset-profile device.
                if store.read::<Calls, _>(|c| c.ringing().next().is_some()) { self.answer() } else { self.hang_up() }
            }
            BtEvent::Dial(number) => {
                store.post(Command::Dial { target: number, account: None, path: Some(AudioPath::Headset) });
            }
            BtEvent::Redial => {
                store.post(Command::SetAudioPath { path: AudioPath::Headset });
                store.post(Command::Redial);
            }
            BtEvent::Dtmf(digit) => {
                if let Some(call) = store.read::<Calls, _>(|c| c.calls.iter().find(|c| c.state == CallState::Active).map(|c| c.id)) {
                    store.post(Command::Dtmf { call, digit });
                }
            }
            BtEvent::SpeakerVolume(level) => {
                let level = from_headset_volume(level, opendeskphone_state::VOLUME_MAX);
                // Not echoed back to the headset.
                self.told_volume = Some(to_headset_volume(level, opendeskphone_state::VOLUME_MAX));
                store.post(Command::SetVolume { target: VolumeTarget::Headset, level });
            }
            // The headset's own microphone gain; the phone has nothing to set.
            BtEvent::MicVolume(_) => {}
        }
    }

    fn answer(&self) {
        let ringing = self.ctx.store.read::<Calls, _>(|c| c.ringing().next().map(|c| c.id));
        if let Some(call) = ringing {
            self.ctx.store.post(Command::Answer { call, path: Some(AudioPath::Headset) });
        }
    }

    /// Hang up as the headset means it: refuse a ringing call, otherwise end
    /// the call in conversation or being set up.
    fn hang_up(&self) {
        let target = self.ctx.store.read::<Calls, _>(|c| {
            let ringing = c.ringing().next().map(|c| (c.id, true));
            ringing.or_else(|| c.focused().filter(|c| !c.state.is_ended()).or_else(|| c.live().next()).map(|c| (c.id, false)))
        });
        match target {
            Some((call, true)) => self.ctx.store.post(Command::Reject { call, busy: false }),
            Some((call, false)) => self.ctx.store.post(Command::Hangup { call }),
            None => false,
        };
    }

    /// Tell the headset what changed on the phone.
    fn sync(&mut self) {
        let connected = self.ctx.store.read::<State, _>(|s| !s.connected.is_empty());
        if !connected {
            return;
        }
        let (call, live) = self.ctx.store.read::<Calls, _>(|c| (headset_call_state(c), c.live().next().is_some()));
        if call != self.told_call {
            self.bt.send(Bt::SetCallState(call.clone()));
            self.told_call = call;
        }
        let (on_headset, volume) = self.ctx.store.read::<Audio, _>(|a| (a.path == AudioPath::Headset, a.headset_volume));
        let audio = live && on_headset;
        if audio != self.told_audio {
            self.bt.send(Bt::SetAudio(audio));
            self.told_audio = audio;
        }
        let volume = to_headset_volume(volume, opendeskphone_state::VOLUME_MAX);
        if self.told_volume != Some(volume) {
            self.bt.send(Bt::SetSpeakerVolume(volume));
            self.told_volume = Some(volume);
        }
    }

    fn command(&mut self, cmd: Command) -> Result<Reply, CommandError> {
        let addr = |text: &str| text.parse::<Addr>().map_err(|()| CommandError::Invalid(format!("'{text}' is not a Bluetooth address")));
        match cmd {
            Command::BluetoothDiscovery { on } => {
                // A new search starts afresh: the devices of the last one
                // and why the last pairing failed go.
                if on {
                    self.update(|s| {
                        s.found.clear();
                        s.error.clear();
                    });
                }
                self.bt.send(if on { Bt::StartDiscovery } else { Bt::StopDiscovery });
            }
            Command::BluetoothPair { addr: a } => {
                let a = addr(&a)?;
                self.update(|s| {
                    s.pairing_with = a.to_string();
                    s.error.clear();
                });
                self.bt.send(Bt::Pair(a));
            }
            Command::BluetoothAnswerPairing { addr: a, accept, pin } => {
                let a = addr(&a)?;
                let question = self.ctx.store.read::<State, _>(|s| s.pairing.clone());
                match question {
                    Some(q) if q.addr == a.to_string() => {
                        self.bt.send(if q.pin_needed {
                            Bt::Pin { addr: a, pin: if accept { pin } else { None } }
                        } else {
                            Bt::ConfirmPairing { addr: a, accept }
                        });
                        self.update(|s| s.pairing = None);
                    }
                    _ => return Err(CommandError::Invalid("no pairing is waiting for that device".into())),
                }
            }
            Command::BluetoothUnpair { addr: a } => self.bt.send(Bt::Unpair(addr(&a)?)),
            Command::BluetoothConnect { addr: a } => self.bt.send(Bt::Connect(addr(&a)?)),
            Command::BluetoothDisconnect => self.bt.send(Bt::Disconnect),
            _ => return Err(CommandError::Invalid("not a Bluetooth command".into())),
        }
        Ok(Reply::Done)
    }
}

async fn run(mut service: Service, mut events: mpsc::UnboundedReceiver<BtEvent>) {
    let commands = service.ctx.store.handle(Domain::Bluetooth);
    let changes = service.ctx.store.subscribe(opendeskphone_state::TopicSet::of(Topic::Calls).with(Topic::Audio));
    loop {
        tokio::select! {
            Some(event) = events.recv() => service.on_event(event),
            _ = changes.changed() => service.sync(),
            Some(envelope) = commands.next() => {
                let (cmd, responder) = envelope.split();
                responder.reply(service.command(cmd));
            }
        }
    }
}

/// Without a controller every Bluetooth command fails with a reason.
async fn refuse(ctx: Context, why: &'static str) {
    let commands = ctx.store.handle(Domain::Bluetooth);
    while let Some(envelope) = commands.next().await {
        envelope.reply(Err(CommandError::Failed(why.into())));
    }
}

pub fn start(ctx: &Context, audio: AudioService) {
    if !super::super::state_model(ctx.model).bluetooth() {
        tokio::spawn(refuse(ctx.clone(), "this model has no Bluetooth"));
        return;
    }
    if ctx.sim {
        tokio::spawn(refuse(ctx.clone(), "the simulator has no Bluetooth controller"));
        return;
    }
    let dir = ctx.paths.own_dir().join("bluetooth");
    let (tx, rx) = mpsc::unbounded_channel();
    let config = opendeskphone_bt::Config {
        name: format!("Digium {}", ctx.model.name()),
        store_dir: dir.clone(),
        // A device that needs a PIN gets the question on the phone's screen.
        default_pin: None,
        // The D80's voice goes from the controller to its voice processor,
        // which takes it narrow-band or wide-band; the D65's goes to the
        // SoC at 8 kHz.
        sco_pcm_params: if ctx.model == crate::model::Model::D80 {
            opendeskphone_bt::SCO_PCM_PARAMS_D80
        } else {
            opendeskphone_bt::SCO_PCM_PARAMS_D65
        },
        wideband: ctx.model == crate::model::Model::D80,
    };
    let bt = match opendeskphone_bt::Bluetooth::spawn(config, move |event| {
        let _ = tx.send(event);
    }) {
        Ok(bt) => bt,
        Err(e) => {
            warn!("bluetooth: {e}");
            ctx.store.update::<State, _>(|s| s.error = e.to_string());
            tokio::spawn(refuse(ctx.clone(), "Bluetooth did not start; see the log"));
            return;
        }
    };
    let service = Service { ctx: ctx.clone(), audio, bt, dir, told_call: BtCall::Idle, told_audio: false, told_volume: None };
    service.publish_paired();
    tokio::spawn(run(service, rx));
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_state::{AccountId, Call, CallId, Party};

    fn calls(states: &[(CallState, Direction)]) -> Calls {
        let mut c = Calls::default();
        for (i, (state, direction)) in states.iter().enumerate() {
            let mut call = Call::new(CallId(i as u32 + 1), AccountId(0), *direction, Party { number: "1002".into(), ..Default::default() });
            call.state = state.clone();
            c.calls.push(call);
        }
        c
    }

    #[test]
    fn call_state_for_the_headset() {
        use Direction::*;
        assert_eq!(headset_call_state(&calls(&[])), BtCall::Idle);
        assert_eq!(headset_call_state(&calls(&[(CallState::Ringing, Incoming)])), BtCall::Incoming { number: Some("1002".into()) });
        assert_eq!(headset_call_state(&calls(&[(CallState::Dialing, Outgoing)])), BtCall::Dialing);
        assert_eq!(headset_call_state(&calls(&[(CallState::Ringback { early_media: false }, Outgoing)])), BtCall::Alerting);
        assert_eq!(headset_call_state(&calls(&[(CallState::Active, Outgoing)])), BtCall::Active);
        assert_eq!(headset_call_state(&calls(&[(CallState::Held { local: true, remote: false }, Outgoing)])), BtCall::Active);
        // A second call ringing in wins over the one in conversation.
        assert_eq!(
            headset_call_state(&calls(&[(CallState::Active, Outgoing), (CallState::Ringing, Incoming)])),
            BtCall::Incoming { number: Some("1002".into()) }
        );
    }

    #[test]
    fn volume_steps() {
        assert_eq!(to_headset_volume(0, 10), 0);
        assert_eq!(to_headset_volume(10, 10), 15);
        assert_eq!(to_headset_volume(5, 10), 8);
        assert_eq!(from_headset_volume(15, 10), 10);
        assert_eq!(from_headset_volume(0, 10), 0);
        // A round trip from the phone's side is stable.
        for level in 0..=10 {
            assert_eq!(from_headset_volume(to_headset_volume(level, 10), 10), level);
        }
    }
}
